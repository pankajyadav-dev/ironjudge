use anyhow::Error;
use nix::sched::{CloneFlags, unshare};
use std::ffi::CString;
use std::fs::{self};
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
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

            println!("Compilation is completed");
    
            let (input_data, expected_output) = testcase_parsing(payload.testcases.clone());
            let testcases_len = payload.testcases.len() as u32;
            let input_file_path = root_dir_path.join("input.txt");
            let output_file_path = root_dir_path.join("output.txt");
            let user_output_file_path = root_dir_path.join("user_output.txt");
            let error_file_path = root_dir_path.join("error.txt");

            tokio::fs::write(&input_file_path, input_data)
                .await
                .unwrap();

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

            let time_limit_secs = std::cmp::max(1, payload.timelimit / 1000);

            let sandbox_config = SandboxConfiguration {
                submissionid: submission_id.clone(),
                memory_limit: payload.memorylimit,
                time_limit: time_limit_secs,
                root_dir: root_dir_path.clone(),
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
            let sub_id_clone = submission_id.clone();
            let sandbox_result = tokio::task::spawn_blocking(move || {
                println!("Starting sandbox execution for job: {}", sub_id_clone);
                sandbox_runner(sandbox_config)
            })
            .await
            .expect("Blocking task panicked");
            let response = match sandbox_result {
                Ok(result) => {
                    if let Some(signal) = result.signal {
                        let error_msg = match signal {
                            11 => "Runtime Error: Segmentation Fault (SIGSEGV)".to_string(),
                            24 => "Time Limit Exceeded (CPU Time)".to_string(), // Killed by RLIMIT_CPU
                            9 => {
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
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_default();
                        if !actual_error.is_empty() {
                            println!("Sandbox stderr:\n{}", actual_error);
                        }

                        ResponsePayload::success(Some(error_msg), 0)
                    } else if result.exit_code != 0 {
                        let error_msg = format!("Runtime Error (Exit Code: {})", result.exit_code);
                        println!("\n=== {} [{}] ===", error_msg, submission_id);
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_default();
                        if !actual_error.is_empty() {
                            println!("Sandbox stderr:\n{}", actual_error);
                        }

                        ResponsePayload::success(Some(error_msg), 0)
                    } else {
                        let actual_output = tokio::fs::read_to_string(&user_output_file_path)
                            .await
                            .unwrap_or_else(|_| "".to_string());
                        let output = tokio::fs::read_to_string(&output_file_path)
                            .await
                            .unwrap_or_else(|_| "".to_string());
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_else(|_| "".to_string());
                        println!("\n=== STDOUT [{}] ===", submission_id);
                        println!("{}", actual_output);
                        println!("========================\n");
                        println!("\n=== output STDOUT [{}] ===", submission_id);
                        println!("{}", output);
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
            drop(permit);
        });
    }
}
pub fn sandbox_runner(sandbox_config: SandboxConfiguration) -> Result<SandboxResult, SandboxError> {
    let start_time = Instant::now();

    let in_file = sandbox_config.input_file;
    let out_file = sandbox_config.output_file;
    let err_file = sandbox_config.error_output;
    let user_out_file = sandbox_config.user_output;
    let out_fd = out_file.as_raw_fd();

    // Unlock the chroot directory so the unprivileged sandbox user can read and execute files
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&sandbox_config.root_dir)
        .unwrap()
        .permissions();
    perms.set_mode(0o777);
    let _ = fs::set_permissions(&sandbox_config.root_dir, perms);

    // --- 1. Setup Cgroups ---
    let cgroup_path = format!("/sys/fs/cgroup/dsa_{}", sandbox_config.submissionid);
    fs::create_dir_all(&cgroup_path).map_err(|e| format!("Cgroup error: {}", e))?;

    let mem_bytes = sandbox_config.memory_limit as u64 * 1024 * 1024;
    fs::write(format!("{}/memory.max", cgroup_path), mem_bytes.to_string()).unwrap();
    let _ = fs::write(format!("{}/memory.swap.max", cgroup_path), "0");
    fs::write(format!("{}/cpu.max", cgroup_path), "100000 100000").unwrap();
    // Increase to 512 for JIT compilers that spawn many background threads on boot
    fs::write(format!("{}/pids.max", cgroup_path), "512").unwrap();

    let readonly_dirs = [
        "/bin",
        "/lib",
        "/lib64",
        "/usr",
        "/etc",
        "/root/.bun",
        "/app/sandbox_bin",
    ];
    let writable_dirs = ["/dev", "/sys"];
    let mut mounts: Vec<(CString, CString, bool)> = Vec::new();

    for dir in &readonly_dirs {
        let host_path = std::path::Path::new(dir);
        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dir.trim_start_matches('/'));
            fs::create_dir_all(&target_path).unwrap_or_default();
            let src = CString::new(*dir).unwrap();
            let tgt = CString::new(target_path.to_str().unwrap()).unwrap();
            mounts.push((src, tgt, true));
        }
    }
    for dir in &writable_dirs {
        let host_path = std::path::Path::new(dir);
        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dir.trim_start_matches('/'));
            fs::create_dir_all(&target_path).unwrap_or_default();
            let src = CString::new(*dir).unwrap();
            let tgt = CString::new(target_path.to_str().unwrap()).unwrap();
            mounts.push((src, tgt, false));
        }
    }

    let proc_path = sandbox_config.root_dir.join("proc");
    fs::create_dir_all(&proc_path).unwrap_or_default();
    let proc_tgt = CString::new(proc_path.to_str().unwrap()).unwrap();

    let tmp_path = sandbox_config.root_dir.join("tmp");
    fs::create_dir_all(&tmp_path).unwrap_or_default();
    let tmp_tgt = CString::new(tmp_path.to_str().unwrap()).unwrap();

    // --- 3. Command Setup ---
    let exe_path = if sandbox_config.run_cmd_exe.starts_with("./") {
        sandbox_config.run_cmd_exe.replacen(".", "", 1)
    } else {
        sandbox_config.run_cmd_exe.clone()
    };

    let mut cmd = Command::new(&exe_path);
    cmd.args(&sandbox_config.run_cmd_args);

    // Clear the host environment and provide safe temp paths for JIT runtimes
    cmd.env_clear();
    cmd.env(
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/root/.bun/bin",
    );
    cmd.env("HOME", "/tmp");
    cmd.env("TMPDIR", "/tmp");

    cmd.stdin(Stdio::from(in_file));
    cmd.stdout(Stdio::from(user_out_file));
    cmd.stderr(Stdio::from(err_file));

    // Prepare paths ahead of time for safe usage inside pre_exec
    let cgroup_procs_file = CString::new(format!("{}/cgroup.procs", cgroup_path)).unwrap();
    let _setgroups_file = CString::new("/proc/self/setgroups").unwrap();
    let _uid_map_file = CString::new("/proc/self/uid_map").unwrap();
    let _gid_map_file = CString::new("/proc/self/gid_map").unwrap();
    let _map_data = b"0 65534 1\n";
    let _deny_data = b"deny\n";
    let time_limit = sandbox_config.time_limit as u64;
    let root_dir_c = CString::new(sandbox_config.root_dir.to_str().unwrap()).unwrap();

    // --- 4. Pre-Exec Hook ---
    unsafe {
        cmd.pre_exec(move || {
            // Helper for async-signal-safe file writing (avoids allocator deadlocks)
            let write_sys = |path: &CString, data: &[u8]| -> bool {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY);
                if fd < 0 {
                    return false;
                }
                let res = libc::write(fd, data.as_ptr() as *const libc::c_void, data.len());
                libc::close(fd);
                res == data.len() as isize
            };

            // Write PID to cgroups using basic math to avoid format! allocations
            let mut pid = libc::getpid();
            let mut pid_buf = [0u8; 32];
            let mut temp = [0u8; 32];
            let mut temp_len = 0;
            while pid > 0 {
                temp[temp_len] = b'0' + (pid % 10) as u8;
                pid /= 10;
                temp_len += 1;
            }
            let mut i = 0;
            while temp_len > 0 {
                temp_len -= 1;
                pid_buf[i] = temp[temp_len];
                i += 1;
            }
            pid_buf[i] = b'\n';

            if !write_sys(&cgroup_procs_file, &pid_buf[..i + 1]) {
                libc::_exit(101);
            }

            // Unshare ALL EXCEPT user namespace first
            let flags = CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWIPC
                | CloneFlags::CLONE_NEWNET
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWNS;
            if unshare(flags).is_err() {
                libc::_exit(102);
            }

            for (src, tgt, readonly) in &mounts {
                if libc::mount(
                    src.as_ptr(),
                    tgt.as_ptr(),
                    b"bind\0".as_ptr() as *const i8,
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                ) != 0
                {
                    libc::_exit(103);
                }

                if *readonly {
                    if libc::mount(
                        src.as_ptr(),
                        tgt.as_ptr(),
                        b"bind\0".as_ptr() as *const i8,
                        libc::MS_BIND | libc::MS_REC | libc::MS_REMOUNT | libc::MS_RDONLY,
                        std::ptr::null(),
                    ) != 0
                    {
                        libc::_exit(104);
                    }
                }
            }

            if libc::mount(
                b"proc\0".as_ptr() as *const i8,
                proc_tgt.as_ptr(),
                b"proc\0".as_ptr() as *const i8,
                0,
                std::ptr::null(),
            ) != 0
            {
                libc::_exit(105);
            }

            if libc::mount(
                b"tmpfs\0".as_ptr() as *const i8,
                tmp_tgt.as_ptr(),
                b"tmpfs\0".as_ptr() as *const i8,
                0,
                b"size=128m,mode=1777\0".as_ptr() as *const libc::c_void,
            ) != 0
            {
                libc::_exit(106);
            }

            if libc::chroot(root_dir_c.as_ptr()) != 0 {
                libc::_exit(107);
            }
            if libc::chdir(b"/\0".as_ptr() as *const i8) != 0 {
                libc::_exit(108);
            }
            if libc::setgroups(0, std::ptr::null()) != 0 {
                libc::_exit(110);
            }

            // Set GID and UID to 'nobody' (65534)
            if libc::setgid(65534) != 0 {
                libc::_exit(111);
            }
            if libc::setuid(65534) != 0 {
                libc::_exit(112);
            }

            let cpu_rlim = libc::rlimit {
                rlim_cur: time_limit,
                rlim_max: time_limit + 1,
            };
            libc::setrlimit(libc::RLIMIT_CPU, &cpu_rlim);

            let core_rlim = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::setrlimit(libc::RLIMIT_CORE, &core_rlim);

            let fsize_rlim = libc::rlimit {
                rlim_cur: 64 * 1024 * 1024,
                rlim_max: 64 * 1024 * 1024,
            };
            libc::setrlimit(libc::RLIMIT_FSIZE, &fsize_rlim);

            if libc::dup2(out_fd, 3) < 0 {
                libc::_exit(109);
            }

            Ok(())
        });
    }

    let mut child = cmd.spawn().map_err(|e| format!("Spawn failed: {}", e))?;
    let status = child.wait().map_err(|e| format!("Wait error: {}", e))?;

    let cgroup_procs_file = format!("{}/cgroup.procs", cgroup_path);

    loop {
        let procs = fs::read_to_string(&cgroup_procs_file).unwrap_or_default();
        let pids: Vec<i32> = procs
            .lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .collect();

        if pids.is_empty() {
            break;
        }

        for pid in pids {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    if let Err(e) = fs::remove_dir(&cgroup_path) {
        println!("warning failed to remove cgroups {} : {}", cgroup_path, e);
    };

    let result = SandboxResult {
        exit_code: status.code().unwrap_or(-1),
        signal: status.signal(),
        wall_time_ms: start_time.elapsed().as_millis(),
    };

    println!("sandbox result {:?}", result);
    Ok(result)
}
