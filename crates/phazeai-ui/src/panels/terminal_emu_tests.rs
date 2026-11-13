//! Tests for the terminal screen model: real escape-sequence streams through
//! the real emulator (no mocks), plus real PTYs running `sh`.

use super::*;

fn core(cols: usize, rows: usize) -> TermCore {
    TermCore::new(cols, rows)
}

fn fed(cols: usize, rows: usize, bytes: &[u8]) -> TermCore {
    let mut c = core(cols, rows);
    c.feed(bytes);
    c
}

fn screen(c: &TermCore) -> Vec<String> {
    c.screen_lines_text()
}

/// Feed `bytes` one byte per call, as if every byte arrived in its own read.
fn feed_bytewise(c: &mut TermCore, bytes: &[u8]) -> FeedOutcome {
    let mut all = FeedOutcome::default();
    for b in bytes {
        let o = c.feed(std::slice::from_ref(b));
        all.reply.extend(o.reply);
        all.blocks_changed |= o.blocks_changed;
        all.cwd_changed |= o.cwd_changed;
        if o.clipboard.is_some() {
            all.clipboard = o.clipboard;
        }
    }
    all
}

// ── Cursor addressing and erase ──────────────────────────────────────────────

#[test]
fn clear_and_home_then_write() {
    let c = fed(20, 4, b"abc\r\ndef\x1b[2J\x1b[HX");
    assert_eq!(screen(&c), vec!["X", "", "", ""]);
    assert_eq!(c.cursor_point(), (0, 1));
}

#[test]
fn csi_h_moves_the_cursor() {
    let c = fed(20, 5, b"\x1b[3;5Hhi\x1b[1;1Htop");
    assert_eq!(screen(&c)[0], "top");
    assert_eq!(screen(&c)[2], "    hi");
    assert_eq!(c.cursor_point(), (0, 3));
}

#[test]
fn csi_k_erases_parts_of_a_line() {
    let c = fed(20, 3, b"hello world\x1b[1;6H\x1b[K");
    assert_eq!(screen(&c)[0], "hello");
    let c = fed(20, 3, b"hello world\x1b[1;6H\x1b[1K");
    assert_eq!(screen(&c)[0], "      world");
    let c = fed(20, 3, b"hello world\x1b[1;6H\x1b[2Kx");
    assert_eq!(screen(&c)[0], "     x");
}

#[test]
fn tabs_are_blank_cells() {
    let c = fed(20, 2, b"a\tb");
    assert_eq!(screen(&c)[0], "a       b");
    let snap = c.snapshot(None, None);
    assert_eq!(snap.cells[0][1].c, ' ');
    assert_eq!(snap.cells[0][8].c, 'b');
}

#[test]
fn carriage_return_overwrites_like_a_progress_bar() {
    let c = fed(20, 3, b"10%\r50%\r100%");
    assert_eq!(screen(&c)[0], "100%");
}

#[test]
fn alt_screen_1049_restores_the_primary_screen() {
    let mut c = fed(20, 4, b"primary\r\n");
    c.feed(b"\x1b[?1049h\x1b[2J\x1b[HALT");
    assert!(c.is_alt_screen());
    assert_eq!(screen(&c)[0], "ALT");
    c.feed(b"\x1b[?1049l");
    assert!(!c.is_alt_screen());
    assert_eq!(screen(&c)[0], "primary");
    assert_eq!(c.cursor_point(), (1, 0));
}

#[test]
fn scroll_region_scrolls_only_its_rows() {
    let mut c = fed(10, 5, b"r1\r\nr2\r\nr3\r\nr4\r\nr5");
    c.feed(b"\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(screen(&c), vec!["r1", "r3", "r4", "", "r5"]);
    assert_eq!(c.history_size(), 0, "a partial region never feeds history");
}

#[test]
fn wide_characters_take_two_cells() {
    let c = fed(10, 2, "a中b".as_bytes());
    assert_eq!(screen(&c)[0], "a中b");
    assert_eq!(c.cursor_point(), (0, 4));
    let snap = c.snapshot(None, None);
    let row = &snap.cells[0];
    assert_eq!(row[1].c, '中');
    assert!(row[1].wide);
    assert!(row[2].spacer);
    assert_eq!(row[3].c, 'b');
}

#[test]
fn resize_keeps_text_and_changes_size() {
    let mut c = fed(10, 3, b"one\r\ntwo");
    c.resize(20, 6, (8, 16));
    assert_eq!(c.size(), TermSize::new(20, 6));
    let text = c.all_text();
    assert!(text.contains("one") && text.contains("two"), "{text}");
    c.feed(b"\x1b[6;20HZ");
    assert_eq!(screen(&c)[5].chars().last(), Some('Z'));
    assert_eq!(screen(&c)[5].len(), 20);
}

// ── Replies to queries ───────────────────────────────────────────────────────

#[test]
fn dsr_cursor_position_reply_reaches_the_writer() {
    let core = Mutex::new(core(20, 10));
    let mut written: Vec<u8> = Vec::new();
    let out = process_pty_output(&core, b"\x1b[5;7H\x1b[6n", &mut written);
    assert_eq!(written, b"\x1b[5;7R");
    assert_eq!(out.reply, b"\x1b[5;7R");
}

#[test]
fn device_attributes_are_answered() {
    let mut c = core(20, 4);
    let out = c.feed(b"\x1b[c");
    assert!(out.reply.starts_with(b"\x1b[?"), "{:?}", out.reply);
}

#[test]
fn background_colour_query_is_answered_with_the_theme() {
    let mut c = core(20, 4);
    let p = TermPalette {
        bg: Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        },
        fg: Rgb {
            r: 0xee,
            g: 0xdd,
            b: 0xcc,
        },
        ..TermPalette::default()
    };
    c.set_palette(p);
    let out = c.feed(b"\x1b]11;?\x07");
    assert_eq!(out.reply, b"\x1b]11;rgb:1212/3434/5656\x07");
    let out = c.feed(b"\x1b]10;?\x1b\\");
    assert_eq!(out.reply, b"\x1b]10;rgb:eeee/dddd/cccc\x1b\\");
}

#[test]
fn text_area_size_queries_are_answered() {
    let mut c = core(30, 7);
    c.resize(30, 7, (9, 18));
    assert_eq!(c.feed(b"\x1b[18t").reply, b"\x1b[8;7;30t");
    assert_eq!(c.feed(b"\x1b[14t").reply, b"\x1b[4;126;270t");
}

#[test]
fn osc52_copy_goes_to_the_clipboard() {
    let mut c = core(20, 4);
    let out = c.feed(b"\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(out.clipboard.as_deref(), Some("hello"));
}

#[test]
fn synchronized_update_without_end_marker_still_draws() {
    let c = fed(20, 3, b"\x1b[?2026hABC");
    assert_eq!(screen(&c)[0], "ABC");
}

// ── Shell integration ────────────────────────────────────────────────────────

const SESSION: &[u8] = b"\x1b]133;A\x07$ ls\r\nfile1\r\nfile2\r\n\x1b]133;D;0\x07\
\x1b]133;A\x07$ false\r\n\x1b]133;D;1\x07\
\x1b]133;A\x07$ exit 127\r\n\x1b]133;D;127;aid=7\x07\x1b]133;A\x07$ ";

fn check_session(c: &TermCore) {
    let blocks = c.blocks();
    assert_eq!(blocks.len(), 4, "{blocks:#?}");
    assert_eq!(blocks[0].exit_code, Some(0));
    assert_eq!(blocks[0].command, "$ ls");
    assert_eq!(blocks[0].output, "file1\nfile2");
    assert_eq!(blocks[0].prompt_row, 0);
    assert_eq!(blocks[0].end_row, Some(3));
    assert_eq!(blocks[1].exit_code, Some(1));
    assert_eq!(blocks[1].command, "$ false");
    assert_eq!(blocks[1].output, "");
    assert_eq!(blocks[1].prompt_row, 3);
    assert_eq!(blocks[2].exit_code, Some(127));
    assert!(blocks[..3].iter().all(|b| b.finished));
    assert!(!blocks[3].finished);
    assert_eq!(blocks[3].prompt_row, 5);
    assert_eq!(c.prompt_rows(), vec![0, 3, 4, 5]);
}

#[test]
fn osc133_blocks_carry_exit_codes() {
    let mut c = core(40, 10);
    let out = c.feed(SESSION);
    assert!(out.blocks_changed);
    check_session(&c);
}

#[test]
fn osc133_blocks_survive_one_byte_reads() {
    let mut c = core(40, 10);
    feed_bytewise(&mut c, SESSION);
    check_session(&c);
}

#[test]
fn osc133_marks_show_in_the_snapshot() {
    let c = fed(40, 10, SESSION);
    let snap = c.snapshot(None, None);
    assert_eq!(
        snap.marks[1], None,
        "output rows carry no mark: {:?}",
        snap.marks
    );
    assert_eq!(
        snap.marks[0],
        Some(RowMark {
            finished: true,
            exit_code: Some(0)
        })
    );
    assert_eq!(
        snap.marks[3],
        Some(RowMark {
            finished: true,
            exit_code: Some(1)
        })
    );
    assert_eq!(
        snap.marks[5],
        Some(RowMark {
            finished: false,
            exit_code: None
        })
    );
}

#[test]
fn osc133_d_without_code_has_no_exit_code() {
    let c = fed(20, 5, b"\x1b]133;A\x07$ x\r\n\x1b]133;D\x07");
    assert_eq!(c.blocks()[0].exit_code, None);
    assert!(c.blocks()[0].finished);
}

#[test]
fn osc7_sets_the_cwd_even_when_split() {
    let mut c = core(20, 4);
    let o1 = c.feed(b"\x1b]7;file://host/home/u/my%20proj");
    assert!(!o1.cwd_changed);
    let o2 = c.feed(b"\x1b\\");
    assert!(o2.cwd_changed);
    assert_eq!(c.cwd(), Some("/home/u/my proj"));
    c.feed(b"\x1b]7;file:///tmp\x07");
    assert_eq!(c.cwd(), Some("/tmp"));
}

#[test]
fn osc7_parsing() {
    assert_eq!(parse_osc7("file:///home/j"), Some("/home/j".into()));
    assert_eq!(parse_osc7("file://box/home/j"), Some("/home/j".into()));
    assert_eq!(parse_osc7("https://x"), None);
    assert_eq!(parse_osc7("file://"), None);
}

#[test]
fn osc_ended_by_another_escape_is_still_seen() {
    // ESC that is not ST ends the OSC and starts a new sequence.
    let c = fed(20, 4, b"\x1b]133;A\x1b[1mX");
    assert_eq!(c.blocks().len(), 1);
    assert_eq!(screen(&c)[0], "X");
}

// ── Scrollback, trimming, navigation ─────────────────────────────────────────

fn numbered_lines(from: usize, to: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for i in from..to {
        v.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    v
}

#[test]
fn scrollback_and_view_scrolling() {
    let mut c = fed(20, 5, &numbered_lines(1, 21));
    assert_eq!(c.history_size(), 16);
    assert_eq!(screen(&c)[0], "line 17");
    c.scroll_lines(3);
    assert_eq!(c.display_offset(), 3);
    assert_eq!(screen(&c)[0], "line 14");
    c.scroll_page(true);
    assert_eq!(screen(&c)[0], "line 9");
    c.scroll_page(false);
    assert_eq!(screen(&c)[0], "line 14");
    c.scroll_to_bottom();
    assert_eq!(c.display_offset(), 0);
    c.scroll_to_abs(0);
    assert_eq!(screen(&c)[0], "line 1");
}

#[test]
fn history_is_trimmed_and_rows_stay_exact() {
    use phazeai_core::constants::terminal::{SCROLLBACK_DRAIN, SCROLLBACK_LIMIT};
    let mut c = core(30, 5);
    c.feed(b"\x1b]133;A\x07$ first\r\n");
    let total = SCROLLBACK_LIMIT + SCROLLBACK_DRAIN + 50;
    c.feed(&numbered_lines(0, total));
    c.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ marker\r\n");
    c.feed(&numbered_lines(0, 30));
    assert!(c.history_size() <= SCROLLBACK_LIMIT + SCROLLBACK_DRAIN);
    assert!(c.trimmed_rows() > 0);
    // The first prompt's row is gone; the second is still addressable.
    let rows = c.prompt_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(c.jump_prompt(-1));
    assert_eq!(screen(&c)[0], "$ marker");
    assert!(c.jump_prompt(1));
    assert_eq!(c.display_offset(), 0);
    // The first block kept its text although its rows are gone.
    assert_eq!(c.blocks()[0].command, "$ first");
    assert!(c.blocks()[0].output.starts_with("line 0\nline 1"));
}

#[test]
fn clearing_history_keeps_prompt_rows_exact() {
    let mut c = core(20, 4);
    c.feed(&numbered_lines(0, 30));
    c.feed(b"\x1b[H\x1b[2J\x1b[3J");
    assert_eq!(c.history_size(), 0);
    c.feed(b"\x1b[H\x1b]133;A\x07$ here\r\nout\r\nout\r\nout\r\nout\r\n");
    assert!(c.jump_prompt(-1));
    assert_eq!(screen(&c)[0], "$ here");
}

#[test]
fn clearing_history_split_across_reads() {
    let mut c = core(20, 4);
    c.feed(&numbered_lines(0, 30));
    c.feed(b"\x1b[3");
    c.feed(b"J\x1b]133;A\x07$ p\r\n1\r\n2\r\n3\r\n4\r\n");
    c.scroll_to_bottom();
    assert!(c.jump_prompt(-1));
    assert_eq!(screen(&c)[0], "$ p");
}

// ── Colours and attributes ───────────────────────────────────────────────────

fn style_of(bytes: &[u8]) -> CellStyle {
    let c = fed(20, 2, bytes);
    c.snapshot(None, None).cells[0][0].style
}

#[test]
fn sgr_attributes_and_colours() {
    let p = TermPalette::default();
    let s = style_of(b"\x1b[1;3;4;9;31;42mX");
    assert!(s.bold && s.italic && s.underline && s.strikeout);
    assert_eq!(s.fg, p.ansi[1]);
    assert_eq!(s.bg, p.ansi[2]);

    let s = style_of(b"\x1b[91;104mX");
    assert_eq!(s.fg, p.ansi[9]);
    assert_eq!(s.bg, p.ansi[12]);

    let s = style_of(b"\x1b[38;5;196mX");
    assert_eq!(s.fg, Rgb { r: 255, g: 0, b: 0 });
    let s = style_of(b"\x1b[38;5;232mX");
    assert_eq!(s.fg, Rgb { r: 8, g: 8, b: 8 });

    let s = style_of(b"\x1b[38;2;1;2;3;48;2;4;5;6mX");
    assert_eq!(s.fg, Rgb { r: 1, g: 2, b: 3 });
    assert_eq!(s.bg, Rgb { r: 4, g: 5, b: 6 });

    let s = style_of(b"\x1b[7mX");
    assert_eq!(s.fg, p.bg);
    assert_eq!(s.bg, p.fg);

    let s = style_of(b"\x1b[8mX");
    assert_eq!(s.fg, s.bg);

    let s = style_of(b"\x1b[2mX");
    assert_ne!(s.fg, p.fg);

    let s = style_of(b"X");
    assert_eq!((s.fg, s.bg), (p.fg, p.bg));
    assert!(!s.bold && !s.italic && !s.underline && !s.strikeout);
}

#[test]
fn program_colour_changes_override_the_palette() {
    let c = fed(20, 2, b"\x1b]4;1;rgb:01/02/03\x07\x1b[31mX");
    let s = c.snapshot(None, None).cells[0][0].style;
    assert_eq!(s.fg, Rgb { r: 1, g: 2, b: 3 });
}

// ── Selection, copy, find ────────────────────────────────────────────────────

#[test]
fn selection_and_copy() {
    let mut c = fed(20, 3, b"hello world\r\nsecond");
    assert_eq!(c.selection_text(), None);
    c.start_selection(SelectionType::Simple, 0, 0, Side::Left);
    c.update_selection(0, 4, Side::Right);
    assert_eq!(c.selection_text().as_deref(), Some("hello"));
    let snap = c.snapshot(None, None);
    assert!(snap.cells[0][4].selected && !snap.cells[0][5].selected);

    c.start_selection(SelectionType::Semantic, 0, 7, Side::Left);
    assert_eq!(c.selection_text().as_deref(), Some("world"));

    c.start_selection(SelectionType::Lines, 1, 2, Side::Left);
    assert_eq!(c.selection_text().as_deref(), Some("second\n"));

    c.start_selection(SelectionType::Simple, 0, 6, Side::Left);
    c.update_selection(1, 1, Side::Right);
    assert_eq!(c.selection_text().as_deref(), Some("world\nse"));

    c.clear_selection();
    assert_eq!(c.selection_text(), None);
    assert_eq!(c.all_text(), "hello world\nsecond");
}

#[test]
fn find_matches_whole_history_and_highlights_the_view() {
    let mut c = fed(20, 3, b"Alpha beta\r\nfoo\r\nbar ALPHA\r\nend");
    let hits = c.find("alpha");
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].start, hits[0].end), (0, 5));
    assert_eq!((hits[1].start, hits[1].end), (4, 9));
    c.scroll_to_bottom();
    let snap = c.snapshot(Some("alpha"), Some(hits[1]));
    // Viewport rows: "foo", "bar ALPHA", "end".
    assert_eq!(snap.cells[1][4].find, FindMark::Current);
    assert_eq!(snap.cells[1][3].find, FindMark::None);
    assert!(c.find("").is_empty());
}

// ── Input encoding ───────────────────────────────────────────────────────────

#[test]
fn arrows_follow_application_cursor_mode() {
    let none = Mods::default();
    assert_eq!(key_bytes(TermKey::Up, none, false), b"\x1b[A");
    assert_eq!(key_bytes(TermKey::Up, none, true), b"\x1bOA");
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    assert_eq!(key_bytes(TermKey::Left, ctrl, true), b"\x1b[1;5D");
    assert_eq!(key_bytes(TermKey::Home, none, true), b"\x1bOH");
    assert_eq!(key_bytes(TermKey::Delete, none, false), b"\x1b[3~");
    assert_eq!(key_bytes(TermKey::F(1), none, false), b"\x1bOP");
    assert_eq!(key_bytes(TermKey::F(12), none, false), b"\x1b[24~");

    let mut c = core(10, 2);
    c.feed(b"\x1b[?1h");
    assert!(c.mode().contains(TermMode::APP_CURSOR));
    c.feed(b"\x1b[?1l");
    assert!(!c.mode().contains(TermMode::APP_CURSOR));
}

#[test]
fn typed_text_with_modifiers() {
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    let alt = Mods {
        alt: true,
        ..Mods::default()
    };
    assert_eq!(text_key_bytes("c", ctrl), vec![3]);
    assert_eq!(text_key_bytes("[", ctrl), vec![0x1b]);
    assert_eq!(text_key_bytes("b", alt), b"\x1bb");
    assert_eq!(text_key_bytes("é", Mods::default()), "é".as_bytes());
}

#[test]
fn bracketed_paste() {
    assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
    assert_eq!(
        paste_bytes("ls\x1b[201~\n", true),
        b"\x1b[200~ls[201~\n\x1b[201~"
    );
    let mut c = core(10, 2);
    c.feed(b"\x1b[?2004h");
    assert!(c.mode().contains(TermMode::BRACKETED_PASTE));
}

#[test]
fn mouse_report_encoding() {
    let mut c = core(300, 5);
    c.feed(b"\x1b[?1000h\x1b[?1006h");
    let mode = c.mode();
    assert!(mode.contains(TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE));
    let none = Mods::default();
    assert_eq!(
        mouse_report(MouseCode::Left, true, false, 4, 2, none, mode),
        b"\x1b[<0;5;3M"
    );
    assert_eq!(
        mouse_report(MouseCode::Left, false, false, 4, 2, none, mode),
        b"\x1b[<0;5;3m"
    );
    assert_eq!(
        mouse_report(MouseCode::WheelDown, true, false, 0, 0, none, mode),
        b"\x1b[<65;1;1M"
    );
    let ctrl = Mods { ctrl: true, ..none };
    assert_eq!(
        mouse_report(MouseCode::Left, true, true, 0, 0, ctrl, mode),
        b"\x1b[<48;1;1M"
    );

    c.feed(b"\x1b[?1006l");
    let legacy = c.mode();
    assert_eq!(
        mouse_report(MouseCode::Left, true, false, 4, 2, none, legacy),
        vec![0x1b, b'[', b'M', 32, 37, 35]
    );
    assert_eq!(
        mouse_report(MouseCode::Left, false, false, 4, 2, none, legacy),
        vec![0x1b, b'[', b'M', 35, 37, 35]
    );
    assert!(mouse_report(MouseCode::Left, true, false, 250, 0, none, legacy).is_empty());

    c.feed(b"\x1b[?1005h");
    let utf8 = c.mode();
    let r = mouse_report(MouseCode::Left, true, false, 250, 0, none, utf8);
    assert_eq!(&r[..4], &[0x1b, b'[', b'M', 32]);
    assert_eq!(std::str::from_utf8(&r[4..]).unwrap(), "\u{11b}!");
}

#[test]
fn cursor_shape_and_visibility() {
    let c = fed(10, 3, b"ab");
    let snap = c.snapshot(None, None);
    let cur = snap.cursor.unwrap();
    assert_eq!((cur.row, cur.col, cur.shape), (0, 2, CursorShape::Block));
    let c = fed(10, 3, b"\x1b[6 q");
    assert_eq!(
        c.snapshot(None, None).cursor.unwrap().shape,
        CursorShape::Beam
    );
    let c = fed(10, 3, b"\x1b[?25l");
    assert!(c.snapshot(None, None).cursor.is_none());
}

// ── Real PTYs ────────────────────────────────────────────────────────────────

#[cfg(unix)]
mod pty {
    use super::*;
    use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Run `script` under `sh -c` in a real PTY, feeding everything it
    /// prints through the core and writing the core's replies back.
    fn run_in_pty(script: &str, cols: u16, rows: u16) -> TermCore {
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new("sh");
        cmd.arg("-c");
        cmd.arg(script);
        cmd.env("TERM", "xterm-256color");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn sh");
        drop(pair.slave); // EOF once sh exits
        let mut writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let core = Mutex::new(TermCore::new(cols as usize, rows as usize));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "pty test timed out");
            match rx.recv_timeout(left) {
                Ok(chunk) => {
                    process_pty_output(&core, &chunk, &mut writer);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("pty test timed out"),
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        drop(pair.master);
        core.into_inner().unwrap()
    }

    #[test]
    fn real_pty_alt_screen_round_trip() {
        let c = run_in_pty(
            r"printf 'before\n'; printf '\033[?1049h\033[2J\033[Hinside'; printf '\033[?1049l'; printf 'after\n'",
            40,
            6,
        );
        assert!(!c.is_alt_screen());
        let s = screen(&c);
        assert_eq!(s[0], "before");
        assert_eq!(s[1], "after");
        assert!(!c.all_text().contains("inside"));
    }

    #[test]
    fn real_pty_osc133_exit_code() {
        let c = run_in_pty(
            r"printf '\033]133;A\007$ x\n'; printf 'out\n\033]133;D;3\007\033]133;A\007$ '",
            40,
            6,
        );
        let b = c.blocks();
        assert_eq!(b.len(), 2, "{b:#?}");
        assert_eq!(b[0].exit_code, Some(3));
        assert_eq!(b[0].command, "$ x");
        assert_eq!(b[0].output, "out");
    }

    #[test]
    fn real_pty_cursor_position_report_round_trip() {
        // The program asks for the cursor position and reads the reply from
        // its terminal: the reply must come from the core via the PTY.
        let c = run_in_pty(
            r#"stty raw -echo; printf '\033[3;4H\033[6n'; r=$(timeout --foreground 5 dd bs=1 count=6 2>/dev/null); stty sane; printf '\033[1;1HGOT[%s]' "$(printf '%s' "$r" | tr '\033' 'E')""#,
            40,
            6,
        );
        assert_eq!(screen(&c)[0], "GOT[E[3;4R]");
    }
}
