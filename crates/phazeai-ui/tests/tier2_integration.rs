//! Tier 2: Full GUI integration tests — real window, real keystrokes.
//!
//! Uses `xdotool` (key events via window ID) + `scrot` (screenshots).
//! Zero GPU/DRI3 requirements — works under xvfb-run with software rendering.
//!
//! Run locally (needs X11 display + fluxbox + xdotool + scrot):
//!   BIN=$(cargo test --test tier2_integration -p phazeai-ui --no-run 2>&1 | grep -oP 'deps/tier2_integration-\w+')
//!   ./target/debug/deps/$BIN --ignored --test-threads=1
//!
//! Run in CI (xvfb-run wrapper; do NOT pass via `cargo test` as it double-invokes xvfb):
//!   cargo build --test tier2_integration -p phazeai-ui
//!   BIN=$(ls -t target/debug/deps/tier2_integration-* | grep -v '\.d$' | head -1)
//!   xvfb-run --auto-servernum --server-args="-screen 0 1920x1080x24" \
//!     env LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=softpipe MESA_GL_VERSION_OVERRIDE=3.3 \
//!     "$BIN" --ignored --test-threads=1

use image::{DynamicImage, GenericImageView, Rgba};
use std::{
    path::PathBuf,
    process::{Child, Command},
    thread,
    time::Duration,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn display() -> String {
    std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into())
}

fn binary_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // phazeai-ui → crates
    p.pop(); // crates     → workspace root
    p.push("target/debug/phazeai-ui");
    p
}

/// Kill any stale IDE processes from previous test runs.
fn cleanup_stale() {
    Command::new("pkill").args(["-f", "phazeai-ui"]).status().ok();
    Command::new("pkill").args(["-f", "fluxbox"]).status().ok();
    thread::sleep(Duration::from_millis(500));
}

/// Launch fluxbox WM so xdotool can set real (XTEST) focus.
fn start_wm() {
    let dpy = display();
    Command::new("fluxbox")
        .arg("-display").arg(&dpy)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
    thread::sleep(Duration::from_millis(800));
}

/// Launch the IDE, wait for window to appear, return (child, window_id).
fn launch_ide() -> (Child, u64) {
    cleanup_stale();
    start_wm();

    let bin = binary_path();
    assert!(
        bin.exists(),
        "phazeai-ui binary not found at {bin:?}. Run `cargo build -p phazeai-ui` first."
    );

    let dpy = display();
    let child = Command::new(&bin)
        .env("DISPLAY", &dpy)
        .env("LIBGL_ALWAYS_SOFTWARE", "1")
        .env("GALLIUM_DRIVER", "softpipe")
        .env("MESA_GL_VERSION_OVERRIDE", "3.3")
        .spawn()
        .expect("Failed to launch phazeai-ui");

    // Poll until window appears (up to 15s)
    let wid = wait_for_window("PhazeAI IDE", 30);
    assert!(wid > 0, "IDE window never appeared (xdotool search timed out)");

    // Give the window time to fully render before interacting
    thread::sleep(Duration::from_millis(1500));

    // Click into the window center to force real OS-level focus
    xdotool(&["windowraise", &wid.to_string()]);
    xdotool(&["mousemove", "--window", &wid.to_string(), "400", "300"]);
    xdotool(&["click", "1"]);
    thread::sleep(Duration::from_millis(800));

    (child, wid)
}

/// Poll xdotool search until the window appears, return its window ID.
fn wait_for_window(name: &str, retries: u32) -> u64 {
    let dpy = display();
    for _ in 0..retries {
        let out = Command::new("xdotool")
            .args(["search", "--name", name])
            .env("DISPLAY", &dpy)
            .output();
        if let Ok(out) = out {
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

/// Run an xdotool command with the current DISPLAY.
fn xdotool(args: &[&str]) {
    Command::new("xdotool")
        .args(args)
        .env("DISPLAY", display())
        .status()
        .ok();
}

/// Send a key combo to a specific window ID via XTEST (real events, not synthetic).
/// Requires fluxbox WM to be running for proper focus handling.
fn send_keys(wid: u64, combo: &str) {
    // Click into window to force real OS-level focus for XTEST key events
    xdotool(&["windowraise", &wid.to_string()]);
    xdotool(&["mousemove", "--window", &wid.to_string(), "400", "300"]);
    xdotool(&["click", "1"]);
    thread::sleep(Duration::from_millis(200));

    // Map our shorthand to xdotool key names
    let xkey = match combo {
        "ctrl+j"         => "ctrl+j",
        "ctrl+b"         => "ctrl+b",
        "ctrl+p"         => "ctrl+p",
        "ctrl+backslash" => "ctrl+backslash",
        "escape"         => "Escape",
        other            => other,
    };

    // No --window flag: uses XTEST extension → real events received by Floem/winit
    xdotool(&["key", "--clearmodifiers", xkey]);

    // Let Floem process + re-render (software rendering needs extra time)
    thread::sleep(Duration::from_millis(2000));
}

/// Type a string into a specific window via XTEST.
fn type_text(wid: u64, text: &str) {
    xdotool(&["windowraise", &wid.to_string()]);
    xdotool(&["mousemove", "--window", &wid.to_string(), "400", "300"]);
    xdotool(&["click", "1"]);
    thread::sleep(Duration::from_millis(200));
    // No --window: uses XTEST for real events
    xdotool(&[
        "type",
        "--clearmodifiers",
        "--delay", "60",
        text,
    ]);
    thread::sleep(Duration::from_millis(700));
}

// ── Screenshot via scrot ──────────────────────────────────────────────────────

fn screenshot(name: &str) -> DynamicImage {
    let dir = format!("{}/tests/snapshots", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).ok();
    let path = format!("{dir}/{name}.png");

    // Delete existing file so scrot doesn't append _001, _002, etc.
    std::fs::remove_file(&path).ok();

    for attempt in 0..3 {
        // -o = overwrite, -z = silent
        let ok = Command::new("scrot")
            .args(["-oz", &path])
            .env("DISPLAY", display())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok {
            if let Ok(img) = image::open(&path) {
                return img;
            }
        }
        if attempt < 2 {
            thread::sleep(Duration::from_millis(400));
        }
    }

    // Fallback: import (ImageMagick)
    Command::new("import")
        .args(["-window", "root", &path])
        .env("DISPLAY", display())
        .status()
        .ok();

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
    let (mut child, wid) = launch_ide();

    let baseline = screenshot("s1_baseline");
    assert!(!is_blank(&baseline), "App rendered a blank screen — software rendering may not be active");

    send_keys(wid, "ctrl+j");
    let open = screenshot("s1_terminal_open");
    let diff_open = diff_percent(&baseline, &open);
    assert!(diff_open > 3.0, "Terminal open: expected >3% diff, got {diff_open:.2}%");

    send_keys(wid, "ctrl+j");
    let closed = screenshot("s1_terminal_closed");
    let diff_close = diff_percent(&baseline, &closed);
    assert!(diff_close < 8.0, "Terminal close: expected <8% diff from baseline, got {diff_close:.2}%");

    child.kill().ok();
}

// ── Scenario 2: Command palette open/close ────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_command_palette_open_and_close() {
    let (mut child, wid) = launch_ide();

    let baseline = screenshot("s2_baseline");
    assert!(!is_blank(&baseline), "App rendered blank screen");

    send_keys(wid, "ctrl+p");
    let open = screenshot("s2_palette_open");
    let diff_open = diff_percent(&baseline, &open);
    assert!(diff_open > 5.0, "Palette open: expected >5% diff, got {diff_open:.2}%");

    send_keys(wid, "escape");
    let closed = screenshot("s2_palette_closed");
    let diff_close = diff_percent(&baseline, &closed);
    assert!(diff_close < 8.0, "Palette close: expected <8% from baseline, got {diff_close:.2}%");

    child.kill().ok();
}

// ── Scenario 3: Explorer toggle ───────────────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_toggle_explorer() {
    let (mut child, wid) = launch_ide();

    let open = screenshot("s3_explorer_open");
    assert!(!is_blank(&open), "App rendered blank screen");

    send_keys(wid, "ctrl+b");
    let closed = screenshot("s3_explorer_closed");
    let diff = diff_percent(&open, &closed);
    assert!(diff > 3.0, "Explorer close: expected >3% diff, got {diff:.2}%");

    send_keys(wid, "ctrl+b");
    let reopened = screenshot("s3_explorer_reopened");
    let diff2 = diff_percent(&open, &reopened);
    assert!(diff2 < 10.0, "Explorer reopen: expected to restore, diff was {diff2:.2}%");

    child.kill().ok();
}

// ── Scenario 4: Chat panel toggle ─────────────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_toggle_chat_panel() {
    let (mut child, wid) = launch_ide();

    let open = screenshot("s4_chat_open");
    assert!(!is_blank(&open), "App rendered blank screen");

    send_keys(wid, "ctrl+backslash");
    let closed = screenshot("s4_chat_closed");
    let diff = diff_percent(&open, &closed);
    assert!(diff > 3.0, "Chat close: expected >3% diff, got {diff:.2}%");

    child.kill().ok();
}

// ── Scenario 5: Type in command palette ──────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_command_palette_type_filter() {
    let (mut child, wid) = launch_ide();

    send_keys(wid, "ctrl+p");
    thread::sleep(Duration::from_millis(400));
    let empty = screenshot("s5_palette_empty");
    assert!(!is_blank(&empty), "App rendered blank screen");

    type_text(wid, "toggle");
    let filtered = screenshot("s5_palette_filtered");
    let diff = diff_percent(&empty, &filtered);
    assert!(diff > 0.5, "Palette filter: expected list to change, diff was {diff:.2}%");

    send_keys(wid, "escape");
    child.kill().ok();
}

// ── Scenario 6: Stability — paired toggles cancel out ────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_panel_stability_sequence() {
    let (mut child, wid) = launch_ide();

    let initial = screenshot("s6_initial");
    assert!(!is_blank(&initial), "App rendered blank screen");

    send_keys(wid, "ctrl+j");   // open terminal
    send_keys(wid, "ctrl+j");   // close terminal
    send_keys(wid, "ctrl+b");   // close explorer
    send_keys(wid, "ctrl+b");   // reopen explorer
    send_keys(wid, "ctrl+p");   // open palette
    send_keys(wid, "escape");   // close palette

    let final_state = screenshot("s6_final");
    let diff = diff_percent(&initial, &final_state);
    assert!(diff < 10.0, "Stability: expected <10% diff after paired toggles, got {diff:.2}%");

    child.kill().ok();
}
