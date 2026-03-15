use floem::reactive::{Memo, RwSignal, SignalGet};

/// Safely read a reactive signal that may have been disposed (e.g. from a
/// removed `dyn_stack` item).  Returns `default` if the signal's scope is gone.
///
/// Floem's `SignalGet::get()` panics when the signal's scope has been disposed,
/// which happens when a `dyn_stack` item is removed but its style/label closures
/// fire one last time during the same event cycle.  This wraps the read in
/// `catch_unwind` so those stale reads return a safe default instead of crashing.
pub fn safe_get<T: Clone + 'static>(sig: RwSignal<T>, default: T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sig.get()))
        .unwrap_or(default)
}

/// Same as [`safe_get`] but for derived memos (`Memo<T>`).
pub fn safe_get_memo<T: Clone + 'static>(memo: Memo<T>, default: T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| memo.get()))
        .unwrap_or(default)
}
