use anyhow::Error;
use nix::sched::{CloneFlags, unshare};
use redis_lib::{
    RedisPool, acknowledge_stream_message, push_result_to_redis, set_processing_status,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};
use std::convert::TryInto;
use std::ffi::CString;
use std::fs::{self};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::time::Instant;
use std::{sync::Arc, thread::available_parallelism};
use tempfile::{Builder, TempDir};
use tokio::sync::Semaphore;
use tracing::{error, info};
use types_lib::{
    FailedTestDetail, LanguageConfig, ResponsePayload, SandboxConfiguration, SandboxError,
    SandboxResult, TaskPayload, TaskType, TestCaseResult, TestCaseType,
};

pub fn get_heavy_tasks_threads() -> usize {
    let total_cores = available_parallelism().map(|n| n.get()).unwrap_or(4);
    if total_cores <= 2 { 1 } else { total_cores - 2 }
}

pub async fn create_temp_file(directory: &str) -> Result<TempDir, Error> {
    let ram_dir = Builder::new().prefix(directory).tempdir_in("/dev/shm")?;
    Ok(ram_dir)
}

pub fn testcase_parsing(payload: Vec<TestCaseType>) -> (String, Vec<String>) {
    let mut input_data = format!("{}\n", payload.len());
    let mut expected_output_data = Vec::new();

    for tc in &payload {
        input_data.push_str(&tc.input);
        if !input_data.ends_with('\n') {
            input_data.push('\n');
        }
        for line in tc.output.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                expected_output_data.push(trimmed.to_string());
            }
        }
    }
    (input_data, expected_output_data)
}

pub fn validate_test_cases(
    fd3_output: Vec<String>,
    testcases: &[TestCaseType],
    tasktype: &TaskType,
    user_stdout: Option<String>,
) -> ResponsePayload {
    match tasktype {
        TaskType::Run => {
            let results: Vec<TestCaseResult> = testcases
                .iter()
                .enumerate()
                .map(|(i, tc)| {
                    let result = fd3_output.get(i).cloned().unwrap_or_default();
                    TestCaseResult {
                        id: tc.id,
                        input: tc.input.trim().to_string(),
                        output: tc.output.trim().to_string(),
                        result,
                    }
                })
                .collect();

            let results_json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
            ResponsePayload::success(user_stdout, Some(results_json), testcases.len() as u32)
        }
        TaskType::Test => {
            // Compare each test case output line against expected using fd3 output
            let expected: Vec<String> = testcases
                .iter()
                .flat_map(|tc| {
                    tc.output
                        .lines()
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                })
                .collect();

            let total = expected.len();
            let mut passed: u32 = 0;

            for (i, exp) in expected.iter().enumerate() {
                let actual = fd3_output.get(i).map(|s| s.as_str()).unwrap_or("");
                if actual != exp.as_str() {
                    // Find which original test case this line belongs to
                    let mut line_cursor = 0;
                    let mut failed_tc = &testcases[0];
                    for tc in testcases {
                        let tc_line_count =
                            tc.output.lines().filter(|l| !l.trim().is_empty()).count();
                        if i < line_cursor + tc_line_count {
                            failed_tc = tc;
                            break;
                        }
                        line_cursor += tc_line_count;
                    }

                    let detail = FailedTestDetail {
                        id: failed_tc.id,
                        input: failed_tc.input.trim().to_string(),
                        expected: exp.clone(),
                        actual: actual.to_string(),
                    };
                    let detail_json =
                        serde_json::to_string(&detail).unwrap_or_else(|_| "{}".to_string());
                    return ResponsePayload::test_failed(passed, Some(detail_json), user_stdout);
                }
                passed += 1;
            }

            // All matched
            ResponsePayload::success(user_stdout, None, total as u32)
        }
    }
}

pub fn build_strict_seccomp_profile() -> Vec<libc::sock_filter> {
    let mut rules = std::collections::BTreeMap::new();

    let allowed_syscalls = vec![
        // Memory Management
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_brk,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_mincore,
        libc::SYS_membarrier,
        // Basic I/O & File Operations
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_open,
        libc::SYS_openat,
        libc::SYS_close,
        libc::SYS_lseek,
        libc::SYS_ioctl,
        libc::SYS_fcntl,
        libc::SYS_dup,
        libc::SYS_dup2,
        libc::SYS_dup3,
        libc::SYS_pipe,
        libc::SYS_pipe2,
        libc::SYS_socketpair,
        // High-Performance Async I/O (Required by Bun)
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        // File Metadata & Directory Resolution
        libc::SYS_stat,
        libc::SYS_fstat,
        libc::SYS_lstat,
        libc::SYS_newfstatat,
        libc::SYS_statx,
        libc::SYS_statfs,
        libc::SYS_fstatfs,
        libc::SYS_access,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
        libc::SYS_readlink,
        libc::SYS_readlinkat,
        libc::SYS_getcwd,
        libc::SYS_getdents64,
        // Threading & Concurrency
        libc::SYS_clone,
        libc::SYS_clone3,
        libc::SYS_execve,
        libc::SYS_futex,
        libc::SYS_set_robust_list,
        libc::SYS_set_tid_address,
        libc::SYS_epoll_create1,
        libc::SYS_epoll_ctl,
        libc::SYS_epoll_wait,
        libc::SYS_epoll_pwait,
        libc::SYS_eventfd2,
        libc::SYS_poll,
        libc::SYS_select,
        libc::SYS_sched_yield,
        libc::SYS_sched_getaffinity,
        // Signals & Process State
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_prctl,
        libc::SYS_arch_prctl,
        libc::SYS_rseq,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_tgkill,
        libc::SYS_wait4,
        // Time, Randomness & System Info
        libc::SYS_gettimeofday,
        libc::SYS_clock_gettime,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_uname,
        libc::SYS_sysinfo,
        libc::SYS_getrandom,
        libc::SYS_prlimit64,
        libc::SYS_getrusage,
        // Termination
        libc::SYS_exit,
        libc::SYS_exit_group,
        // Safe Network Initialization (Blocked by CLONE_NEWNET anyway)
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_shutdown,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_gettimeofday,
        libc::SYS_clock_gettime,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_uname,
        libc::SYS_sysinfo,
        libc::SYS_getrandom,
        libc::SYS_prlimit64,
        libc::SYS_getrusage,
        // --- NEW: High-Performance Event Loop Timers (Required for Bun/uSockets) ---
        libc::SYS_timerfd_create,
        libc::SYS_timerfd_settime,
        libc::SYS_timerfd_gettime,
        libc::SYS_signalfd4,
        // Temp Filesystem Manipulation (Jailed by pivot_root)
        libc::SYS_mkdir,
        libc::SYS_rmdir,
        libc::SYS_rename,
        libc::SYS_unlink,
        libc::SYS_unlinkat,
        libc::SYS_chmod,
        libc::SYS_fchmod,
        libc::SYS_chdir,
    ];

    for syscall in allowed_syscalls {
        rules.insert(syscall, vec![]);
    }

    let filter = SeccompFilter::new(
        rules,
        // THE FIX: Change KillProcess to Errno(ENOSYS)
        // This makes unknown syscalls act like they just don't exist in the OS,
        // forcing Java and Bun to gracefully fallback to older, safe syscalls.
        SeccompAction::Errno(libc::ENOSYS as u32),
        SeccompAction::Allow,
        std::env::consts::ARCH.try_into().unwrap(),
    )
    .expect("Failed to build seccomp filter structure");

    let bpf_prog: BpfProgram = filter.try_into().expect("Failed to compile to BPF");

    bpf_prog
        .into_iter()
        .map(|inst| libc::sock_filter {
            code: inst.code,
            jt: inst.jt,
            jf: inst.jf,
            k: inst.k,
        })
        .collect()
}

pub async fn execute_submissions_detached(
    tasks: Vec<(String, String, TaskPayload)>,
    concurrency_limiter: Arc<Semaphore>,
    redis_pool: Arc<RedisPool>,
    stream_key: String,
    group_name: String,
) {
    for (stream_entry_id, submission_id, payload) in tasks {
        let permit = concurrency_limiter
            .clone()
            .acquire_owned()
            .await
            .expect("Semaphore closed");
        let pool = redis_pool.clone();
        let s_key = stream_key.clone();
        let g_name = group_name.clone();

        tokio::spawn(async move {
            // Set processing status in Redis
            if let Err(e) = set_processing_status(&pool, &submission_id).await {
                error!(
                    "Failed to set processing status for {}: {}",
                    submission_id, e
                );
            }

            let temp_dir = match create_temp_file(&submission_id).await {
                Ok(dir) => dir,
                Err(e) => {
                    error!(
                        "Failed to create temp directory for {}: {}",
                        submission_id, e
                    );
                    let resp = ResponsePayload::error();
                    let _ = push_result_to_redis(&pool, &submission_id, &resp).await;
                    let _ =
                        acknowledge_stream_message(&pool, &s_key, &g_name, &stream_entry_id).await;
                    return;
                }
            };

            let root_dir_path = temp_dir.path().to_path_buf();
            let language_config = LanguageConfig::get(&payload.language);
            let source_path = root_dir_path.join(language_config.source_filename);
            tokio::fs::write(&source_path, &payload.code).await.unwrap();

            if let Some((compiler, args)) = &language_config.compile_cmd {
                let _error_file_path = root_dir_path.join("compile_error.txt");
                let compile_result = tokio::process::Command::new(compiler)
                    .args(args)
                    .current_dir(&root_dir_path)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await;

                match compile_result {
                    Ok(output) => {
                        if !output.status.success() {
                            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                            info!("Compilation failed for {}: {}", submission_id, stderr);
                            let resp = ResponsePayload::compiler_error(Some(stderr));
                            let _ = push_result_to_redis(&pool, &submission_id, &resp).await;
                            let _ = acknowledge_stream_message(
                                &pool,
                                &s_key,
                                &g_name,
                                &stream_entry_id,
                            )
                            .await;
                            drop(permit);
                            return;
                        }
                    }
                    Err(e) => {
                        let resp = ResponsePayload::compiler_error(Some(format!(
                            "Compiler not found: {}",
                            e
                        )));
                        let _ = push_result_to_redis(&pool, &submission_id, &resp).await;
                        let _ =
                            acknowledge_stream_message(&pool, &s_key, &g_name, &stream_entry_id)
                                .await;
                        drop(permit);
                        return;
                    }
                }
            }

            info!("Compilation completed for {}", submission_id);

            let (input_data, _expected_output) = testcase_parsing(payload.testcases.clone());
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
                info!("Starting sandbox execution for job: {}", sub_id_clone);
                sandbox_runner(sandbox_config)
            })
            .await
            .expect("Blocking task panicked");

            // Read fd3 output (test case answers) from output.txt
            let fd3_string = tokio::fs::read_to_string(&output_file_path)
                .await
                .unwrap_or_default();
            let fd3_lines: Vec<String> = fd3_string
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();

            // Read user stdout (console output) from user_output.txt
            let user_stdout_raw = tokio::fs::read_to_string(&user_output_file_path)
                .await
                .unwrap_or_default();
            let user_stdout = if user_stdout_raw.trim().is_empty() {
                None
            } else {
                Some(user_stdout_raw)
            };

            let response = match sandbox_result {
                Ok(result) => {
                    // The double-fork pattern encodes child signals as exit code 128+signal.
                    // Decode the actual signal from either source.
                    let effective_signal = result.signal.or_else(|| {
                        if result.exit_code > 128 {
                            Some(result.exit_code - 128)
                        } else {
                            None
                        }
                    });

                    if let Some(signal) = effective_signal {
                        let error_msg = match signal {
                            11 => "Runtime Error: Segmentation Fault (SIGSEGV)".to_string(),
                            24 => "Time Limit Exceeded (CPU Time)".to_string(),
                            9 => {
                                if result.wall_time_ms >= payload.timelimit as u128 {
                                    "Time Limit Exceeded (Wall Time)".to_string()
                                } else {
                                    "Memory Limit Exceeded".to_string()
                                }
                            }
                            8 => "Runtime Error: Floating Point Exception".to_string(),
                            6 => "Runtime Error: Aborted (SIGABRT)".to_string(),
                            31 => "Security Violation: Unauthorized System Call Blocked (SIGSYS)"
                                .to_string(),
                            _ => format!("Runtime Error: Killed by signal {}", signal),
                        };

                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_default();

                        match signal {
                            24 => ResponsePayload::time_error(0, user_stdout.clone()),
                            9 => {
                                if result.wall_time_ms >= payload.timelimit as u128 {
                                    ResponsePayload::time_error(0, user_stdout.clone())
                                } else {
                                    ResponsePayload::memory_error(0, user_stdout.clone())
                                }
                            }
                            _ => {
                                let full_err_msg = if actual_error.is_empty() {
                                    error_msg
                                } else {
                                    format!("{}\n{}", error_msg, actual_error)
                                };
                                ResponsePayload::runtime_error(
                                    Some(full_err_msg),
                                    0,
                                    user_stdout.clone(),
                                    None,
                                )
                            }
                        }
                    } else if result.exit_code != 0 {
                        let error_msg = format!("Runtime Error (Exit Code: {})", result.exit_code);
                        let actual_error = tokio::fs::read_to_string(&error_file_path)
                            .await
                            .unwrap_or_default();

                        let full_err_msg = if actual_error.is_empty() {
                            error_msg
                        } else {
                            format!("{}\n{}", error_msg, actual_error)
                        };

                        ResponsePayload::runtime_error(
                            Some(full_err_msg),
                            0,
                            user_stdout.clone(),
                            None,
                        )
                    } else {
                        // SUCCESS path: use validate_test_cases with fd3 output
                        validate_test_cases(
                            fd3_lines,
                            &payload.testcases,
                            &payload.tasktype,
                            user_stdout.clone(),
                        )
                    }
                }
                Err(e) => {
                    error!("Sandbox engine failed for {}: {:?}", submission_id, e);
                    ResponsePayload::error()
                }
            };

            // Push final result to Redis
            if let Err(e) = push_result_to_redis(&pool, &submission_id, &response).await {
                error!(
                    "Failed to push result to Redis for {}: {}",
                    submission_id, e
                );
            }

            // Acknowledge the stream message so it won't be re-delivered
            if let Err(e) =
                acknowledge_stream_message(&pool, &s_key, &g_name, &stream_entry_id).await
            {
                error!(
                    "Failed to XACK stream message {} for {}: {}",
                    stream_entry_id, submission_id, e
                );
            }

            info!(
                "Job {} completed with status: {:?}, message: {:?}",
                submission_id, response.status, response.message
            );
            drop(permit);
        });
    }
}

// --- CORE SANDBOX ENGINE ---
pub fn sandbox_runner(sandbox_config: SandboxConfiguration) -> Result<SandboxResult, SandboxError> {
    let start_time = Instant::now();
    unsafe {
        let cgroup_fs_name = std::ffi::CString::new("cgroup2").unwrap();
        let cgroup_mnt_target = std::ffi::CString::new("/sys/fs/cgroup").unwrap();

        if libc::mount(
            cgroup_fs_name.as_ptr(),
            cgroup_mnt_target.as_ptr(),
            cgroup_fs_name.as_ptr(),
            0,
            std::ptr::null(),
        ) != 0
        {
            println!(
                "[cgroup] Warning: Failed to mount fresh cgroup2 fs. It might already be writable."
            );
        }
    }

    let in_file = sandbox_config.input_file;
    let out_file = sandbox_config.output_file;
    let err_file = sandbox_config.error_output;
    let user_out_file = sandbox_config.user_output;
    let out_fd = out_file.as_raw_fd();

    // 1. Secure chroot permissions
    let mut perms = fs::metadata(&sandbox_config.root_dir)
        .unwrap()
        .permissions();
    perms.set_mode(0o777);
    let _ = fs::set_permissions(&sandbox_config.root_dir, perms);

    // 2. Prepare strict directory layout
    let old_root = sandbox_config.root_dir.join("oldroot");
    fs::create_dir_all(&old_root).expect("Failed to create oldroot dir");
    fs::create_dir_all(sandbox_config.root_dir.join("proc")).expect("Failed to create proc dir");
    fs::create_dir_all(sandbox_config.root_dir.join("tmp")).expect("Failed to create tmp dir");
    fs::create_dir_all(sandbox_config.root_dir.join("dev/shm"))
        .expect("Failed to create dev/shm dir");

    // 3. Setup Cgroups v2
    // CRITICAL: Enable controllers in parent cgroup's subtree_control
    // Without this, child cgroups cannot enforce memory/cpu/pids limits.

    // Log current state for diagnostics
    let current_subtree = fs::read_to_string("/sys/fs/cgroup/cgroup.subtree_control")
        .unwrap_or_else(|e| format!("READ_ERROR: {}", e));
    println!(
        "[cgroup] Current subtree_control: {}",
        current_subtree.trim()
    );

    match fs::write(
        "/sys/fs/cgroup/cgroup.subtree_control",
        "+memory +cpu +pids",
    ) {
        Ok(_) => println!("[cgroup] subtree_control delegation: OK"),
        Err(e) => println!(
            "[cgroup] subtree_control delegation FAILED: {} (limits may not work!)",
            e
        ),
    }

    let cgroup_path = format!("/sys/fs/cgroup/dsa_{}", sandbox_config.submissionid);
    fs::create_dir_all(&cgroup_path).map_err(|e| format!("Cgroup error: {}", e))?;

    let mem_bytes = sandbox_config.memory_limit as u64 * 1024 * 1024;
    fs::write(format!("{}/memory.max", cgroup_path), mem_bytes.to_string()).unwrap();
    let _ = fs::write(format!("{}/memory.swap.max", cgroup_path), "0");
    fs::write(format!("{}/cpu.max", cgroup_path), "100000 100000").unwrap();
    fs::write(format!("{}/pids.max", cgroup_path), "512").unwrap();

    // Verify memory.max was actually set
    let actual_mem = fs::read_to_string(format!("{}/memory.max", cgroup_path))
        .unwrap_or_else(|e| format!("READ_ERROR: {}", e));
    println!(
        "[cgroup] memory.max set to: {} (requested: {})",
        actual_mem.trim(),
        mem_bytes
    );

    // Check which controllers are active in the child cgroup
    let child_controllers = fs::read_to_string(format!("{}/cgroup.controllers", cgroup_path))
        .unwrap_or_else(|e| format!("READ_ERROR: {}", e));
    println!(
        "[cgroup] Child cgroup controllers: {}",
        child_controllers.trim()
    );

    // 4. Pre-calculate CStrings for pure Async-Signal-Safety
    // THE FIX: We split Read-Only system directories and Writable Device files into two separate lists.
    let readonly_dirs = [
        "/bin",
        "/lib",
        "/lib64",
        "/usr",
        "/etc",
        "/root/.bun",
        "/app/sandbox_bin",
    ];
    let mut ro_mounts_c: Vec<(CString, CString)> = Vec::new();

    for dir in &readonly_dirs {
        let host_path = std::path::Path::new(dir);
        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dir.trim_start_matches('/'));
            fs::create_dir_all(&target_path).unwrap_or_default();
            ro_mounts_c.push((
                CString::new(*dir).unwrap(),
                CString::new(target_path.to_str().unwrap()).unwrap(),
            ));
        }
    }

    let device_files = ["/dev/null", "/dev/urandom", "/dev/zero"];
    let mut dev_mounts_c: Vec<(CString, CString)> = Vec::new();
    for dev in &device_files {
        let host_path = std::path::Path::new(dev);
        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dev.trim_start_matches('/'));
            fs::create_dir_all(target_path.parent().unwrap()).unwrap_or_default();
            fs::File::create(&target_path).expect("Failed to create device file");
            dev_mounts_c.push((
                CString::new(*dev).unwrap(),
                CString::new(target_path.to_str().unwrap()).unwrap(),
            ));
        }
    }

    let proc_tgt_c = CString::new(sandbox_config.root_dir.join("proc").to_str().unwrap()).unwrap();
    let tmp_tgt_c = CString::new(sandbox_config.root_dir.join("tmp").to_str().unwrap()).unwrap();
    let dev_shm_tgt_c =
        CString::new(sandbox_config.root_dir.join("dev/shm").to_str().unwrap()).unwrap();
    let root_dir_c = CString::new(sandbox_config.root_dir.to_str().unwrap()).unwrap();
    let cgroup_procs_file_c = CString::new(format!("{}/cgroup.procs", cgroup_path)).unwrap();

    let exe_path = if sandbox_config.run_cmd_exe.starts_with("./") {
        sandbox_config.run_cmd_exe.replacen(".", "", 1)
    } else {
        sandbox_config.run_cmd_exe.clone()
    };

    let mut cmd = Command::new(&exe_path);
    cmd.args(&sandbox_config.run_cmd_args);
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

    let time_limit = sandbox_config.time_limit as u64;
    let bpf_instructions = build_strict_seccomp_profile();

    // --- 5. The Hardened Pre-Exec Hook ---
    unsafe {
        cmd.pre_exec(move || {
            let write_sys = |path: &CString, data: &[u8]| -> bool {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY);
                if fd < 0 {
                    return false;
                }
                let res = libc::write(fd, data.as_ptr() as *const libc::c_void, data.len());
                libc::close(fd);
                res == data.len() as isize
            };

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

            if !write_sys(&cgroup_procs_file_c, &pid_buf[..i + 1]) {
                libc::_exit(101);
            }

            let flags = CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWIPC
                | CloneFlags::CLONE_NEWNET
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWNS;
            if unshare(flags).is_err() {
                libc::_exit(102);
            }

            let child_pid = libc::fork();
            if child_pid < 0 {
                libc::_exit(103);
            }
            if child_pid > 0 {
                let mut status = 0;
                if libc::waitpid(child_pid, &mut status, 0) < 0 {
                    libc::_exit(104);
                }
                if libc::WIFEXITED(status) {
                    libc::_exit(libc::WEXITSTATUS(status));
                } else if libc::WIFSIGNALED(status) {
                    libc::_exit(128 + libc::WTERMSIG(status));
                }
                libc::_exit(1);
            }

            libc::mount(
                b"/\0".as_ptr() as *const i8,
                b"/\0".as_ptr() as *const i8,
                b"bind\0".as_ptr() as *const i8,
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            );
            if libc::mount(
                b"none\0".as_ptr() as *const i8,
                b"/\0".as_ptr() as *const i8,
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) != 0
            {
                libc::_exit(105);
            }

            if libc::mount(
                root_dir_c.as_ptr(),
                root_dir_c.as_ptr(),
                b"bind\0".as_ptr() as *const i8,
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            ) != 0
            {
                libc::_exit(106);
            }
            if libc::mount(
                b"none\0".as_ptr() as *const i8,
                root_dir_c.as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) != 0
            {
                libc::_exit(121);
            }

            // THE FIX: System Mounts (Explicitly add NOSUID and NODEV, drop MS_REC on remount)
            for (src, tgt) in &ro_mounts_c {
                if libc::mount(
                    src.as_ptr(),
                    tgt.as_ptr(),
                    b"bind\0".as_ptr() as *const i8,
                    libc::MS_BIND,
                    std::ptr::null(),
                ) != 0
                {
                    libc::_exit(107);
                }

                let remount_flags = libc::MS_BIND
                    | libc::MS_REMOUNT
                    | libc::MS_RDONLY
                    | libc::MS_NOSUID
                    | libc::MS_NODEV;
                if libc::mount(
                    std::ptr::null(),
                    tgt.as_ptr(),
                    std::ptr::null(),
                    remount_flags,
                    std::ptr::null(),
                ) != 0
                {
                    libc::_exit(108);
                }
            }

            // THE FIX: Device Mounts (Bind only, DO NOT remount read-only, DO NOT use NODEV)
            for (src, tgt) in &dev_mounts_c {
                if libc::mount(
                    src.as_ptr(),
                    tgt.as_ptr(),
                    b"bind\0".as_ptr() as *const i8,
                    libc::MS_BIND,
                    std::ptr::null(),
                ) != 0
                {
                    libc::_exit(132);
                }
            }

            let secure_tmpfs_flags = libc::MS_NOEXEC | libc::MS_NOSUID | libc::MS_NODEV;
            if libc::mount(
                b"proc\0".as_ptr() as *const i8,
                proc_tgt_c.as_ptr(),
                b"proc\0".as_ptr() as *const i8,
                secure_tmpfs_flags,
                std::ptr::null(),
            ) != 0
            {
                libc::_exit(109);
            }
            if libc::mount(
                b"tmpfs\0".as_ptr() as *const i8,
                tmp_tgt_c.as_ptr(),
                b"tmpfs\0".as_ptr() as *const i8,
                secure_tmpfs_flags,
                b"size=128m,mode=1777\0".as_ptr() as *const libc::c_void,
            ) != 0
            {
                libc::_exit(110);
            }
            if libc::mount(
                b"tmpfs\0".as_ptr() as *const i8,
                dev_shm_tgt_c.as_ptr(),
                b"tmpfs\0".as_ptr() as *const i8,
                secure_tmpfs_flags,
                b"size=64m,mode=1777\0".as_ptr() as *const libc::c_void,
            ) != 0
            {
                libc::_exit(112);
            }

            if libc::chdir(root_dir_c.as_ptr()) != 0 {
                libc::_exit(113);
            }
            if libc::syscall(libc::SYS_pivot_root, b".\0".as_ptr(), b"oldroot\0".as_ptr()) != 0 {
                libc::_exit(114);
            }
            if libc::chdir(b"/\0".as_ptr() as *const i8) != 0 {
                libc::_exit(115);
            }
            if libc::umount2(b"/oldroot\0".as_ptr() as *const i8, libc::MNT_DETACH) != 0 {
                libc::_exit(116);
            }

            if libc::setgroups(0, std::ptr::null()) != 0 {
                libc::_exit(117);
            }
            if libc::setgid(65534) != 0 {
                libc::_exit(118);
            }
            if libc::setuid(65534) != 0 {
                libc::_exit(119);
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
                libc::_exit(120);
            }

            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                libc::_exit(122);
            }
            let fprog = libc::sock_fprog {
                len: bpf_instructions.len() as u16,
                filter: bpf_instructions.as_ptr() as *mut libc::sock_filter,
            };
            if libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &fprog) != 0 {
                libc::_exit(123);
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
