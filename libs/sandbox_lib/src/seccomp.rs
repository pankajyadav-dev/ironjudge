use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};
use std::convert::TryInto;

pub fn build_strict_seccomp_profile() -> Vec<libc::sock_filter> {
    let mut rules = std::collections::BTreeMap::new();

    let allowed_syscalls = vec![
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_brk,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_mincore,
        libc::SYS_membarrier,
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_openat,
        libc::SYS_close,
        libc::SYS_lseek,
        libc::SYS_ioctl,
        libc::SYS_fcntl,
        libc::SYS_dup,
        libc::SYS_dup2,
        libc::SYS_dup3,
        libc::SYS_pipe2,
        libc::SYS_clone,
        libc::SYS_clone3,
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
        libc::SYS_execve,
        libc::SYS_memfd_create,
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
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_arch_prctl,
        libc::SYS_rseq,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_tgkill,
        libc::SYS_wait4,
        libc::SYS_gettimeofday,
        libc::SYS_clock_gettime,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_getrandom,
        libc::SYS_prlimit64,
        libc::SYS_getrlimit,
        libc::SYS_getrusage,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_timerfd_create,
        libc::SYS_timerfd_settime,
        libc::SYS_timerfd_gettime,
        libc::SYS_signalfd4,
    ];

    for syscall in allowed_syscalls {
        rules.insert(syscall, vec![]);
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::ENOSYS as u32),
        SeccompAction::Allow,
        std::env::consts::ARCH.try_into().unwrap(),
    )
    .expect("failed to build seccomp filter structure");

    let bpf_prog: BpfProgram = filter.try_into().expect("failed to compile to bpf");

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
