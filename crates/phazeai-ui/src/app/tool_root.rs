//! The agent's file tools follow the folder the IDE has open.
//!
//! `phazeai_core::tools::sandbox` keeps one workspace root for the whole
//! process: the file tools refuse paths outside it, relative tool paths are
//! read from it, and the shell starts there. The IDE sets it at startup; this
//! keeps it on the workspace after Open Folder (or anything else that changes
//! the workspace), as the privacy policy and code search already are.

use floem::reactive::{create_effect, RwSignal, SignalGet};
use std::path::PathBuf;

/// Install the effect that keeps the tools' root on `workspace_root`.
pub(super) fn install(workspace_root: RwSignal<PathBuf>) {
    follow(workspace_root, |root| {
        phazeai_core::tools::sandbox::set_workspace_root(Some(root))
    });
}

/// Call `apply` with the workspace now and again each time it changes.
fn follow(workspace_root: RwSignal<PathBuf>, apply: impl Fn(PathBuf) + 'static) {
    create_effect(move |_| apply(workspace_root.get()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::reactive::{create_rw_signal, SignalUpdate};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn the_tools_root_is_set_at_once_and_again_after_open_folder() {
        let seen = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
        let root = create_rw_signal(PathBuf::from("/projects/first"));
        let log = seen.clone();
        follow(root, move |p| log.borrow_mut().push(p));
        root.set(PathBuf::from("/projects/second"));
        assert_eq!(
            *seen.borrow(),
            [
                PathBuf::from("/projects/first"),
                PathBuf::from("/projects/second")
            ]
        );
    }
}
