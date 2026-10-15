//! Tier 2: Full GUI integration tests — real window, real keystrokes.
//!
//! Uses `xdotool` (key events via window ID) + ImageMagick `import -window` (screenshots).
//! CI installs `imagemagick`; `scrot` stays listed for optional local workflows.
//! Zero GPU/DRI3 requirements — works under xvfb-run with software rendering.
//!
//! Run locally (needs X11 display + fluxbox + xdotool + ImageMagick `import`):
//!   BIN=$(cargo test --test tier2_integration -p phazeai-ui --no-run 2>&1 | grep -oP 'deps/tier2_integration-\w+')
//!   ./target/debug/deps/$BIN --ignored --test-threads=1
//!
//! Run in CI (xvfb-run wrapper; do NOT pass via `cargo test` as it double-invokes xvfb):
//!   cargo build --test tier2_integration -p phazeai-ui
//!   BIN=$(ls -t target/debug/deps/tier2_integration-* | grep -v '\.d$' | head -1)
//!   xvfb-run --auto-servernum --server-args="-screen 0 1920x1080x24" \
//!     env LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=softpipe MESA_GL_VERSION_OVERRIDE=3.3 \
//!     "$BIN" --ignored --test-threads=1
#![cfg(target_os = "linux")]

use image::{DynamicImage, GenericImageView, Rgba};
use serde_json::json;
use std::{
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn display() -> String {
    std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into())
}

fn debug_log(hypothesis_id: &str, location: &str, message: &str, data: serde_json::Value) {
    phazeai_core::debug_ndjson::log(
        "0179af",
        "full-ide-sweep",
        hypothesis_id,
        location,
        message,
        data,
    );
}

fn binary_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // phazeai-ui → crates
    p.pop(); // crates     → workspace root
    p.push("target/debug/phazeai-ui");
    p
}

fn test_home_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phazeai-ui-tier2-home-{}", std::process::id()));
    let config_dir = dir.join(".config").join("phazeai");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&config_dir).expect("failed to create tier2 test home");
    std::fs::write(
        config_dir.join("session.toml"),
        "left_panel_width = 300\nshow_bottom_panel = false\nvim_mode = false\ntheme = \"Midnight Blue\"\n",
    )
    .expect("failed to write tier2 session");
    // An existing config.toml means "not the first run", so the first-run
    // setup card doesn't pop up mid-scenario and skew the screenshots.
    let settings = toml::to_string_pretty(&phazeai_core::Settings::default())
        .expect("serialize default settings");
    std::fs::write(config_dir.join("config.toml"), settings).expect("failed to write tier2 config");
    dir
}

/// One GUI scenario at a time: they share the X display, and parallel runs
/// steal each other's focus and window ids.
static DISPLAY_LOCK: Mutex<()> = Mutex::new(());

/// Run `cmd`, killing it if it takes longer than `limit`. A wedged xdotool or
/// `import` must fail the test, not hang CI.
fn run_with_timeout(cmd: &mut Command, limit: Duration) -> Option<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Kill a process group started with `process_group(0)` and reap its leader.
fn kill_group(child: &mut Child) {
    // SAFETY: plain kill(2) on the process group this test created.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}

/// A running IDE plus the window manager it needs. Dropping it — including
/// when an assertion panics — kills both process groups (so helpers such as
/// the Python sidecar die too) and releases the display for the next test.
struct Ide {
    wid: u64,
    app: Child,
    wm: Option<Child>,
    _display: MutexGuard<'static, ()>,
}

impl Drop for Ide {
    fn drop(&mut self) {
        kill_group(&mut self.app);
        if let Some(wm) = self.wm.as_mut() {
            kill_group(wm);
        }
    }
}

/// Launch fluxbox WM so xdotool can set real (XTEST) focus.
fn start_wm() -> Option<Child> {
    let wm = Command::new("fluxbox")
        .arg("-display")
        .arg(display())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok();
    thread::sleep(Duration::from_millis(800));
    wm
}

/// Launch the IDE and wait for its window.
fn launch_ide() -> Ide {
    let display_lock = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let bin = binary_path();
    assert!(
        bin.exists(),
        "phazeai-ui binary not found at {bin:?}. Run `cargo build -p phazeai-ui` first."
    );

    let wm = start_wm();
    let dpy = display();
    let test_home = test_home_dir();
    // Output goes to a file: an inherited stdout would keep the CI log pipe
    // open for as long as any child of the IDE survives.
    let log = std::fs::File::create(test_home.join("ide.log")).expect("create IDE log");
    let app = Command::new(&bin)
        .env("DISPLAY", &dpy)
        .env("HOME", &test_home)
        .env("XDG_CONFIG_HOME", test_home.join(".config"))
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .env("GALLIUM_DRIVER", "softpipe")
        .env("MESA_GL_VERSION_OVERRIDE", "3.3")
        .stdin(Stdio::null())
        .stdout(log.try_clone().expect("clone IDE log"))
        .stderr(log)
        .process_group(0)
        .spawn()
        .expect("Failed to launch phazeai-ui");
    let mut ide = Ide {
        wid: 0,
        app,
        wm,
        _display: display_lock,
    };

    // Poll until the window appears (up to 15s).
    ide.wid = wait_for_window("PhazeAI IDE", 30);
    assert!(
        ide.wid > 0,
        "IDE window never appeared (xdotool search timed out); see {}",
        test_home.join("ide.log").display()
    );
    let wid = ide.wid;
    // #region agent log
    debug_log(
        "H3",
        "tier2_integration.rs:launch_ide",
        "ide window detected",
        json!({ "windowId": wid, "display": dpy }),
    );
    // #endregion

    // Software rendering can take several seconds to paint the first frame.
    wait_until_rendered(wid);

    // Click into the window center to force real OS-level focus
    focus_window(wid);
    thread::sleep(Duration::from_millis(800));

    ide
}

/// Poll xdotool search until the window appears, return its window ID.
fn wait_for_window(name: &str, retries: u32) -> u64 {
    let dpy = display();
    for _ in 0..retries {
        let out = run_with_timeout(
            Command::new("xdotool")
                .args(["search", "--name", name])
                .env("DISPLAY", &dpy),
            Duration::from_secs(5),
        );
        if let Some(out) = out {
            if out.status.success() {
                let ids: Vec<u64> = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|l| l.trim().parse().ok())
                    .collect();
                if let Some(&id) = ids.last() {
                    return id;
                }
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    0
}

/// Give `wid` real keyboard focus: activate it through the window manager,
/// focus it, then click inside so winit sees pointer + focus events.
fn focus_window(wid: u64) {
    let id = wid.to_string();
    xdotool(&["windowactivate", "--sync", &id]);
    xdotool(&["windowfocus", "--sync", &id]);
    xdotool(&["mousemove", "--window", &id, "400", "300"]);
    xdotool(&["click", "1"]);
}

/// Under Xvfb's software Vulkan the window shows the frame *before* the last
/// one presented, so a change made by a keystroke stays invisible until
/// something else repaints. Hovering on and off an activity-bar button forces
/// fresh frames, so the capture reflects the current state.
fn flush_frames(wid: u64) {
    let id = wid.to_string();
    for _ in 0..2 {
        xdotool(&["mousemove", "--window", &id, "24", "100"]);
        thread::sleep(Duration::from_millis(400));
        xdotool(&["mousemove", "--window", &id, "24", "700"]);
        thread::sleep(Duration::from_millis(400));
    }
}

/// Run an xdotool command with the current DISPLAY.
fn xdotool(args: &[&str]) {
    run_with_timeout(
        Command::new("xdotool").args(args).env("DISPLAY", display()),
        Duration::from_secs(10),
    );
}

/// Send a key combo to a specific window ID via XTEST (real events, not synthetic).
/// Requires fluxbox WM to be running for proper focus handling.
fn send_keys(wid: u64, combo: &str) {
    // #region agent log
    debug_log(
        "H1",
        "tier2_integration.rs:send_keys",
        "sending key combo",
        json!({ "windowId": wid, "combo": combo }),
    );
    // #endregion
    // Click into window to force real OS-level focus for XTEST key events
    focus_window(wid);
    thread::sleep(Duration::from_millis(200));

    // Map our shorthand to xdotool key names
    let xkey = match combo {
        "ctrl+j" => "ctrl+j",
        "ctrl+b" => "ctrl+b",
        "ctrl+p" => "ctrl+p",
        "ctrl+backslash" => "ctrl+backslash",
        "escape" => "Escape",
        other => other,
    };

    // No --window flag: uses XTEST extension → real events received by Floem/winit
    xdotool(&["key", "--clearmodifiers", xkey]);

    // Let Floem process + re-render (software rendering needs extra time)
    thread::sleep(Duration::from_millis(2000));
}

/// Type a string into a specific window via XTEST.
fn type_text(wid: u64, text: &str) {
    // #region agent log
    debug_log(
        "H2",
        "tier2_integration.rs:type_text",
        "typing text",
        json!({ "windowId": wid, "length": text.len() }),
    );
    // #endregion
    focus_window(wid);
    thread::sleep(Duration::from_millis(200));
    // No --window: uses XTEST for real events
    xdotool(&["type", "--clearmodifiers", "--delay", "60", text]);
    thread::sleep(Duration::from_millis(700));
}

// ── Screenshot via ImageMagick import ──────────────────────────────────────────

fn screenshot(wid: u64, name: &str) -> DynamicImage {
    let dir = format!("{}/tests/snapshots", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).ok();
    let path = format!("{dir}/{name}.png");

    // Delete existing file so scrot doesn't append _001, _002, etc.
    std::fs::remove_file(&path).ok();

    flush_frames(wid);

    run_with_timeout(
        // Capture the root window: reading the IDE's own window can return a
        // stale frame when wgpu presents through a swapchain.
        Command::new("import")
            .args(["-window", "root", &path])
            .env("DISPLAY", display()),
        Duration::from_secs(20),
    );

    image::open(&path)
        .unwrap_or_else(|_| DynamicImage::ImageRgba8(image::RgbaImage::new(1920, 1080)))
}

// ── Image helpers ─────────────────────────────────────────────────────────────

fn diff_percent(a: &DynamicImage, b: &DynamicImage) -> f64 {
    let (aw, ah) = a.dimensions();
    let (bw, bh) = b.dimensions();
    if aw != bw || ah != bh {
        return 100.0;
    }
    let total = (aw * ah) as f64;
    let mut diff = 0u64;
    for y in 0..ah {
        for x in 0..aw {
            let Rgba([r1, g1, b1, _]) = a.get_pixel(x, y);
            let Rgba([r2, g2, b2, _]) = b.get_pixel(x, y);
            if (r1 as i32 - r2 as i32).unsigned_abs() > 10
                || (g1 as i32 - g2 as i32).unsigned_abs() > 10
                || (b1 as i32 - b2 as i32).unsigned_abs() > 10
            {
                diff += 1;
            }
        }
    }
    (diff as f64 / total) * 100.0
}

fn diff_stats(a: &DynamicImage, b: &DynamicImage) -> (f64, Option<(u32, u32, u32, u32)>) {
    let (aw, ah) = a.dimensions();
    let (bw, bh) = b.dimensions();
    if aw != bw || ah != bh {
        return (100.0, None);
    }

    let total = (aw * ah) as f64;
    let mut diff = 0u64;
    let mut min_x = aw;
    let mut min_y = ah;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..ah {
        for x in 0..aw {
            let Rgba([r1, g1, b1, _]) = a.get_pixel(x, y);
            let Rgba([r2, g2, b2, _]) = b.get_pixel(x, y);
            if (r1 as i32 - r2 as i32).unsigned_abs() > 10
                || (g1 as i32 - g2 as i32).unsigned_abs() > 10
                || (b1 as i32 - b2 as i32).unsigned_abs() > 10
            {
                diff += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    let percent = (diff as f64 / total) * 100.0;
    let bbox = if diff > 0 {
        Some((min_x, min_y, max_x, max_y))
    } else {
        None
    };
    (percent, bbox)
}

/// Share of sampled pixels that have the single most common colour. An
/// unpainted window (black, or the window manager's background) is nearly
/// all one colour; the IDE's UI never is.
fn dominant_fraction(img: &DynamicImage) -> f64 {
    let (w, h) = img.dimensions();
    let mut counts = std::collections::HashMap::new();
    let mut total = 0u32;
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            *counts.entry(img.get_pixel(x, y)).or_insert(0u32) += 1;
            total += 1;
        }
    }
    let top = counts.values().copied().max().unwrap_or(0);
    top as f64 / total.max(1) as f64
}

/// Wait (up to 60s) until the window shows the painted UI and two captures
/// in a row match, so a scenario's baseline isn't a half-drawn frame.
fn wait_until_rendered(wid: u64) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut previous: Option<DynamicImage> = None;
    while Instant::now() < deadline {
        let shot = screenshot(wid, "_settle");
        let painted = !is_blank(&shot) && dominant_fraction(&shot) < 0.85;
        if painted {
            if let Some(prev) = &previous {
                if diff_percent(prev, &shot) < 0.5 {
                    return;
                }
            }
            previous = Some(shot);
        } else {
            previous = None;
        }
        thread::sleep(Duration::from_millis(1000));
    }
    panic!("IDE window never finished rendering within 60s");
}

fn is_blank(img: &DynamicImage) -> bool {
    let (w, h) = img.dimensions();
    let mut non_black = 0u32;
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            let Rgba([r, g, b, _]) = img.get_pixel(x, y);
            if r as u32 + g as u32 + b as u32 > 30 {
                non_black += 1;
                if non_black > 20 {
                    return false;
                }
            }
        }
    }
    true
}

// ── Scenario 1: Toggle terminal (Ctrl+J) ─────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_toggle_terminal() {
    let ide = launch_ide();
    let wid = ide.wid;

    let baseline = screenshot(wid, "s1_baseline");
    assert!(
        !is_blank(&baseline),
        "App rendered a blank screen — software rendering may not be active"
    );

    send_keys(wid, "ctrl+j");
    let open = screenshot(wid, "s1_terminal_open");
    let diff_open = diff_percent(&baseline, &open);
    assert!(
        diff_open > 3.0,
        "Terminal open: expected >3% diff, got {diff_open:.2}%"
    );

    send_keys(wid, "ctrl+j");
    let closed = screenshot(wid, "s1_terminal_closed");
    let diff_close = diff_percent(&baseline, &closed);
    assert!(
        diff_close < 8.0,
        "Terminal close: expected <8% diff from baseline, got {diff_close:.2}%"
    );
}

// ── Scenario 2: Command palette open/close ────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_command_palette_open_and_close() {
    let ide = launch_ide();
    let wid = ide.wid;

    let baseline = screenshot(wid, "s2_baseline");
    assert!(!is_blank(&baseline), "App rendered blank screen");

    send_keys(wid, "ctrl+p");
    let open = screenshot(wid, "s2_palette_open");
    let diff_open = diff_percent(&baseline, &open);
    assert!(
        diff_open > 5.0,
        "Palette open: expected >5% diff, got {diff_open:.2}%"
    );

    send_keys(wid, "escape");
    let closed = screenshot(wid, "s2_palette_closed");
    let diff_close = diff_percent(&baseline, &closed);
    assert!(
        diff_close < 8.0,
        "Palette close: expected <8% from baseline, got {diff_close:.2}%"
    );
}

// ── Scenario 3: Explorer toggle ───────────────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_toggle_explorer() {
    let ide = launch_ide();
    let wid = ide.wid;

    let open = screenshot(wid, "s3_explorer_open");
    assert!(!is_blank(&open), "App rendered blank screen");

    send_keys(wid, "ctrl+b");
    let closed = screenshot(wid, "s3_explorer_closed");
    let diff = diff_percent(&open, &closed);
    assert!(
        diff > 3.0,
        "Explorer close: expected >3% diff, got {diff:.2}%"
    );

    send_keys(wid, "ctrl+b");
    let reopened = screenshot(wid, "s3_explorer_reopened");
    let diff2 = diff_percent(&open, &reopened);
    assert!(
        diff2 < 10.0,
        "Explorer reopen: expected to restore, diff was {diff2:.2}%"
    );
}

// ── Scenario 4: Chat panel toggle ─────────────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_toggle_chat_panel() {
    let ide = launch_ide();
    let wid = ide.wid;

    let open = screenshot(wid, "s4_chat_open");
    assert!(!is_blank(&open), "App rendered blank screen");

    send_keys(wid, "ctrl+backslash");
    let closed = screenshot(wid, "s4_chat_closed");
    let diff = diff_percent(&open, &closed);
    assert!(diff > 1.0, "Chat close: expected >1% diff, got {diff:.2}%");
}

// ── Scenario 5: Type in command palette ──────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_command_palette_type_filter() {
    let ide = launch_ide();
    let wid = ide.wid;

    send_keys(wid, "ctrl+p");
    thread::sleep(Duration::from_millis(400));
    let empty = screenshot(wid, "s5_palette_empty");
    assert!(!is_blank(&empty), "App rendered blank screen");

    type_text(wid, "toggle");
    let filtered = screenshot(wid, "s5_palette_filtered");
    let diff = diff_percent(&empty, &filtered);
    assert!(
        diff > 0.5,
        "Palette filter: expected list to change, diff was {diff:.2}%"
    );

    send_keys(wid, "escape");
}

// ── Scenario 6: Stability — paired toggles cancel out ────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_panel_stability_sequence() {
    let ide = launch_ide();
    let wid = ide.wid;

    let initial = screenshot(wid, "s6_initial");
    assert!(!is_blank(&initial), "App rendered blank screen");

    send_keys(wid, "ctrl+j"); // open terminal
    send_keys(wid, "ctrl+j"); // close terminal
    let after_terminal_pair = screenshot(wid, "s6_after_terminal_pair");
    let (after_terminal_diff, after_terminal_bbox) = diff_stats(&initial, &after_terminal_pair);
    // #region agent log
    debug_log(
        "H17",
        "tier2_integration.rs:scenario_panel_stability_sequence",
        "after terminal toggle pair",
        json!({ "diffPercent": after_terminal_diff, "bbox": after_terminal_bbox }),
    );
    // #endregion

    send_keys(wid, "ctrl+b"); // close explorer
    send_keys(wid, "ctrl+b"); // reopen explorer
    let after_explorer_pair = screenshot(wid, "s6_after_explorer_pair");
    let (after_explorer_diff, after_explorer_bbox) = diff_stats(&initial, &after_explorer_pair);
    // #region agent log
    debug_log(
        "H18",
        "tier2_integration.rs:scenario_panel_stability_sequence",
        "after explorer toggle pair",
        json!({ "diffPercent": after_explorer_diff, "bbox": after_explorer_bbox }),
    );
    // #endregion

    send_keys(wid, "ctrl+p"); // open palette
    send_keys(wid, "escape"); // close palette

    let final_state = screenshot(wid, "s6_final");
    let (diff, bbox) = diff_stats(&initial, &final_state);
    // #region agent log
    debug_log(
        "H19",
        "tier2_integration.rs:scenario_panel_stability_sequence",
        "stability scenario diff computed",
        json!({ "diffPercent": diff, "bbox": bbox }),
    );
    // #endregion
    assert!(
        diff < 10.0,
        "Stability: expected <10% diff after paired toggles, got {diff:.2}%"
    );
}
