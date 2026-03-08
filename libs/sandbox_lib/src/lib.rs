use anyhow::Error;
use nix::sched::{CloneFlags, unshare};
use std::ffi::CString;
use std::fs::{self};
use std::os::fd::AsRawFd;
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
                    return;
                }
            }

            // ==========================================
            // 3. PREPARE I/O FILES & TESTCASES ASYNC
            // ==========================================
            let (input_data, expected_output) = testcase_parsing(payload.testcases.clone());
            let testcases_len = payload.testcases.len() as u32;

            let input_file_path = root_dir_path.join("input.txt");
            let output_file_path = root_dir_path.join("output.txt");
            let user_output_file_path = root_dir_path.join("user_output.txt");
            let error_file_path = root_dir_path.join("error.txt");

            // Write input data asynchronously
            tokio::fs::write(&input_file_path, input_data)
                .await
                .unwrap();

            // Open/Create files asynchronously and convert to std::fs::File for the blocking command
            let in_file = tokio::fs::File::open(&input_file_path)
                .await
                .unwrap()
                .into_std();
            let out_file = tokio::fs::File::create(&output_file_path)
                .await
                .unwrap()
                .into_std();
            let err_file = tokio::fs::File::create(&error_file_path)
                .await
                .unwrap()
                .into_std();
            let user_output_file = tokio::fs::File::create(&user_output_file_path)
                .await
                .unwrap()
                .into_std();

            // ==========================================
            // 4. BUILD THE CONFIGURATION
            // ==========================================
            let time_limit_secs = std::cmp::max(1, payload.timelimit / 1000);

            let sandbox_config = SandboxConfiguration {
                submissionid: submission_id.clone(),
                memory_limit: payload.memorylimit,
                time_limit: time_limit_secs,
                root_dir: root_dir_path.clone(),

                // Pass the open file handles directly
                input_file: in_file.await,
                output_file: out_file.await,
                error_output: err_file.await,
                user_output: user_output_file.await,

                run_cmd_exe: language_config.run_cmd.0.to_string(),
                run_cmd_args: language_config
                    .run_cmd
                    .1
                    .iter()
                    .map(|&s| s.to_string())
                    .collect(),
            };

            // ==========================================
            // 5. EXECUTE THE SANDBOX (BLOCKING)
            // ==========================================
            let sub_id_clone = submission_id.clone();

            let sandbox_result = tokio::task::spawn_blocking(move || {
                println!("Starting sandbox execution for job: {}", sub_id_clone);
                sandbox_runner(sandbox_config)
            })
            .await
            .expect("Blocking task panicked");

            // ==========================================
            // 6. ASYNC OUTPUT EVALUATION
            // ==========================================
            let response = match sandbox_result {
                Ok(result) => {
                    println!("sandbox result {:?}", &result);

                    // 1. Process was killed by a Signal (e.g., Segfault, TLE, MLE)
                    if let Some(signal) = result.signal {
                        let error_msg = match signal {
                            11 => "Runtime Error: Segmentation Fault (SIGSEGV)".to_string(),
                            24 => "Time Limit Exceeded (CPU Time)".to_string(), // Killed by RLIMIT_CPU
                            9 => {
                                // SIGKILL: Check wall time against the payload limit to differentiate TLE vs MLE.
                                // If it ran longer than the allowed time, it's TLE. Otherwise, OOM Killer stepped in.
                                if result.wall_time_ms >= payload.timelimit as u128 {
                                    "Time Limit Exceeded (Wall Time)".to_string()
                                } else {
                                    "Memory Limit Exceeded".to_string()
                                }
                            }
                            8 => "Runtime Error: Floating Point Exception".to_string(),
                            6 => "Runtime Error: Aborted (SIGABRT)".to_string(),
                            _ => format!("Runtime Error: Killed by signal {}", signal),
                        };

                        println!("\n=== {} [{}] ===", error_msg, submission_id);

                        // Print stderr if available for debugging
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_default();
                        if !actual_error.is_empty() {
                            println!("Sandbox stderr:\n{}", actual_error);
                        }

                        ResponsePayload::success(Some(error_msg), 0)
                    }
                    // 2. Process exited cleanly but with an error code
                    else if result.exit_code != 0 {
                        let error_msg = format!("Runtime Error (Exit Code: {})", result.exit_code);
                        println!("\n=== {} [{}] ===", error_msg, submission_id);

                        // Print stderr to show why it crashed
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_default();
                        if !actual_error.is_empty() {
                            println!("Sandbox stderr:\n{}", actual_error);
                        }

                        ResponsePayload::success(Some(error_msg), 0)
                    }
                    // 3. Process completed successfully, evaluate output
                    else {
                        // Read files asynchronously
                        let actual_output = tokio::fs::read_to_string(&user_output_file_path)
                            .await
                            .unwrap_or_else(|_| "".to_string());
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_else(|_| "".to_string());

                        println!("\n=== STDOUT [{}] ===", submission_id);
                        println!("{}", actual_output);
                        println!("========================\n");

                        if !actual_error.is_empty() {
                            println!("\n=== STDERR [{}] ===", submission_id);
                            println!("{}", actual_error);
                            println!("========================\n");
                        }

                        let is_match = actual_output.trim() == expected_output.trim();
                        let msg = if is_match {
                            "Accepted: Output matched!".to_string()
                        } else {
                            format!(
                                "Wrong Answer.\nExpected:\n{}\nGot:\n{}",
                                expected_output.trim(),
                                actual_output.trim()
                            )
                        };

                        ResponsePayload::success(Some(msg), testcases_len)
                    }
                }
                Err(e) => {
                    println!("Sandbox engine failed fundamentally: {:?}", e);
                    ResponsePayload::error()
                }
            };

            info!(
                "Job {} completed with status: {:?}",
                submission_id, response.status
            );

            // Drop permit at the end of the async flow
            drop(permit);
        });
    }
}

pub fn sandbox_runner(sandbox_config: SandboxConfiguration) -> Result<SandboxResult, SandboxError> {
    let start_time = Instant::now();

    // ==========================================
    // 1. OPEN FILES FOR I/O REDIRECTION
    // ==========================================
    let in_file = sandbox_config.input_file;
    let out_file = sandbox_config.output_file;
    let err_file = sandbox_config.error_output;
    let user_out_file = sandbox_config.user_output; // Extract user output file

    // We get the raw file descriptor for output.txt so we can bind it to FD 3 later
    let out_fd = out_file.as_raw_fd();

    // ==========================================
    // 2. CGROUPS V2
    // ==========================================
    let cgroup_path = format!("/sys/fs/cgroup/dsa_{}", sandbox_config.submissionid);
    fs::create_dir_all(&cgroup_path).map_err(|e| format!("Cgroup error: {}", e))?;

    let mem_bytes = sandbox_config.memory_limit as u64 * 1024 * 1024;
    fs::write(format!("{}/memory.max", cgroup_path), mem_bytes.to_string()).unwrap();
    let _ = fs::write(format!("{}/memory.swap.max", cgroup_path), "0");
    fs::write(format!("{}/cpu.max", cgroup_path), "100000 100000").unwrap();
    fs::write(format!("{}/pids.max", cgroup_path), "64").unwrap();

    // ==========================================
    // 3. PREPARE BIND MOUNTS FOR INTERPRETED LANGUAGES
    // ==========================================
    let bind_dirs = ["/bin", "/lib", "/lib64", "/usr", "/etc"];
    let mut mounts = Vec::new();

    for dir in &bind_dirs {
        let host_path = std::path::Path::new(dir);
        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dir.trim_start_matches('/'));
            fs::create_dir_all(&target_path).unwrap_or_default();

            let src = CString::new(*dir).unwrap();
            let tgt = CString::new(target_path.to_str().unwrap()).unwrap();
            mounts.push((src, tgt));
        }
    }

    // ==========================================
    // 4. PREPARE THE PAYLOAD & WIRE FILE DESCRIPTORS
    // ==========================================
    let exe_path = if sandbox_config.run_cmd_exe.starts_with("./") {
        sandbox_config.run_cmd_exe.replacen(".", "", 1)
    } else {
        sandbox_config.run_cmd_exe.clone()
    };

    let mut cmd = Command::new(&exe_path);
    cmd.args(&sandbox_config.run_cmd_args);

    // FIX: Standard Output now goes to user_output.txt (FD 1)
    cmd.stdin(Stdio::from(in_file));
    cmd.stdout(Stdio::from(user_out_file));
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
                rlim_max: time_limit + 1,
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

            // F. MAP FD 3 FOR EVALUATION OUTPUT
            // dup2 duplicates `out_fd` specifically into FD 3 and strips the CLOEXEC flag
            // so the sandboxed process can inherit and use it natively.
            if libc::dup2(out_fd, 3) < 0 {
                libc::_exit(1);
            }

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
