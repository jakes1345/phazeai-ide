fn main() {
    // The Floem view tree in app.rs is deeply nested (~6000 lines of stacked panels).
    // The default OS stack (8 MB) overflows during view construction.
    // stacker::grow expands the stack in-place without spawning a new thread,
    // satisfying winit's requirement that the event loop runs on the main OS thread.
    stacker::grow(64 * 1024 * 1024, phazeai_ui::launch_phaze_ide);
}
