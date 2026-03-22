use crate::cgroups::{CgroupGuard, initialize_global_cgroups_once};
use crate::seccomp::build_strict_seccomp_profile;
use nix::sched::{CloneFlags, unshare};
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{error, info};
use types_lib::{SandboxConfiguration, SandboxError, SandboxResult};

pub async fn sandbox_runner(
    sandbox_config: SandboxConfiguration,
) -> Result<SandboxResult, SandboxError> {
    let start_time = Instant::now();
    initialize_global_cgroups_once();

    let in_file = sandbox_config.input_file;
    let out_file = sandbox_config.output_file;
    let err_file = sandbox_config.error_output;
    let user_out_file = sandbox_config.user_output;
    let out_fd = out_file.as_raw_fd();

    let mut perms = tokio::fs::metadata(&sandbox_config.root_dir)
        .await
        .unwrap()
        .permissions();
    perms.set_mode(0o777);
    let _ = tokio::fs::set_permissions(&sandbox_config.root_dir, perms).await;

    let old_root = sandbox_config.root_dir.join("oldroot");

    tokio::fs::create_dir_all(&old_root)
        .await
        .expect("failed to create oldroot dir");
    tokio::fs::create_dir_all(sandbox_config.root_dir.join("proc"))
        .await
        .expect("failed to create proc dir");
    tokio::fs::create_dir_all(sandbox_config.root_dir.join("tmp"))
        .await
        .expect("failed to create tmp dir");
    tokio::fs::create_dir_all(sandbox_config.root_dir.join("dev/shm"))
        .await
        .expect("failed to create dev/shm dir");

    let _ = std::os::unix::fs::symlink("/proc/self/fd", sandbox_config.root_dir.join("dev/fd"));
    let init_cgroup = "/sys/fs/cgroup/init";

    tokio::fs::create_dir_all(init_cgroup)
        .await
        .unwrap_or_default();

    let my_pid = std::process::id();
    if let Err(e) =
        tokio::fs::write(format!("{}/cgroup.procs", init_cgroup), my_pid.to_string()).await
    {
        error!(
            "[cgroup] warning: failed to move executor to init cgroup: {}",
            e
        );
    }

    let _ = tokio::fs::read_to_string("/sys/fs/cgroup/cgroup.subtree_control")
        .await
        .unwrap_or_else(|e| format!("read_error: {}", e));

    match tokio::fs::write(
        "/sys/fs/cgroup/cgroup.subtree_control",
        "+memory +cpu +pids",
    )
    .await
    {
        Ok(_) => info!("[cgroup] subtree_control delegation: ok"),
        Err(e) => error!(
            "[cgroup] subtree_control delegation failed: {} (limits may not work!)",
            e
        ),
    }

    let cgroup_path = format!("/sys/fs/cgroup/dsa_{}", sandbox_config.submissionid);

    tokio::fs::create_dir_all(&cgroup_path)
        .await
        .map_err(|e| format!("cgroup error: {}", e))?;

    let _cgroup_guard = CgroupGuard {
        path: cgroup_path.clone(),
    };

    let mem_bytes = sandbox_config.memory_limit as u64 * 1024 * 1024;
    tokio::fs::write(format!("{}/memory.max", cgroup_path), mem_bytes.to_string())
        .await
        .unwrap();
    let _ = tokio::fs::write(format!("{}/memory.swap.max", cgroup_path), "0").await;
    tokio::fs::write(format!("{}/cpu.max", cgroup_path), "100000 100000")
        .await
        .unwrap();
    tokio::fs::write(format!("{}/pids.max", cgroup_path), "64")
        .await
        .unwrap();

    let _ = tokio::fs::read_to_string(format!("{}/memory.max", cgroup_path))
        .await
        .unwrap_or_else(|e| format!("read_error: {}", e));

    let _child_controllers =
        tokio::fs::read_to_string(format!("{}/cgroup.controllers", cgroup_path))
            .await
            .unwrap_or_else(|e| format!("read_error: {}", e));

    let readonly_dirs = ["/bin", "/lib", "/lib64", "/usr", "/etc"];
    let mut ro_mounts_c: Vec<(CString, CString)> = Vec::new();

    for dir in &readonly_dirs {
        // Pointing to the tmpfs layer we created during initialization
        let host_path_str = format!("/opt/sandbox_rootfs{}", dir);
        let host_path = std::path::Path::new(&host_path_str);

        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dir.trim_start_matches('/'));
            tokio::fs::create_dir_all(&target_path)
                .await
                .unwrap_or_default();

            ro_mounts_c.push((
                CString::new(host_path_str).unwrap(),
                CString::new(target_path.to_str().unwrap()).unwrap(),
            ));
        } else {
            error!(
                "Critical: Missing expected ssd rootfs path: {}",
                host_path_str
            );
        }
    }

    let device_files = ["/dev/null", "/dev/urandom", "/dev/zero"];
    let mut dev_mounts_c: Vec<(CString, CString)> = Vec::new();
    for dev in &device_files {
        let host_path = std::path::Path::new(dev);
        if host_path.exists() {
            let target_path = sandbox_config.root_dir.join(dev.trim_start_matches('/'));
            tokio::fs::create_dir_all(target_path.parent().unwrap())
                .await
                .unwrap_or_default();
            tokio::fs::File::create(&target_path)
                .await
                .expect("failed to create device file");
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

    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let uid_map = format!("0 {} 1\n", host_uid).into_bytes();
    let gid_map = format!("0 {} 1\n", host_gid).into_bytes();
    let proc_uid_map_c = CString::new("/proc/self/uid_map").unwrap();
    let proc_setgroups_c = CString::new("/proc/self/setgroups").unwrap();
    let proc_gid_map_c = CString::new("/proc/self/gid_map").unwrap();

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
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    );
    cmd.env("HOME", "/tmp");
    cmd.env("TMPDIR", "/tmp");
    cmd.env("UV_USE_IO_URING", "0");
    cmd.stdin(Stdio::from(in_file));
    cmd.stdout(Stdio::from(user_out_file));
    cmd.stderr(Stdio::from(err_file));

    let time_limit = sandbox_config.time_limit as u64 / 1000;
    let bpf_instructions = build_strict_seccomp_profile();

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
                | CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWUSER;

            if unshare(flags).is_err() {
                libc::_exit(102);
            }

            if !write_sys(&proc_uid_map_c, &uid_map) {
                libc::_exit(150);
            }
            if !write_sys(&proc_setgroups_c, b"deny\n") {
                libc::_exit(151);
            }
            if !write_sys(&proc_gid_map_c, &gid_map) {
                libc::_exit(152);
            }

            let child_pid = libc::fork();
            if child_pid < 0 {
                libc::_exit(103);
            }
            if child_pid > 0 {
                for fd in 3..1024 {
                    libc::close(fd);
                }

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
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                libc::_exit(130);
            }
            // if libc::getppid() == 0 || libc::getppid() == 1 {
            //     libc::_exit(131);
            // }
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
            libc::rmdir(b"/oldroot\0".as_ptr() as *const i8);
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

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {}", e))?;

    let timeout_duration =
        std::time::Duration::from_millis(sandbox_config.time_limit as u64 + 1000);

    let status = match timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("Wait error: {}", e).into()),
        Err(_) => {
            // let cpu_stat_path = format!("{}/cpu.stat", cgroup_path);
            // let mut cpu_usage_ms: u128 = 0;
            // if let Ok(stat_data) = tokio::fs::read_to_string(&cpu_stat_path).await {
            //     for line in stat_data.lines() {
            //         if let Some(value) = line.strip_prefix("usage_usec ") {
            //             if let Ok(usage) = value.parse::<u128>() {
            //                 cpu_usage_ms = usage / 1000;
            //             }
            //         }
            //     }
            // }
            // let expected_ms = sandbox_config.time_limit as u128 / 2;
            // if cpu_usage_ms < expected_ms {
            // warn!(
            // "Sleeping process detected: cpu_usage_ms={} expected_ms={}",
            // cpu_usage_ms, expected_ms
            // );
            // }
            let kill_file = format!("{}/cgroup.kill", cgroup_path);
            if let Err(e) = tokio::fs::write(&kill_file, "1").await {
                tracing::error!("failed to write cgroup.kill: {}", e);
            };
            let _ = child.kill().await;
            // // let _ = child.wait().await;
            std::os::unix::process::ExitStatusExt::from_raw(9)
        }
    };
    let events_path = format!("{}/memory.events", cgroup_path);
    let events_data = tokio::fs::read_to_string(&events_path)
        .await
        .unwrap_or_default();

    let mut is_oom = false;
    for line in events_data.lines() {
        if line.starts_with("oom_kill ") || line.starts_with("oom ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 {
                if let Ok(count) = parts[1].parse::<u32>() {
                    if count > 0 {
                        is_oom = true;
                    }
                }
            }
        }
    }

    let result = SandboxResult {
        exit_code: status.code().unwrap_or(-1),
        signal: status.signal(),
        wall_time_ms: start_time.elapsed().as_millis(),
        is_oom,
    };

    Ok(result)
}
