//! End-to-end DAP session smoke test: compiles a tiny C program, debugs it
//! through `debug_session::run_session` with a breakpoint, continues, and
//! expects clean termination. Skips (passes) when no adapter/compiler exists.

use phazeai_ui::debug_session::{detect_adapter, run_session, DebugCmd, SessionUpdate};
use phazeai_ui::domain_state::project::DebugStatus;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn dap_session_breakpoint_continue_exit() {
    let Some((adapter_cmd, adapter_args)) = detect_adapter() else {
        eprintln!("SKIP: no DAP adapter installed");
        return;
    };

    // Build the debuggee.
    let dir = std::env::temp_dir().join("phazeai_dap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("main.c");
    let bin = dir.join("main");
    std::fs::write(
        &src,
        "#include <stdio.h>\nint main(void) {\n    int a = 1;\n    int b = a + 41;\n    printf(\"%d\\n\", b);\n    return 0;\n}\n",
    )
    .unwrap();
    let cc = std::process::Command::new("cc")
        .args(["-g", "-O0", "-o"])
        .arg(&bin)
        .arg(&src)
        .status();
    match cc {
        Ok(s) if s.success() => {}
        _ => {
            eprintln!("SKIP: no C compiler available");
            return;
        }
    }

    let (cmd_tx, cmd_rx) = mpsc::channel::<DebugCmd>();
    let (update_tx, update_rx) = mpsc::sync_channel::<SessionUpdate>(256);

    let program = bin.to_string_lossy().to_string();
    let breakpoints = vec![(src.clone(), 4u64)]; // line `int b = a + 41;`
    let cwd: PathBuf = dir.clone();
    let session = std::thread::spawn(move || {
        run_session(
            adapter_cmd,
            adapter_args,
            program,
            cwd,
            breakpoints,
            cmd_rx,
            update_tx,
        );
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut saw_stop_at_bp = false;
    let mut saw_frames = false;
    let mut final_idle = false;
    let mut continued = false;

    while Instant::now() < deadline {
        let Ok(update) = update_rx.recv_timeout(Duration::from_secs(5)) else {
            break;
        };
        match update {
            SessionUpdate::StoppedAt(Some((file, line))) => {
                assert_eq!(file, src, "stopped in the wrong file");
                assert_eq!(line, 4, "stopped at the wrong line");
                saw_stop_at_bp = true;
            }
            SessionUpdate::Frames(frames) if !frames.is_empty() => {
                saw_frames = true;
                assert!(
                    frames.iter().any(|(_, name, _, _)| name.contains("main")),
                    "expected a 'main' frame, got: {frames:?}"
                );
                // We're stopped with frames populated — resume.
                if !continued {
                    continued = true;
                    cmd_tx.send(DebugCmd::Continue).unwrap();
                }
            }
            SessionUpdate::Status(DebugStatus::Idle) if continued => {
                final_idle = true;
                break;
            }
            _ => {}
        }
    }

    session.join().unwrap();
    assert!(saw_stop_at_bp, "never stopped at the breakpoint");
    assert!(saw_frames, "never received stack frames");
    assert!(final_idle, "session never returned to Idle after continue");
}
