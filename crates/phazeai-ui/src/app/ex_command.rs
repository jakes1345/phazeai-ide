//! Save and quit commands of the Vim ex bar (`:w`, `:wq`, `:q!`, `:wqa`, ...).
//!
//! What a command means is decided by [`plan_ex_command`], a pure function over a snapshot of
//! the open tabs. [`run_ex_command`] carries a plan out through the editor panes' handles and
//! says what to show and whether to quit; [`finish_write`] turns the real save results into that
//! answer, so a "Saved" message only exists after a write and a quit only after every write.
//! None of it touches Floem signals or windows.

use std::rc::Rc;

/// One open tab, as the ex bar sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    /// Name shown on the tab, used in messages.
    pub name: String,
    /// Has edits that are not on disk.
    pub dirty: bool,
    /// The tab `:w` acts on: the active tab of the main editor pane.
    pub active: bool,
}

/// What came of writing one tab to disk.
#[derive(Debug)]
pub enum SaveOutcome {
    Saved,
    /// The tab's editor has not been built yet, so there was nothing to write.
    NotLoaded,
    Failed(std::io::Error),
}

/// What an ex command should do, decided before anything is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExPlan {
    /// Not a save/quit command; the ex bar handles it as before (`:e`, `:cd`, unknown).
    NotHandled,
    /// Show this message and change nothing.
    Notify(String),
    /// Quit now; nothing needs saving first.
    Quit,
    /// Quit and throw away these tabs' unsaved work (indices into the tab snapshot): `:q!`.
    /// Each one's crash-recovery shadow goes too, or the next launch would announce the
    /// edits that were just discarded on purpose.
    QuitDiscard { tabs: Vec<usize> },
    /// Write these tabs (indices into the tab snapshot), then do `then`.
    Write { tabs: Vec<usize>, then: AfterWrite },
}

/// What follows once every write in an [`ExPlan::Write`] has succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterWrite {
    /// Stay open and say what was saved.
    Stay,
    /// Quit.
    Quit,
    /// Stay open: the write was wanted, but quitting would drop unsaved work elsewhere.
    StayBecause(String),
}

const NO_FILE: &str = "No file to save";

/// Decide what `cmd` (the text typed after `:`) does given the open `tabs`.
pub fn plan_ex_command(cmd: &str, tabs: &[TabInfo]) -> ExPlan {
    let active = tabs.iter().position(|t| t.active);
    let dirty: Vec<usize> = tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.dirty)
        .map(|(i, _)| i)
        .collect();

    match cmd.trim() {
        "w" | "write" => match active {
            Some(i) => ExPlan::Write {
                tabs: vec![i],
                then: AfterWrite::Stay,
            },
            None => ExPlan::Notify(NO_FILE.to_string()),
        },
        // Vim's `:x` writes only a modified buffer; here both always write, like Ctrl+S.
        "wq" | "x" | "xit" | "exit" => match active {
            Some(i) => {
                let others: Vec<usize> = dirty.iter().copied().filter(|&d| d != i).collect();
                let then = if others.is_empty() {
                    AfterWrite::Quit
                } else {
                    AfterWrite::StayBecause(format!(
                        "{} (:wqa saves all, :q! discards)",
                        unsaved_summary(tabs, &others)
                    ))
                };
                ExPlan::Write {
                    tabs: vec![i],
                    then,
                }
            }
            None => ExPlan::Notify(NO_FILE.to_string()),
        },
        "q" | "quit" | "qa" | "qall" | "quitall" => {
            if dirty.is_empty() {
                ExPlan::Quit
            } else {
                ExPlan::Notify(format!(
                    "No write since last change (add ! to override): {}",
                    unsaved_summary(tabs, &dirty)
                ))
            }
        }
        "q!" | "quit!" | "qa!" | "qall!" | "quitall!" => ExPlan::QuitDiscard { tabs: dirty },
        "wqa" | "wqall" | "xa" | "xall" | "wall" => {
            if dirty.is_empty() {
                ExPlan::Quit
            } else {
                ExPlan::Write {
                    tabs: dirty,
                    then: AfterWrite::Quit,
                }
            }
        }
        _ => ExPlan::NotHandled,
    }
}

/// "1 tab has unsaved changes (a.rs)" / "5 tabs have unsaved changes (a.rs, b.rs, c.rs and 2 more)"
fn unsaved_summary(tabs: &[TabInfo], which: &[usize]) -> String {
    const SHOWN: usize = 3;
    let names: Vec<&str> = which.iter().map(|&i| tabs[i].name.as_str()).collect();
    let listed = names[..names.len().min(SHOWN)].join(", ");
    let more = names.len().saturating_sub(SHOWN);
    let detail = if more > 0 {
        format!("{listed} and {more} more")
    } else {
        listed
    };
    if names.len() == 1 {
        format!("1 tab has unsaved changes ({detail})")
    } else {
        format!("{} tabs have unsaved changes ({detail})", names.len())
    }
}

/// What to show and whether to quit once the writes of an [`ExPlan::Write`] are done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    pub toast: Option<String>,
    pub quit: bool,
}

/// Turn the real outcome of each write (tab name, result) into the toast and the quit decision.
/// Quits only when `then` asks for it and every write succeeded.
pub fn finish_write(then: &AfterWrite, results: &[(String, SaveOutcome)]) -> WriteResult {
    let failures: Vec<(&str, String)> = results
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            SaveOutcome::Saved => None,
            SaveOutcome::NotLoaded => {
                Some((name.as_str(), "its editor is not loaded yet".to_string()))
            }
            SaveOutcome::Failed(e) => Some((name.as_str(), e.to_string())),
        })
        .collect();

    if let Some((name, reason)) = failures.first() {
        let mut msg = if results.len() == 1 {
            format!("Couldn't save {name}: {reason}")
        } else {
            let saved = results.len() - failures.len();
            let mut m = format!(
                "Saved {saved} of {} tabs; couldn't save {name}: {reason}",
                results.len()
            );
            if failures.len() > 1 {
                m.push_str(&format!(" (and {} more failed)", failures.len() - 1));
            }
            m
        };
        // Only when a quit was asked for: a plain `:w` never promised one.
        if *then != AfterWrite::Stay {
            msg.push_str("; not quitting");
        }
        return WriteResult {
            toast: Some(msg),
            quit: false,
        };
    }

    let saved = match results {
        [] => None,
        [(name, _)] => Some(format!("Saved {name}")),
        many => Some(format!("Saved {} tabs", many.len())),
    };
    match then {
        AfterWrite::Quit => WriteResult {
            toast: None,
            quit: true,
        },
        AfterWrite::Stay => WriteResult {
            toast: saved,
            quit: false,
        },
        AfterWrite::StayBecause(why) => WriteResult {
            toast: Some(match saved {
                Some(s) => format!("{s}; not quitting: {why}"),
                None => format!("Not quitting: {why}"),
            }),
            quit: false,
        },
    }
}

/// One editor pane (the main editor or a split), registered by `editor_panel` so the ex bar can
/// read its tabs and save them through the same code as Ctrl+S.
#[derive(Clone)]
pub struct EditorPaneHandle {
    /// The main editor pane. `:w` and `:wq` act on its active tab; splits only count toward
    /// "any unsaved changes" and `:wqa`.
    pub primary: bool,
    /// Current tabs, in tab-bar order.
    pub tabs: Rc<dyn Fn() -> Vec<TabInfo>>,
    /// Write tab `index` (position in `tabs`) to disk; reports the real result.
    pub save_tab: Rc<dyn Fn(usize) -> SaveOutcome>,
    /// Drop tab `index`'s crash-recovery shadow, because its unsaved edits are being discarded.
    pub discard_tab: Rc<dyn Fn(usize)>,
}

/// What the ex bar should do after a handled command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExRun {
    pub toast: Option<String>,
    pub quit: bool,
}

/// Run a save/quit command against the open editor panes. `None` means `cmd` is not one of
/// them (the ex bar handles it as before). Writes and discards happen here, in plan order; a
/// failed write does not stop the remaining ones.
pub fn run_ex_command(cmd: &str, panes: &[EditorPaneHandle]) -> Option<ExRun> {
    let mut tabs = Vec::new();
    let mut slots: Vec<(usize, usize)> = Vec::new(); // (pane, tab position within the pane)
    for (p, pane) in panes.iter().enumerate() {
        for (t, mut info) in (pane.tabs)().into_iter().enumerate() {
            info.active &= pane.primary;
            tabs.push(info);
            slots.push((p, t));
        }
    }

    match plan_ex_command(cmd, &tabs) {
        ExPlan::NotHandled => None,
        ExPlan::Notify(msg) => Some(ExRun {
            toast: Some(msg),
            quit: false,
        }),
        ExPlan::Quit => Some(ExRun {
            toast: None,
            quit: true,
        }),
        ExPlan::QuitDiscard { tabs: which } => {
            for i in which {
                let (pane, tab) = slots[i];
                (panes[pane].discard_tab)(tab);
            }
            Some(ExRun {
                toast: None,
                quit: true,
            })
        }
        ExPlan::Write { tabs: which, then } => {
            let results: Vec<(String, SaveOutcome)> = which
                .iter()
                .map(|&i| {
                    let (pane, tab) = slots[i];
                    (tabs[i].name.clone(), (panes[pane].save_tab)(tab))
                })
                .collect();
            let done = finish_write(&then, &results);
            Some(ExRun {
                toast: done.toast,
                quit: done.quit,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::{Error, ErrorKind};

    fn tab(name: &str, dirty: bool, active: bool) -> TabInfo {
        TabInfo {
            name: name.to_string(),
            dirty,
            active,
        }
    }

    fn denied() -> SaveOutcome {
        SaveOutcome::Failed(Error::new(ErrorKind::PermissionDenied, "Permission denied"))
    }

    // ── plan_ex_command: :w ─────────────────────────────────────────────────

    #[test]
    fn write_saves_only_the_active_tab() {
        let tabs = [tab("a.rs", true, false), tab("b.rs", true, true)];
        for cmd in ["w", "write"] {
            assert_eq!(
                plan_ex_command(cmd, &tabs),
                ExPlan::Write {
                    tabs: vec![1],
                    then: AfterWrite::Stay
                },
                "{cmd}"
            );
        }
    }

    #[test]
    fn write_of_a_clean_active_tab_still_writes_like_ctrl_s() {
        let tabs = [tab("a.rs", false, true)];
        assert_eq!(
            plan_ex_command("w", &tabs),
            ExPlan::Write {
                tabs: vec![0],
                then: AfterWrite::Stay
            }
        );
    }

    #[test]
    fn write_with_no_tabs_says_so_and_writes_nothing() {
        assert_eq!(
            plan_ex_command("w", &[]),
            ExPlan::Notify("No file to save".into())
        );
        assert_eq!(
            plan_ex_command("write", &[]),
            ExPlan::Notify("No file to save".into())
        );
    }

    #[test]
    fn write_with_tabs_but_none_active_says_no_file() {
        let tabs = [tab("a.rs", true, false)];
        assert_eq!(
            plan_ex_command("w", &tabs),
            ExPlan::Notify("No file to save".into())
        );
    }

    // ── plan_ex_command: :wq / :x ───────────────────────────────────────────

    #[test]
    fn wq_and_x_save_the_active_tab_then_quit() {
        let tabs = [tab("a.rs", true, true)];
        for cmd in ["wq", "x", "xit", "exit"] {
            assert_eq!(
                plan_ex_command(cmd, &tabs),
                ExPlan::Write {
                    tabs: vec![0],
                    then: AfterWrite::Quit
                },
                "{cmd}"
            );
        }
    }

    #[test]
    fn wq_with_a_clean_active_tab_still_writes_then_quits() {
        let tabs = [tab("a.rs", false, true), tab("b.rs", false, false)];
        assert_eq!(
            plan_ex_command("wq", &tabs),
            ExPlan::Write {
                tabs: vec![0],
                then: AfterWrite::Quit
            }
        );
    }

    #[test]
    fn wq_with_no_tabs_does_not_quit() {
        for cmd in ["wq", "x"] {
            assert_eq!(
                plan_ex_command(cmd, &[]),
                ExPlan::Notify("No file to save".into()),
                "{cmd}"
            );
        }
    }

    #[test]
    fn wq_keeps_the_window_open_when_another_tab_is_dirty() {
        let tabs = [
            tab("a.rs", true, true),
            tab("b.rs", true, false),
            tab("c.rs", false, false),
        ];
        let ExPlan::Write { tabs: which, then } = plan_ex_command("wq", &tabs) else {
            panic!("wq must still write the active tab");
        };
        assert_eq!(which, vec![0], "only the active tab is written");
        let AfterWrite::StayBecause(why) = then else {
            panic!("must not quit while b.rs is unsaved");
        };
        assert!(why.contains("1 tab has unsaved changes (b.rs)"), "{why}");
        assert!(why.contains(":wqa"), "points at the way out: {why}");
    }

    #[test]
    fn wq_counts_the_active_tab_as_saved_when_checking_others() {
        // Only the active tab is dirty: writing it leaves nothing unsaved, so quitting is fine.
        let tabs = [tab("a.rs", true, true), tab("b.rs", false, false)];
        assert_eq!(
            plan_ex_command("wq", &tabs),
            ExPlan::Write {
                tabs: vec![0],
                then: AfterWrite::Quit
            }
        );
    }

    // ── plan_ex_command: :q / :qa ───────────────────────────────────────────

    #[test]
    fn quit_with_no_tabs_or_only_clean_tabs_quits() {
        let clean = [tab("a.rs", false, true), tab("b.rs", false, false)];
        for cmd in ["q", "quit", "qa", "qall", "quitall"] {
            assert_eq!(plan_ex_command(cmd, &[]), ExPlan::Quit, "{cmd}, no tabs");
            assert_eq!(plan_ex_command(cmd, &clean), ExPlan::Quit, "{cmd}, clean");
        }
    }

    #[test]
    fn quit_refuses_when_any_tab_is_dirty_even_a_background_one() {
        let tabs = [tab("a.rs", false, true), tab("b.rs", true, false)];
        for cmd in ["q", "quit", "qa", "qall", "quitall"] {
            let ExPlan::Notify(msg) = plan_ex_command(cmd, &tabs) else {
                panic!("{cmd} must refuse");
            };
            assert!(
                msg.contains("No write since last change (add ! to override)"),
                "{cmd}: {msg}"
            );
            assert!(
                msg.contains("1 tab has unsaved changes (b.rs)"),
                "{cmd}: {msg}"
            );
        }
    }

    #[test]
    fn quit_refusal_counts_every_dirty_tab() {
        let tabs = [
            tab("a.rs", true, true),
            tab("b.rs", true, false),
            tab("c.rs", false, false),
            tab("d.rs", true, false),
        ];
        let ExPlan::Notify(msg) = plan_ex_command("q", &tabs) else {
            panic!("must refuse");
        };
        assert!(
            msg.contains("3 tabs have unsaved changes (a.rs, b.rs, d.rs)"),
            "{msg}"
        );
        assert!(!msg.contains("c.rs"), "clean tabs are not listed: {msg}");
    }

    #[test]
    fn quit_refusal_shortens_a_long_list() {
        let tabs: Vec<TabInfo> = (0..5)
            .map(|i| tab(&format!("f{i}.rs"), true, i == 0))
            .collect();
        let ExPlan::Notify(msg) = plan_ex_command("qa", &tabs) else {
            panic!("must refuse");
        };
        assert!(
            msg.contains("5 tabs have unsaved changes (f0.rs, f1.rs, f2.rs and 2 more)"),
            "{msg}"
        );
    }

    #[test]
    fn bang_quits_and_discards_exactly_the_dirty_tabs() {
        let tabs = [
            tab("a.rs", true, true),
            tab("b.rs", false, false),
            tab("c.rs", true, false),
        ];
        for cmd in ["q!", "quit!", "qa!", "qall!", "quitall!"] {
            assert_eq!(
                plan_ex_command(cmd, &tabs),
                ExPlan::QuitDiscard { tabs: vec![0, 2] },
                "{cmd}"
            );
            assert_eq!(
                plan_ex_command(cmd, &[]),
                ExPlan::QuitDiscard { tabs: vec![] },
                "{cmd}, no tabs"
            );
        }
    }

    // ── plan_ex_command: :wqa ───────────────────────────────────────────────

    #[test]
    fn wqa_saves_every_dirty_tab_and_only_those() {
        let tabs = [
            tab("a.rs", true, false),
            tab("b.rs", false, true),
            tab("c.rs", true, false),
        ];
        for cmd in ["wqa", "wqall", "xa", "xall", "wall"] {
            assert_eq!(
                plan_ex_command(cmd, &tabs),
                ExPlan::Write {
                    tabs: vec![0, 2],
                    then: AfterWrite::Quit
                },
                "{cmd}"
            );
        }
    }

    #[test]
    fn wqa_with_nothing_dirty_just_quits() {
        let clean = [tab("a.rs", false, true)];
        for cmd in ["wqa", "xa", "wall"] {
            assert_eq!(plan_ex_command(cmd, &clean), ExPlan::Quit, "{cmd}");
            assert_eq!(plan_ex_command(cmd, &[]), ExPlan::Quit, "{cmd}, no tabs");
        }
    }

    // ── plan_ex_command: parsing ────────────────────────────────────────────

    #[test]
    fn surrounding_whitespace_is_ignored() {
        let tabs = [tab("a.rs", true, true)];
        assert_eq!(
            plan_ex_command("  w \t", &tabs),
            plan_ex_command("w", &tabs)
        );
        assert_eq!(
            plan_ex_command(" q! ", &tabs),
            ExPlan::QuitDiscard { tabs: vec![0] }
        );
        assert_eq!(
            plan_ex_command("\twqa\n", &tabs),
            plan_ex_command("wqa", &tabs)
        );
    }

    #[test]
    fn other_commands_are_left_to_the_ex_bar() {
        let tabs = [tab("a.rs", true, true)];
        for cmd in [
            "",
            "   ",
            "e foo.rs",
            "cd /tmp",
            "set number",
            "wat",
            "Q",
            "W",
            "w other.txt",
            "q foo",
            "w!",
            "wq!",
            "x!",
            "q !",
            ":w",
            "qq",
            "wqx",
        ] {
            assert_eq!(plan_ex_command(cmd, &tabs), ExPlan::NotHandled, "{cmd:?}");
            assert_eq!(plan_ex_command(cmd, &[]), ExPlan::NotHandled, "{cmd:?}");
        }
    }

    // ── finish_write ────────────────────────────────────────────────────────

    #[test]
    fn saved_message_names_the_file_after_a_successful_write() {
        let r = finish_write(&AfterWrite::Stay, &[("a.rs".into(), SaveOutcome::Saved)]);
        assert_eq!(
            r,
            WriteResult {
                toast: Some("Saved a.rs".into()),
                quit: false
            }
        );
    }

    #[test]
    fn failed_write_reports_the_reason_and_never_says_saved() {
        let r = finish_write(&AfterWrite::Stay, &[("a.rs".into(), denied())]);
        assert!(!r.quit);
        let msg = r.toast.expect("error toast");
        assert!(msg.contains("Couldn't save a.rs"), "{msg}");
        assert!(msg.contains("Permission denied"), "{msg}");
        assert!(!msg.contains("Saved"), "{msg}");
    }

    #[test]
    fn failed_write_never_quits() {
        let r = finish_write(&AfterWrite::Quit, &[("a.rs".into(), denied())]);
        assert!(!r.quit, "a failed :wq must keep the window open");
        let msg = r.toast.expect("error toast");
        assert!(msg.contains("Permission denied"), "{msg}");
        assert!(msg.contains("not quitting"), "{msg}");
    }

    #[test]
    fn failed_plain_write_does_not_promise_a_quit() {
        // `:w` never asked to quit, so its failure must not say it is "not quitting".
        let r = finish_write(&AfterWrite::Stay, &[("a.rs".into(), denied())]);
        let msg = r.toast.expect("error toast");
        assert_eq!(msg, "Couldn't save a.rs: Permission denied");
    }

    #[test]
    fn unloaded_editor_is_a_failure_not_a_save() {
        let r = finish_write(
            &AfterWrite::Quit,
            &[("a.rs".into(), SaveOutcome::NotLoaded)],
        );
        assert!(!r.quit);
        let msg = r.toast.expect("error toast");
        assert!(msg.contains("Couldn't save a.rs"), "{msg}");
        assert!(!msg.contains("Saved"), "{msg}");
    }

    #[test]
    fn successful_wq_quits_without_claiming_anything() {
        let r = finish_write(&AfterWrite::Quit, &[("a.rs".into(), SaveOutcome::Saved)]);
        assert_eq!(
            r,
            WriteResult {
                toast: None,
                quit: true
            }
        );
    }

    #[test]
    fn one_failure_among_several_blocks_quit_and_counts_what_was_saved() {
        let r = finish_write(
            &AfterWrite::Quit,
            &[
                ("a.rs".into(), SaveOutcome::Saved),
                ("b.rs".into(), denied()),
                ("c.rs".into(), SaveOutcome::Saved),
            ],
        );
        assert!(!r.quit);
        let msg = r.toast.expect("error toast");
        assert!(msg.contains("Saved 2 of 3 tabs"), "{msg}");
        assert!(
            msg.contains("couldn't save b.rs: Permission denied"),
            "{msg}"
        );
        assert!(msg.contains("not quitting"), "{msg}");
    }

    #[test]
    fn several_failures_are_summarised() {
        let r = finish_write(
            &AfterWrite::Stay,
            &[("a.rs".into(), denied()), ("b.rs".into(), denied())],
        );
        let msg = r.toast.expect("error toast");
        assert!(msg.contains("Saved 0 of 2 tabs"), "{msg}");
        assert!(msg.contains("(and 1 more failed)"), "{msg}");
    }

    #[test]
    fn blocked_quit_still_reports_the_successful_save() {
        let r = finish_write(
            &AfterWrite::StayBecause("1 tab has unsaved changes (b.rs)".into()),
            &[("a.rs".into(), SaveOutcome::Saved)],
        );
        assert!(!r.quit);
        let msg = r.toast.expect("toast");
        assert!(msg.starts_with("Saved a.rs"), "{msg}");
        assert!(msg.contains("not quitting"), "{msg}");
        assert!(msg.contains("b.rs"), "{msg}");
    }

    #[test]
    fn blocked_quit_with_a_failed_save_reports_the_failure_instead() {
        let r = finish_write(
            &AfterWrite::StayBecause("1 tab has unsaved changes (b.rs)".into()),
            &[("a.rs".into(), denied())],
        );
        assert!(!r.quit);
        let msg = r.toast.expect("toast");
        assert!(msg.contains("Couldn't save a.rs"), "{msg}");
        assert!(!msg.contains("Saved"), "{msg}");
        assert!(msg.contains("not quitting"), "a quit was asked for: {msg}");
    }

    #[test]
    fn several_saved_writes_are_counted_when_staying_open() {
        let two = [
            ("a.rs".to_string(), SaveOutcome::Saved),
            ("b.rs".to_string(), SaveOutcome::Saved),
        ];
        assert_eq!(
            finish_write(&AfterWrite::Stay, &two),
            WriteResult {
                toast: Some("Saved 2 tabs".into()),
                quit: false
            }
        );
        assert_eq!(
            finish_write(&AfterWrite::StayBecause("c.rs is unsaved".into()), &two),
            WriteResult {
                toast: Some("Saved 2 tabs; not quitting: c.rs is unsaved".into()),
                quit: false
            }
        );
    }

    #[test]
    fn nothing_written_and_staying_open_says_nothing() {
        let r = finish_write(&AfterWrite::Stay, &[]);
        assert_eq!(
            r,
            WriteResult {
                toast: None,
                quit: false
            }
        );
        let r = finish_write(&AfterWrite::StayBecause("why".into()), &[]);
        assert_eq!(r.toast.as_deref(), Some("Not quitting: why"));
        assert!(!r.quit);
    }

    // ── run_ex_command: through real handles, with fake saves ───────────────

    /// Calls a fake pane received, as (pane label, tab index).
    type Log = Rc<RefCell<Vec<(&'static str, usize)>>>;

    /// A pane whose tabs are `(name, dirty, active)`, whose saves are logged into `log` and
    /// answered by `answer(pane_label, tab_index)`, and whose discards go into `discards`.
    fn pane_logging_discards(
        label: &'static str,
        primary: bool,
        tabs: Vec<(&'static str, bool, bool)>,
        log: Log,
        discards: Log,
        answer: impl Fn(&'static str, usize) -> SaveOutcome + 'static,
    ) -> EditorPaneHandle {
        EditorPaneHandle {
            primary,
            tabs: Rc::new(move || {
                tabs.iter()
                    .map(|&(n, d, a)| tab(n, d, a))
                    .collect::<Vec<_>>()
            }),
            save_tab: Rc::new(move |i| {
                log.borrow_mut().push((label, i));
                answer(label, i)
            }),
            discard_tab: Rc::new(move |i| discards.borrow_mut().push((label, i))),
        }
    }

    /// [`pane_logging_discards`] for tests that do not look at discards.
    fn pane(
        label: &'static str,
        primary: bool,
        tabs: Vec<(&'static str, bool, bool)>,
        log: Log,
        answer: impl Fn(&'static str, usize) -> SaveOutcome + 'static,
    ) -> EditorPaneHandle {
        pane_logging_discards(label, primary, tabs, log, Log::default(), answer)
    }

    fn ok(_: &'static str, _: usize) -> SaveOutcome {
        SaveOutcome::Saved
    }

    #[test]
    fn run_unhandled_command_touches_nothing() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", true, true)],
            log.clone(),
            ok,
        )];
        assert_eq!(run_ex_command("e foo.rs", &panes), None);
        assert_eq!(run_ex_command("bogus", &panes), None);
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn run_w_writes_the_active_tab_once_and_reports_it() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", true, false), ("b.rs", true, true)],
            log.clone(),
            ok,
        )];
        let run = run_ex_command("w", &panes).expect("handled");
        assert_eq!(*log.borrow(), vec![("main", 1)]);
        assert_eq!(
            run,
            ExRun {
                toast: Some("Saved b.rs".into()),
                quit: false
            }
        );
    }

    #[test]
    fn run_w_acts_on_the_main_pane_not_a_split() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [
            pane("main", true, vec![("a.rs", false, true)], log.clone(), ok),
            pane("split", false, vec![("b.rs", true, true)], log.clone(), ok),
        ];
        let run = run_ex_command("w", &panes).expect("handled");
        assert_eq!(*log.borrow(), vec![("main", 0)]);
        assert_eq!(run.toast.as_deref(), Some("Saved a.rs"));
    }

    #[test]
    fn run_w_ignores_a_split_that_is_registered_before_the_main_pane() {
        // Panes register in build order, which is not guaranteed to put the main one first.
        for cmd in ["w", "wq", "x"] {
            let log = Rc::new(RefCell::new(vec![]));
            let panes = [
                pane("split", false, vec![("s.rs", true, true)], log.clone(), ok),
                pane(
                    "main",
                    true,
                    vec![("x.rs", false, false), ("m.rs", true, true)],
                    log.clone(),
                    ok,
                ),
            ];
            run_ex_command(cmd, &panes).expect("handled");
            assert_eq!(*log.borrow(), vec![("main", 1)], "{cmd}");
        }
    }

    #[test]
    fn run_w_with_an_empty_main_pane_does_not_write_the_splits_file() {
        for cmd in ["w", "wq", "x"] {
            let log = Rc::new(RefCell::new(vec![]));
            let panes = [
                pane("main", true, vec![], log.clone(), ok),
                pane("split", false, vec![("s.rs", true, true)], log.clone(), ok),
            ];
            let run = run_ex_command(cmd, &panes).expect("handled");
            assert!(log.borrow().is_empty(), "{cmd} wrote a split's tab");
            assert_eq!(run.toast.as_deref(), Some("No file to save"), "{cmd}");
            assert!(!run.quit, "{cmd}");
        }
    }

    #[test]
    fn run_w_without_any_tab_reports_it_and_saves_nothing() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane("main", true, vec![], log.clone(), ok)];
        let run = run_ex_command("w", &panes).expect("handled");
        assert!(log.borrow().is_empty());
        assert_eq!(
            run,
            ExRun {
                toast: Some("No file to save".into()),
                quit: false
            }
        );
        assert!(
            !run_ex_command("wq", &[]).expect("handled").quit,
            "no panes at all"
        );
    }

    #[test]
    fn run_w_failure_is_reported_and_does_not_claim_saved() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", true, true)],
            log,
            |_, _| denied(),
        )];
        let run = run_ex_command("w", &panes).expect("handled");
        let toast = run.toast.expect("error toast");
        assert!(toast.contains("Permission denied"), "{toast}");
        assert!(!toast.contains("Saved"), "{toast}");
        assert!(!run.quit);
    }

    #[test]
    fn run_wq_saves_then_quits() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", true, true)],
            log.clone(),
            ok,
        )];
        let run = run_ex_command("wq", &panes).expect("handled");
        assert_eq!(*log.borrow(), vec![("main", 0)]);
        assert!(run.quit);
    }

    #[test]
    fn run_wq_does_not_quit_when_the_save_fails() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", true, true)],
            log.clone(),
            |_, _| denied(),
        )];
        for cmd in ["wq", "x"] {
            let run = run_ex_command(cmd, &panes).expect("handled");
            assert!(!run.quit, "{cmd}");
            assert!(run.toast.expect("toast").contains("Permission denied"));
        }
        assert_eq!(log.borrow().len(), 2, "each attempt really tried to write");
    }

    #[test]
    fn run_wq_does_not_quit_over_unsaved_work_in_a_split() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [
            pane("main", true, vec![("a.rs", true, true)], log.clone(), ok),
            pane("split", false, vec![("b.rs", true, true)], log.clone(), ok),
        ];
        let run = run_ex_command("wq", &panes).expect("handled");
        assert_eq!(*log.borrow(), vec![("main", 0)], "only the active main tab");
        assert!(!run.quit);
        assert!(run.toast.expect("toast").contains("b.rs"));
    }

    #[test]
    fn run_q_refuses_without_saving_when_a_split_tab_is_dirty() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [
            pane("main", true, vec![("a.rs", false, true)], log.clone(), ok),
            pane("split", false, vec![("b.rs", true, true)], log.clone(), ok),
        ];
        let run = run_ex_command("q", &panes).expect("handled");
        assert!(log.borrow().is_empty(), ":q never writes");
        assert!(!run.quit);
        assert!(run
            .toast
            .expect("toast")
            .contains("No write since last change"));
    }

    #[test]
    fn run_q_bang_quits_without_writing() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", true, true)],
            log.clone(),
            ok,
        )];
        let run = run_ex_command("q!", &panes).expect("handled");
        assert!(log.borrow().is_empty());
        assert_eq!(
            run,
            ExRun {
                toast: None,
                quit: true
            }
        );
    }

    #[test]
    fn run_q_bang_drops_the_recovery_shadow_of_every_dirty_tab() {
        for cmd in ["q!", "qa!"] {
            let (log, discards) = (Log::default(), Log::default());
            let panes = [
                pane_logging_discards(
                    "main",
                    true,
                    vec![("a.rs", true, false), ("b.rs", false, true)],
                    log.clone(),
                    discards.clone(),
                    ok,
                ),
                pane_logging_discards(
                    "split",
                    false,
                    vec![("c.rs", true, true)],
                    log.clone(),
                    discards.clone(),
                    ok,
                ),
            ];
            let run = run_ex_command(cmd, &panes).expect("handled");
            assert!(run.quit, "{cmd}");
            assert_eq!(
                *discards.borrow(),
                vec![("main", 0), ("split", 0)],
                "{cmd}: dirty tabs only, in whichever pane they are"
            );
            assert!(log.borrow().is_empty(), "{cmd} never writes");
        }
    }

    #[test]
    fn run_q_bang_with_nothing_dirty_discards_nothing() {
        let discards = Log::default();
        let panes = [pane_logging_discards(
            "main",
            true,
            vec![("a.rs", false, true)],
            Log::default(),
            discards.clone(),
            ok,
        )];
        assert!(run_ex_command("q!", &panes).expect("handled").quit);
        assert!(discards.borrow().is_empty());
    }

    #[test]
    fn only_q_bang_discards_recovery_files() {
        // Saving, refusing to quit and a clean quit must all leave the shadows alone: they are
        // the safety net for work that is still unsaved.
        for cmd in ["w", "wq", "x", "q", "qa", "wqa"] {
            let discards = Log::default();
            let panes = [pane_logging_discards(
                "main",
                true,
                vec![("a.rs", true, true), ("b.rs", true, false)],
                Log::default(),
                discards.clone(),
                ok,
            )];
            run_ex_command(cmd, &panes).expect("handled");
            assert!(discards.borrow().is_empty(), "{cmd} discarded a shadow");
        }
    }

    #[test]
    fn run_wqa_writes_dirty_tabs_in_every_pane_then_quits() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [
            pane(
                "main",
                true,
                vec![("a.rs", true, false), ("b.rs", false, true)],
                log.clone(),
                ok,
            ),
            pane(
                "split",
                false,
                vec![("c.rs", false, true), ("d.rs", true, false)],
                log.clone(),
                ok,
            ),
        ];
        let run = run_ex_command("wqa", &panes).expect("handled");
        assert_eq!(*log.borrow(), vec![("main", 0), ("split", 1)]);
        assert!(run.quit);
    }

    #[test]
    fn run_wqa_keeps_going_after_a_failure_but_does_not_quit() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![
                ("a.rs", true, true),
                ("b.rs", true, false),
                ("c.rs", true, false),
            ],
            log.clone(),
            |_, i| if i == 1 { denied() } else { SaveOutcome::Saved },
        )];
        let run = run_ex_command("wqa", &panes).expect("handled");
        assert_eq!(
            *log.borrow(),
            vec![("main", 0), ("main", 1), ("main", 2)],
            "a failure on b.rs must not stop c.rs from being written"
        );
        assert!(!run.quit);
        let toast = run.toast.expect("toast");
        assert!(toast.contains("Saved 2 of 3 tabs"), "{toast}");
        assert!(toast.contains("b.rs"), "{toast}");
    }

    #[test]
    fn run_wqa_with_nothing_dirty_quits_without_writing() {
        let log = Rc::new(RefCell::new(vec![]));
        let panes = [pane(
            "main",
            true,
            vec![("a.rs", false, true)],
            log.clone(),
            ok,
        )];
        let run = run_ex_command("wqa", &panes).expect("handled");
        assert!(log.borrow().is_empty());
        assert!(run.quit);
    }
}
