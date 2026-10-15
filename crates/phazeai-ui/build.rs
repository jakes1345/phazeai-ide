fn main() {
    // The Floem view tree is deep enough to overflow the default main-thread stack.
    // Linux raises RLIMIT_STACK at startup (src/bin/phazeai-ui.rs); Windows has no
    // runtime equivalent and its default is only 1 MiB, so reserve a larger stack in
    // the executable itself. This reserves address space; memory is committed on demand.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") {
        let stack_bytes = 256 * 1024 * 1024;
        if target.contains("msvc") {
            println!("cargo:rustc-link-arg-bin=phazeai-ui=/STACK:{stack_bytes}");
        } else {
            println!("cargo:rustc-link-arg-bin=phazeai-ui=-Wl,--stack,{stack_bytes}");
        }
    }
}
