use anyhow::Error;
use nix::sched::{CloneFlags, unshare};
use std::ffi::CString;
use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Instant;
use std::{sync::Arc, thread::available_parallelism};
use tempfile::{Builder, TempDir};
use tokio::sync::Semaphore;
use tracing::info;
use types_lib::{
    LanguageConfig, ResponsePayload, SandboxConfiguration, SandboxError, SandboxResult,
    TaskPayload, TestCaseType,
};
pub fn get_heavy_tasks_threads() -> usize {
    let total_cores = available_parallelism().map(|n| n.get()).unwrap_or(4);

    if total_cores <= 2 { 1 } else { total_cores - 2 }
}

pub async fn create_temp_file(directory: &str) -> Result<TempDir, Error> {
    let ram_dir = Builder::new().prefix(directory).tempdir_in("/dev/shm")?;
    Ok(ram_dir)
}

pub fn testcase_parsing(payload: Vec<TestCaseType>) -> (String, String) {
    let mut input_data = format!("{}\n", payload.len());
    let mut expected_output_data = String::new();

    for tc in &payload {
        input_data.push_str(&tc.input);
        if !input_data.ends_with('\n') {
            input_data.push('\n');
        }

        if !expected_output_data.ends_with('\n') {
            expected_output_data.push('\n');
        }
    }
    (input_data, expected_output_data)
}

pub async fn execute_submissions_detached(
    tasks: Vec<(String, TaskPayload)>,
    concurrency_limiter: Arc<Semaphore>,
) {
    for (submission_id, payload) in tasks {
        let permit = concurrency_limiter
            .clone()
            .acquire_owned()
            .await
            .expect("Semaphore closed");

        tokio::spawn(async move {
            // ==========================================
            // 1. CREATE WORKSPACE
            // ==========================================
            let temp_dir = match create_temp_file(&submission_id).await {
                Ok(dir) => dir,
                Err(e) => {
                    println!(
                        "Failed to create temp directory for {}: {}",
                        submission_id, e
                    );
                    drop(permit);
                    return;
                }
            };

            let root_dir_path = temp_dir.path().to_path_buf();

            // ==========================================
            // 2. WRITE SOURCE AND COMPILE
            // ==========================================
            let language_config = LanguageConfig::get(&payload.language);
            let source_path = root_dir_path.join(language_config.source_filename);
            tokio::fs::write(&source_path, &payload.code).await.unwrap();

            if let Some((compiler, args)) = &language_config.compile_cmd {
                let compile_status = tokio::process::Command::new(compiler)
                    .args(args)
                    .current_dir(&root_dir_path)
                    .status()
                    .await
                    .unwrap();

                if !compile_status.success() {
                    println!("Compilation failed for {}", submission_id);
                    drop(permit);
                    return;
                }
            }

            // ==========================================
            // 3. PREPARE I/O FILES & TESTCASES
            // ==========================================
            let (input_data, expected_output) = testcase_parsing(payload.testcases.clone());

            let input_file_path = root_dir_path.join("input.txt");
            let output_file_path = root_dir_path.join("output.txt");
            let user_output_file_path = root_dir_path.join("user_output.txt");
            let error_file_path = root_dir_path.join("error.txt");

            tokio::fs::write(&input_file_path, input_data)
                .await
                .unwrap();

            // ==========================================
            // 4. BUILD THE CONFIGURATION
            // ==========================================
            let time_limit_secs = std::cmp::max(1, payload.timelimit / 1000);

            let sandbox_config = SandboxConfiguration {
                submissionid: submission_id.clone(),
                memory_limit: payload.memorylimit,
                time_limit: time_limit_secs,
                root_dir: root_dir_path.clone(),
                input_file: input_file_path.clone(),
                output_file: output_file_path.clone(),
                error_output: error_file_path.clone(),
                user_output: user_output_file_path.clone(),

                // Properly convert &'static str to String and Vec<String>
                run_cmd_exe: language_config.run_cmd.0.to_string(),
                run_cmd_args: language_config
                    .run_cmd
                    .1
                    .iter()
                    .map(|&s| s.to_string())
                    .collect(),
            };

            // ==========================================
            // 5. EXECUTE THE SANDBOX
            // ==========================================
            let (sub_id, response) = tokio::task::spawn_blocking(move || {
                println!("Starting sandbox execution for job: {}", submission_id);

                match sandbox_runner(sandbox_config) {
                    Ok(result) => {
                        if result.signal == Some(9) {
                            let response = ResponsePayload::success(
                                Some("Time Limit or Memory Limit Exceeded".to_string()),
                                0,
                            );
                            return (submission_id, response);
                        }

                        // 2. Just print exactly what the sandbox produced
                        let actual_output = std::fs::read_to_string(&output_file_path)
                            .unwrap_or_else(|_| "".to_string());
                        let actual_error = std::fs::read_to_string(&error_file_path)
                            .unwrap_or_else(|_| "".to_string());

                        println!("\n=== STDOUT [{}] ===", submission_id);
                        println!("{}", actual_output);
                        println!("========================\n");

                        if !actual_error.is_empty() {
                            println!("\n=== STDERR [{}] ===", submission_id);
                            println!("{}", actual_error);
                            println!("========================\n");
                        }
                        if !actual_error.is_empty() {
                            println!("Sandbox stderr: {}", actual_error);
                        }

                        let is_match = actual_output.trim() == expected_output.trim();
                        let msg = if is_match {
                            "Output matched!".to_string()
                        } else {
                            format!(
                                "Wrong Answer.\nExpected:\n{}\nGot:\n{}",
                                expected_output.trim(),
                                actual_output.trim()
                            )
                        };

                        let response =
                            ResponsePayload::success(Some(msg), payload.testcases.len() as u32);

                        (submission_id, response)
                    }
                    Err(e) => {
                        println!("Sandbox engine failed fundamentally: {}", e);
                        let response = ResponsePayload::error();
                        (submission_id, response)
                    }
                }
            })
            .await
            .expect("Blocking task panicked");

            info!(
                "Job {} completed with status: {:?}",
                sub_id, response.status
            );
            drop(permit);
        });
    }
}

pub fn sandbox_runner(sandbox_config: SandboxConfiguration) -> Result<SandboxResult, SandboxError> {
    let start_time = Instant::now();

    // ==========================================
    // 1. OPEN FILES FOR I/O REDIRECTION
    // ==========================================
    let in_file = File::open(&sandbox_config.input_file)
        .map_err(|e| format!("Failed to open input file: {}", e))?;
    let out_file = File::create(&sandbox_config.output_file)
        .map_err(|e| format!("Failed to create output file: {}", e))?;
    let err_file = File::create(&sandbox_config.error_output)
        .map_err(|e| format!("Failed to create error file: {}", e))?;

    // ==========================================
    // 2. CGROUPS V2
    // ==========================================
    let cgroup_path = format!("/sys/fs/cgroup/dsa_{}", sandbox_config.submissionid);
    fs::create_dir_all(&cgroup_path).map_err(|e| format!("Cgroup error: {}", e))?;

    let mem_bytes = sandbox_config.memory_limit as u64 * 1024 * 1024;
    fs::write(format!("{}/memory.max", cgroup_path), mem_bytes.to_string()).unwrap();
    fs::write(format!("{}/cpu.max", cgroup_path), "100000 100000").unwrap();
    fs::write(format!("{}/pids.max", cgroup_path), "64").unwrap();

    // ==========================================
    // 3. PREPARE BIND MOUNTS FOR INTERPRETED LANGUAGES
    // ==========================================
    // We must project these host directories into the sandbox so Python/Java can run.
    let bind_dirs = ["/bin", "/lib", "/lib64", "/usr", "/etc"];
    let mut mounts = Vec::new();

    for dir in &bind_dirs {
        let host_path = std::path::Path::new(dir);
        if host_path.exists() {
            // Create the empty landing folder in the sandbox (e.g., /dev/shm/job_id/usr)
            let target_path = sandbox_config.root_dir.join(dir.trim_start_matches('/'));
            fs::create_dir_all(&target_path).unwrap_or_default();

            // Prepare C-Strings to safely pass into the unsafe pre_exec closure
            let src = CString::new(*dir).unwrap();
            let tgt = CString::new(target_path.to_str().unwrap()).unwrap();
            mounts.push((src, tgt));
        }
    }

    // ==========================================
    // 4. PREPARE THE PAYLOAD & WIRE FILE DESCRIPTORS
    // ==========================================
    // PATH TRANSLATION FOR CHROOT:
    // If the config asks for "./solution", we must translate it to "/solution".
    let exe_path = if sandbox_config.run_cmd_exe.starts_with("./") {
        sandbox_config.run_cmd_exe.replacen(".", "", 1)
    } else {
        sandbox_config.run_cmd_exe.clone()
    };

    let mut cmd = Command::new(&exe_path);
    cmd.args(&sandbox_config.run_cmd_args);

    // Wire the standard streams
    cmd.stdin(Stdio::from(in_file));
    cmd.stdout(Stdio::from(out_file));
    cmd.stderr(Stdio::from(err_file));

    // ==========================================
    // 5. THE ISOLATION CELL (pre_exec hook)
    // ==========================================
    let cgroup_procs_file = format!("{}/cgroup.procs", cgroup_path);
    let time_limit = sandbox_config.time_limit as u64;
    let root_dir_c = CString::new(sandbox_config.root_dir.to_str().unwrap()).unwrap();

    unsafe {
        cmd.pre_exec(move || {
            // A. LOCK RESOURCES FIRST
            let pid = libc::getpid();
            if fs::write(&cgroup_procs_file, pid.to_string()).is_err() {
                libc::_exit(1);
            }

            // B. TOTAL NAMESPACE ISOLATION
            let flags = CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWIPC
                | CloneFlags::CLONE_NEWNET
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWUSER;
            if unshare(flags).is_err() {
                libc::_exit(1);
            }

            // C. BIND MOUNT THE HOST RUNTIMES
            for (src, tgt) in &mounts {
                // 1. Create the bind mount
                if libc::mount(
                    src.as_ptr(),
                    tgt.as_ptr(),
                    b"bind\0".as_ptr() as *const i8,
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                ) != 0
                {
                    libc::_exit(1);
                }

                // 2. Remount it as STRICTLY READ-ONLY so user code cannot alter host files
                if libc::mount(
                    src.as_ptr(),
                    tgt.as_ptr(),
                    b"bind\0".as_ptr() as *const i8,
                    libc::MS_BIND | libc::MS_REC | libc::MS_REMOUNT | libc::MS_RDONLY,
                    std::ptr::null(),
                ) != 0
                {
                    libc::_exit(1);
                }
            }

            // D. ISOLATE THE FILESYSTEM (chroot)
            if libc::chroot(root_dir_c.as_ptr()) != 0 {
                libc::_exit(1);
            }
            if libc::chdir(b"/\0".as_ptr() as *const i8) != 0 {
                libc::_exit(1);
            }

            // E. APPLY HARDWARE AND TIME SAFETY NETS
            let cpu_rlim = libc::rlimit {
                rlim_cur: time_limit,
                rlim_max: time_limit,
            };
            libc::setrlimit(libc::RLIMIT_CPU, &cpu_rlim);

            let proc_rlim = libc::rlimit {
                rlim_cur: 64,
                rlim_max: 64,
            };
            libc::setrlimit(libc::RLIMIT_NPROC, &proc_rlim);

            let core_rlim = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::setrlimit(libc::RLIMIT_CORE, &core_rlim);

            Ok(())
        });
    }

    // ==========================================
    // 6. EXECUTE AND WAIT
    // ==========================================
    let mut child = cmd.spawn().map_err(|e| format!("Spawn failed: {}", e))?;
    let status = child.wait().map_err(|e| format!("Wait error: {}", e))?;

    // ==========================================
    // 7. CLEANUP CGROUPS
    // ==========================================
    let _ = fs::remove_dir(&cgroup_path);

    use std::os::unix::process::ExitStatusExt;
    let result = SandboxResult {
        exit_code: status.code().unwrap_or(-1),
        signal: status.signal(),
        wall_time_ms: start_time.elapsed().as_millis(),
    };

    println!("sandbox result {:?}", result);
    Ok(result)
}
