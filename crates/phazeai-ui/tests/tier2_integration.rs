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

/// Append a timestamped line to the harness log, which CI uploads when a GUI
/// test fails so the cause can be read without reproducing it.
fn harness_log(msg: impl AsRef<str>) {
    use std::io::Write;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("phazeai-tier2-harness.log"))
    {
        let _ = writeln!(f, "[{ms:>7} ms] {}", msg.as_ref());
    }
}

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
/// when an assertion panics — kills both process groups (so child processes
/// such as language servers die too) and releases the display for the next test.
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
    focus_window_with_click(wid);
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
    // Re-activating a window that already has focus is not a no-op under every
    // window manager, and any focus churn can drop the keyboard focus of the
    // text box inside an open overlay. Only act when the focus is elsewhere.
    let before = xdotool_out(&["getwindowfocus"]);
    if before == id {
        return;
    }
    harness_log(format!(
        "focus_window: {id} is not focused (focus is on '{before}'), activating"
    ));
    xdotool(&["windowactivate", "--sync", &id]);
    xdotool(&["windowfocus", "--sync", &id]);
    harness_log(format!(
        "focus_window: now focused '{}'",
        xdotool_out(&["getwindowfocus"])
    ));
}

/// Focus plus one click in the editor area. Only used right after launch:
/// clicking while an overlay such as the command palette is open could land
/// on one of its rows and run a command.
fn focus_window_with_click(wid: u64) {
    focus_window(wid);
    let id = wid.to_string();
    xdotool(&["mousemove", "--window", &id, "400", "300"]);
    xdotool(&["click", "1"]);
}

/// Under Xvfb's software Vulkan the window shows the frame *before* the last
/// one presented, so a change made by a keystroke stays invisible until
/// something else repaints. Resizing the window by one pixel and back forces
/// a full relayout and present, so the next capture shows the current state.
/// (Hover tricks don't work: they only repaint when a hover style changes,
/// and an open overlay hides the elements underneath.)
fn force_repaint(wid: u64) {
    let id = wid.to_string();
    let Some(out) = run_with_timeout(
        Command::new("xdotool")
            .args(["getwindowgeometry", "--shell", &id])
            .env("DISPLAY", display()),
        Duration::from_secs(5),
    ) else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |name: &str| -> Option<u32> {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.trim().parse().ok())
    };
    let (Some(w), Some(h)) = (field("WIDTH="), field("HEIGHT=")) else {
        return;
    };
    xdotool(&["windowsize", &id, &(w + 1).to_string(), &h.to_string()]);
    thread::sleep(Duration::from_millis(800));
    xdotool(&["windowsize", &id, &w.to_string(), &h.to_string()]);
    thread::sleep(Duration::from_millis(1200));
}

/// Run an xdotool command with the current DISPLAY.
fn xdotool(args: &[&str]) {
    let ok = run_with_timeout(
        Command::new("xdotool").args(args).env("DISPLAY", display()),
        Duration::from_secs(10),
    )
    .is_some_and(|o| o.status.success());
    if !ok {
        harness_log(format!("xdotool {args:?} failed or timed out"));
    }
}

/// Run an xdotool command and return its trimmed stdout ("" on failure).
fn xdotool_out(args: &[&str]) -> String {
    run_with_timeout(
        Command::new("xdotool").args(args).env("DISPLAY", display()),
        Duration::from_secs(10),
    )
    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    .unwrap_or_default()
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
        "Return" => "Return",
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
    harness_log(format!(
        "type_text {text:?}: X focus on '{}' (window {wid})",
        xdotool_out(&["getwindowfocus"])
    ));
    // No --window: uses XTEST for real events
    xdotool(&["type", "--clearmodifiers", "--delay", "60", text]);
    thread::sleep(Duration::from_millis(700));
}

// ── Screenshot via ImageMagick import ──────────────────────────────────────────

/// Capture a named scenario screenshot into tests/snapshots/.
fn screenshot(wid: u64, name: &str) -> DynamicImage {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&dir).ok();
    capture(wid, &dir.join(format!("{name}.png")))
}

/// Screenshot once `ok` accepts the frame and the window has stopped changing,
/// polling for up to 30 s.
///
/// The IDE paints in software here and a frame can trail the keys that caused
/// it by seconds, so one capture after a fixed sleep sometimes shows the state
/// from before the change, or a half-typed one. A frame counts only when `ok`
/// accepts it and the next capture matches it, so a single transient frame can
/// neither satisfy a wait nor decide an assertion. Each capture already costs a
/// couple of seconds, so polling needs no extra sleep. The last frame is
/// returned when the time is up, which leaves the caller's assertion to fail
/// with what the window really showed.
fn screenshot_until(wid: u64, name: &str, ok: impl Fn(&DynamicImage) -> bool) -> DynamicImage {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut accepted: Option<DynamicImage> = None;
    loop {
        let shot = screenshot(wid, name);
        if ok(&shot) {
            if accepted
                .as_ref()
                .is_some_and(|prev| diff_percent(prev, &shot) < 0.05)
            {
                return shot;
            }
            accepted = Some(shot.clone());
        } else {
            accepted = None;
            harness_log(format!(
                "{name}: the expected change is not on screen yet, capturing again"
            ));
        }
        if Instant::now() >= deadline {
            return shot;
        }
    }
}

fn capture(wid: u64, path: &std::path::Path) -> DynamicImage {
    // Delete existing file so scrot doesn't append _001, _002, etc.
    std::fs::remove_file(path).ok();

    force_repaint(wid);

    run_with_timeout(
        // Capture the root window: reading the IDE's own window can return a
        // stale frame when wgpu presents through a swapchain.
        Command::new("import")
            .args(["-window", "root"])
            .arg(path)
            .env("DISPLAY", display()),
        Duration::from_secs(20),
    );

    image::open(path)
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

/// Percentage of pixels that differ inside a band of the frame, comparing `a`
/// at `(x, y)` with `b` at `(x + dx, y + dy)`. The band is given as columns
/// `x0..x1` (fractions of the width) and rows `y0..y1` (pixels).
fn band_diff_percent(
    a: &DynamicImage,
    b: &DynamicImage,
    (x0, x1, y0, y1): (f64, f64, u32, u32),
    (dx, dy): (i32, i32),
) -> f64 {
    let (aw, ah) = a.dimensions();
    if (aw, ah) != b.dimensions() {
        return 100.0;
    }
    let (xa, xb) = ((aw as f64 * x0) as u32, (aw as f64 * x1) as u32);
    let (ya, yb) = (y0.min(ah), y1.min(ah));
    let total = (xb.saturating_sub(xa) * yb.saturating_sub(ya)).max(1) as f64;
    let mut diff = 0u64;
    for y in ya..yb {
        for x in xa..xb {
            let (bx, by) = (x as i32 + dx, y as i32 + dy);
            if bx < 0 || by < 0 || bx >= aw as i32 || by >= ah as i32 {
                diff += 1;
                continue;
            }
            let Rgba([r1, g1, b1, _]) = a.get_pixel(x, y);
            let Rgba([r2, g2, b2, _]) = b.get_pixel(bx as u32, by as u32);
            if (r1 as i32 - r2 as i32).unsigned_abs() > 10
                || (g1 as i32 - g2 as i32).unsigned_abs() > 10
                || (b1 as i32 - b2 as i32).unsigned_abs() > 10
            {
                diff += 1;
            }
        }
    }
    diff as f64 / total * 100.0
}

/// The best match of a band over shifts of up to two pixels either way.
///
/// The harness flushes a repaint by resizing the window by a pixel and back,
/// which can leave the whole UI offset by a pixel in the next capture. The same
/// text shifted by one pixel differs at every glyph edge, so a plain comparison
/// calls identical rows different; a different file name stays different at
/// every shift.
fn band_diff_percent_tolerant(
    a: &DynamicImage,
    b: &DynamicImage,
    band: (f64, f64, u32, u32),
) -> f64 {
    let mut best = 100.0f64;
    for dy in -2..=2 {
        for dx in -2..=2 {
            best = best.min(band_diff_percent(a, b, band, (dx, dy)));
        }
    }
    best
}

/// How much the first row of the open file picker differs between two frames.
/// The row sits right under the search box, whatever the window's width.
fn picker_first_row_diff(a: &DynamicImage, b: &DynamicImage) -> f64 {
    band_diff_percent_tolerant(a, b, (0.25, 0.55, 168, 194))
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
    top_colors_fraction(img, 1)
}

/// Share of the sampled pixels held by the `n` most common colours.
fn top_colors_fraction(img: &DynamicImage, n: usize) -> f64 {
    let (w, h) = img.dimensions();
    let mut counts = std::collections::HashMap::new();
    let mut total = 0u32;
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            *counts.entry(img.get_pixel(x, y)).or_insert(0u32) += 1;
            total += 1;
        }
    }
    let mut sizes: Vec<u32> = counts.into_values().collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.iter().take(n).sum::<u32>() as f64 / total.max(1) as f64
}

/// Whether `img` shows the IDE rather than a blank or half-drawn frame.
///
/// A window that is mapped but has not presented its first frame shows what is
/// behind it: Xvfb's root window, a two-tone checkerboard with a logo. That is
/// not black and is perfectly still, so "not blank, then stable" alone accepts
/// it as the settled UI. Its two checker colours cover about 96% of the frame;
/// a real IDE frame's two most common colours cover about 60%.
fn looks_painted(img: &DynamicImage) -> bool {
    !is_blank(img) && dominant_fraction(img) < 0.85 && top_colors_fraction(img, 2) < 0.90
}

/// Wait (up to 60s) until the window shows the painted UI and two captures
/// in a row match, so a scenario's baseline isn't a half-drawn frame.
fn wait_until_rendered(wid: u64) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut previous: Option<DynamicImage> = None;
    // Why the last frame was not accepted, for the failure message.
    let mut last_why = String::from("no frame captured");
    let settle_png = std::env::temp_dir().join("phazeai-tier2-settle.png");
    while Instant::now() < deadline {
        let shot = capture(wid, &settle_png);
        if looks_painted(&shot) {
            if let Some(prev) = &previous {
                let change = diff_percent(prev, &shot);
                if change < 0.5 {
                    return;
                }
                last_why =
                    format!("painted but still changing ({change:.2}% since the last frame)");
            } else {
                last_why = String::from("painted, waiting for a second matching frame");
            }
            previous = Some(shot);
        } else {
            last_why = format!(
                "not a painted IDE: blank={}, top colour {:.1}%, top two {:.1}%",
                is_blank(&shot),
                dominant_fraction(&shot) * 100.0,
                top_colors_fraction(&shot, 2) * 100.0
            );
            previous = None;
        }
        thread::sleep(Duration::from_millis(1000));
    }
    let kept = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/settle_timeout.png");
    std::fs::copy(&settle_png, &kept).ok();
    panic!(
        "IDE window never finished rendering within 60s; last frame: {last_why} (saved as {})",
        kept.display()
    );
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

// ── Frame classifier (needs no display, so it runs in plain `cargo test`) ─────

/// Xvfb's root window: a two-tone checkerboard with a white logo in the middle.
fn desktop_frame() -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_fn(1280, 1024, |x, y| {
        let (dx, dy) = (x as f64 - 640.0, y as f64 - 480.0);
        if dx * dx + dy * dy < 140.0 * 140.0 {
            Rgba([255, 255, 255, 255])
        } else if (x / 16 + y / 16) % 2 == 0 {
            Rgba([102, 102, 102, 255])
        } else {
            Rgba([153, 153, 153, 255])
        }
    }))
}

/// Shaped like the IDE: an editor, a sidebar and a chat panel as large flat
/// regions (50% / 16% / 22%) and a status bar in many shades.
fn ide_frame() -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_fn(1280, 1024, |x, y| match (x, y) {
        (_, 900..) => Rgba([40 + (x % 200) as u8, 30, 90, 255]),
        (..=229, _) => Rgba([20, 16, 52, 255]),
        (960.., _) => Rgba([26, 20, 70, 255]),
        _ => Rgba([14, 11, 38, 255]),
    }))
}

#[test]
fn the_bare_desktop_is_not_taken_for_a_painted_ide() {
    let desktop = desktop_frame();
    // Not black and no single colour dominates: this is what the old check
    // accepted as "the UI has settled", so it took the desktop as a baseline.
    assert!(!is_blank(&desktop));
    assert!(dominant_fraction(&desktop) < 0.85);
    assert!(top_colors_fraction(&desktop, 2) > 0.90);
    assert!(!looks_painted(&desktop));
}

#[test]
fn an_ide_shaped_frame_counts_as_painted() {
    let ide = ide_frame();
    assert!(top_colors_fraction(&ide, 2) < 0.90);
    assert!(looks_painted(&ide));
}

/// A frame with a "row of text" (a few solid glyph-like blocks) at `x`.
fn row_frame(blocks: &[u32], shift: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_fn(1280, 1024, |x, y| {
        let in_row = (172..188).contains(&y);
        let on_glyph = blocks
            .iter()
            .any(|b| (x >= b + shift && x < b + shift + 6) && in_row);
        if on_glyph {
            Rgba([230, 230, 240, 255])
        } else {
            Rgba([20, 16, 52, 255])
        }
    }))
}

#[test]
fn the_same_row_shifted_a_pixel_still_counts_as_the_same_row() {
    let old = row_frame(&[330, 345, 360, 380, 395, 410], 0);
    let shifted = row_frame(&[330, 345, 360, 380, 395, 410], 1);
    // Compared pixel by pixel, a one-pixel shift looks like a different row.
    assert!(band_diff_percent(&old, &shifted, (0.25, 0.55, 168, 194), (0, 0)) > 1.0);
    assert!(picker_first_row_diff(&old, &shifted) < 1.0);
}

#[test]
fn a_different_row_is_different_at_every_shift() {
    let agents = row_frame(&[330, 345, 360, 380, 395, 410], 0);
    let cargo = row_frame(&[330, 338, 352, 366, 372, 400], 0);
    assert!(picker_first_row_diff(&agents, &cargo) > 1.0);
    assert!(picker_first_row_diff(&agents, &row_frame(&[330, 345, 360, 380, 395, 410], 2)) < 1.0);
}

#[test]
fn blank_and_flat_frames_are_not_painted() {
    let black = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1280,
        1024,
        Rgba([0, 0, 0, 255]),
    ));
    let flat = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1280,
        1024,
        Rgba([14, 10, 40, 255]),
    ));
    assert!(!looks_painted(&black));
    assert!(!looks_painted(&flat));
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
    let open = screenshot_until(wid, "s1_terminal_open", |f| {
        diff_percent(&baseline, f) > 3.0
    });
    let diff_open = diff_percent(&baseline, &open);
    assert!(
        diff_open > 3.0,
        "Terminal open: expected >3% diff, got {diff_open:.2}%"
    );

    send_keys(wid, "ctrl+j");
    let closed = screenshot_until(wid, "s1_terminal_closed", |f| {
        diff_percent(&baseline, f) < 8.0
    });
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
    let open = screenshot_until(wid, "s2_palette_open", |f| diff_percent(&baseline, f) > 5.0);
    let diff_open = diff_percent(&baseline, &open);
    assert!(
        diff_open > 5.0,
        "Palette open: expected >5% diff, got {diff_open:.2}%"
    );

    send_keys(wid, "escape");
    let closed = screenshot_until(wid, "s2_palette_closed", |f| {
        diff_percent(&baseline, f) < 8.0
    });
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
    let closed = screenshot_until(wid, "s3_explorer_closed", |f| diff_percent(&open, f) > 3.0);
    let diff = diff_percent(&open, &closed);
    assert!(
        diff > 3.0,
        "Explorer close: expected >3% diff, got {diff:.2}%"
    );

    send_keys(wid, "ctrl+b");
    let reopened = screenshot_until(wid, "s3_explorer_reopened", |f| {
        diff_percent(&open, f) < 10.0
    });
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
    let closed = screenshot_until(wid, "s4_chat_closed", |f| diff_percent(&open, f) > 1.0);
    let diff = diff_percent(&open, &closed);
    assert!(diff > 1.0, "Chat close: expected >1% diff, got {diff:.2}%");
}

// ── Scenario 5: Type in command palette ──────────────────────────────────────

#[test]
#[ignore = "requires display; run with --ignored under xvfb-run"]
fn scenario_command_palette_type_filter() {
    let ide = launch_ide();
    let wid = ide.wid;

    let baseline = screenshot(wid, "s5_baseline");
    assert!(!is_blank(&baseline), "App rendered blank screen");

    // Ctrl+P must put the keyboard in the search box: no click in between.
    send_keys(wid, "ctrl+p");
    let empty = screenshot_until(wid, "s5_palette_empty", |f| {
        diff_percent(&baseline, f) > 5.0
    });
    assert!(
        diff_percent(&baseline, &empty) > 5.0,
        "the file picker did not open"
    );

    type_text(wid, "cargo.toml");
    // Wait for the typing to arrive and the list to settle; the first-row check
    // below must judge that settled frame, not decide when to stop waiting.
    let filtered = screenshot_until(wid, "s5_palette_filtered", |f| {
        diff_percent(&empty, f) > 0.5
    });
    let diff = diff_percent(&empty, &filtered);
    assert!(
        diff > 0.5,
        "typing in the picker had no effect (list unchanged), diff was {diff:.2}%"
    );
    // The query changes which file is first (AGENTS.md -> Cargo.toml). Rows
    // that keep their old names while the list shrinks are a bug of their own.
    let first_row = picker_first_row_diff(&empty, &filtered);
    assert!(
        first_row > 1.0,
        "the picker's first row still shows the unfiltered file (stale rows), row diff was {first_row:.2}%"
    );

    // Enter opens the top match: the picker closes and a file tab appears.
    send_keys(wid, "Return");
    let opened = screenshot_until(wid, "s5_palette_opened", |f| {
        diff_percent(&filtered, f) > 5.0 && diff_percent(&baseline, f) > 1.0
    });
    let closed_diff = diff_percent(&filtered, &opened);
    assert!(
        closed_diff > 5.0,
        "Enter did not close the picker, diff was {closed_diff:.2}%"
    );
    let tab_diff = diff_percent(&baseline, &opened);
    assert!(
        tab_diff > 1.0,
        "Enter did not open a file, diff from the start was {tab_diff:.2}%"
    );
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
    let after_terminal_pair = screenshot_until(wid, "s6_after_terminal_pair", |f| {
        diff_percent(&initial, f) < 10.0
    });
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
    let after_explorer_pair = screenshot_until(wid, "s6_after_explorer_pair", |f| {
        diff_percent(&initial, f) < 10.0
    });
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

    let final_state = screenshot_until(wid, "s6_final", |f| diff_percent(&initial, f) < 10.0);
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
