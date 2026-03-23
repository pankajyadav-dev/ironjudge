use crate::cgroups::{CgroupGuard, initialize_global_cgroups_once};
// use crate::seccomp::build_strict_seccomp_profile;
// use nix::sched::{CloneFlags, unshare};
// use std::ffi::CString;
// use std::os::fd::AsRawFd;
// use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
// use std::time::Instant;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{error, info};
use types_lib::{CompileResult, CompileSandboxConfig, SandboxError};

pub async fn compile_sandbox_runner(
    sandbox_config: CompileSandboxConfig,
) -> Result<CompileResult, SandboxError> {
    // let start_time = Instant::now();
    initialize_global_cgroups_once();

    // 1. Setup Parent Cgroup
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

    // 2. Setup Sandbox Cgroup for Compilation
    let cgroup_path = format!("/sys/fs/cgroup/compile_{}", sandbox_config.submissionid);
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
    tokio::fs::write(format!("{}/pids.max", cgroup_path), "128")
        .await
        .unwrap();

    // 3. Prepare Command
    let mut cmd = Command::new(&sandbox_config.run_cmd_exe);
    cmd.args(&sandbox_config.run_cmd_args);
    cmd.current_dir(&sandbox_config.root_dir);
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::piped());

    // 4. SPAWN (Don't wait yet) so we can assign the cgroup
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    if let Some(pid) = child.id() {
        if let Err(e) =
            tokio::fs::write(format!("{}/cgroup.procs", cgroup_path), pid.to_string()).await
        {
            error!("Failed to move child {} into cgroup: {}", pid, e);
            let _ = child.kill().await; // Abort if we can't secure it
            return Err("Failed to apply cgroup limits".into());
        }
    }

    // 5. Wait for the process with a timeout
    let timeout_duration =
        std::time::Duration::from_millis(sandbox_config.time_limit as u64 + 1000);

    let (status, error_output) = match timeout(timeout_duration, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();

            let combined_error = if !stderr_str.trim().is_empty() {
                stderr_str
            } else {
                stdout_str
            };

            (output.status, combined_error)
        }
        Ok(Err(e)) => return Err(format!("Wait error: {}", e).into()),
        Err(_) => {
            // Timeout occurred! Kill everything in the cgroup.
            let kill_file = format!("{}/cgroup.kill", cgroup_path);
            if let Err(e) = tokio::fs::write(&kill_file, "1").await {
                error!("failed to write cgroup.kill: {}", e);
            }
            // Wait for the child to exit to reap the zombie process
            // Handle by the tokio run time  drop the child process when timeout function completely
            // let _ = child.wait().await;

            (
                ExitStatusExt::from_raw(9),
                "Compile Timeout Error".to_string(),
            )
        }
    };

    // 7. Return Results
    let result = CompileResult {
        success: status.success(),
        error: error_output,
    };

    Ok(result)
}
