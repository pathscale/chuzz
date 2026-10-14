//! Process-wide limits installed before workers or page data exist.

use std::sync::OnceLock;

static HEAP_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static HARDENED: OnceLock<Result<(), String>> = OnceLock::new();

pub fn startup() {
    if let Err(reason) = ensure() {
        eprintln!("chuzz: platform-hardening: {reason}");
        std::process::exit(1);
    }
}

pub fn ensure() -> Result<(), String> {
    HARDENED.get_or_init(install).clone()
}

pub fn core_limit_zero() -> bool {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit { rlim_cur: 1, rlim_max: 1 };
        // The kernel writes exactly one initialized rlimit structure.
        (unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) == 0 })
            && limit.rlim_cur == 0
            && limit.rlim_max == 0
    }
    #[cfg(not(unix))]
    false
}

pub fn heap_reports_disabled() -> bool {
    HEAP_DISABLED.load(std::sync::atomic::Ordering::Acquire)
}

fn install() -> Result<(), String> {
    #[cfg(unix)]
    {
        let value = u64::from(crate::sandbox::defect("core-limit-zero"));
        let limit = libc::rlimit { rlim_cur: value as libc::rlim_t, rlim_max: value as libc::rlim_t };
        // Lower both limits. Unprivileged code cannot raise the hard limit.
        if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } != 0 {
            return Err(format!("core-limit-zero: {}", std::io::Error::last_os_error()));
        }
    }
    #[cfg(target_os = "macos")]
    {
        // task_info.h: disable corpse forks, which preserve the task's heap
        // for postmortem inspection independently of RLIMIT_CORE.
        const TASK_CORPSE_FORKING_DISABLED_MEM_DIAG: u32 = 1;
        let result = if crate::sandbox::defect("heap-reports-disabled") { 0 } else { unsafe {
            task_set_corpse_forking_behavior(
                mach_task_self_, TASK_CORPSE_FORKING_DISABLED_MEM_DIAG,
            )
        } };
        if result != 0 {
            return Err(format!("heap-reports-disabled: task policy returned {result}"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Also blocks ptrace and the kernel's alternate dump paths.
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err(format!("heap-reports-disabled: {}", std::io::Error::last_os_error()));
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    HEAP_DISABLED.store(!crate::sandbox::defect("heap-reports-disabled"), std::sync::atomic::Ordering::Release);
    // A panic payload can contain page bytes, credentials or heap values.
    std::panic::set_hook(Box::new(|_| eprintln!("chuzz: internal failure")));
    Ok(())
}

#[cfg(target_os = "macos")]
#[link(name = "System")]
unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_set_corpse_forking_behavior(task: u32, behavior: u32) -> libc::c_int;
}
