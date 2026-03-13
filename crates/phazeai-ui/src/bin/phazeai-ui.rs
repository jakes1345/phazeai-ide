fn main() {
    // The Floem view tree (~6000 lines of nested panels) exceeds the default
    // OS main thread stack. Set RLIMIT_STACK to unlimited before constructing
    // any views. This is safe on Linux — the kernel grows the stack on demand
    // up to the new limit. Winit's main-thread requirement is fully preserved
    // since we stay on the same OS thread.
    #[cfg(target_os = "linux")]
    {
        unsafe {
            let unlimited = libc::rlimit {
                rlim_cur: libc::RLIM_INFINITY,
                rlim_max: libc::RLIM_INFINITY,
            };
            libc::setrlimit(libc::RLIMIT_STACK, &unlimited);
        }
    }
    phazeai_ui::launch_phaze_ide();
}
