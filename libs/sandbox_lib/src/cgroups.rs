use std::fs::{self};
use std::sync::Once;
use tracing::{debug, error, info};

static CGROUP_INIT: Once = Once::new();

pub fn initialize_global_cgroups_once() {
    CGROUP_INIT.call_once(|| {
        // info!("[init] performing one-time global cgroup and rootfs initialization...");
        // info!("[init] Copying isolated rootfs to RAM (/dev/shm/rootfs) for user-namespace compatibility...");

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
                info!("[cgroup] cgroup2 fs already mounted or mount failed (normal in docker).");
            }
        }

        let init_cgroup = "/sys/fs/cgroup/init";
        fs::create_dir_all(init_cgroup).unwrap_or_default();

        let my_pid = std::process::id();
        if let Err(e) = fs::write(format!("{}/cgroup.procs", init_cgroup), my_pid.to_string()) {
            error!(
                "[cgroup] warning: failed to move executor to init cgroup: {}",
                e
            );
        }

        match fs::write(
            "/sys/fs/cgroup/cgroup.subtree_control",
            "+memory +cpu +pids",
        ) {
            Ok(_) => info!("[cgroup] global subtree_control delegation: ok"),
            Err(e) => error!("[cgroup] global subtree_control delegation failed: {}", e),
        }
    });
}

pub struct CgroupGuard {
    pub path: String,
}

impl Drop for CgroupGuard {
    fn drop(&mut self) {
        let kill_file = format!("{}/cgroup.kill", self.path);
        let procs_file = format!("{}/cgroup.events", self.path);
        let _ = std::fs::write(&kill_file, "1");

        let mut retries = 200;

        while retries > 0 {
            if let Ok(events) = std::fs::read_to_string(&procs_file) {
                if events.contains("populated 0") {
                    break;
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(20));
            retries -= 1;
        }
        if retries == 0 {
            tracing::warn!("cgroups {} was still populated after 100ms wait", self.path);
        }
        if let Err(e) = std::fs::remove_dir(&self.path) {
            error!("warning failed to remove cgroups {} : {}", self.path, e);
        } else {
            debug!("Succesfully cleaned up cgroups file {}", self.path);
        }
    }
}
