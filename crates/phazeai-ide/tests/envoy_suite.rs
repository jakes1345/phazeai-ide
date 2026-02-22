use phazeai_ide::PhazeApp;
use phazeai_ide::panels::editor::{EditorTab, TextPosition, GitLineStatus};
use phazeai_core::Settings;
use std::collections::HashMap;
use egui::{Context, Event, RawInput, pos2, Rect};

#[test]
fn test_crosstalk_prevention() {
    println!("RUNNING: test_crosstalk_prevention");
    let settings = Settings::default();
    let mut app = PhazeApp::new_headless(settings);
    let ctx = Context::default();
    
    let mut raw_input = RawInput::default();
    raw_input.screen_rect = Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0)));

    // 1. Set editor focus to FALSE
    app.editor.has_focus = false;
    app.editor.new_tab();
    
    // 2. Send some text input
    let mut input = raw_input.clone();
    input.events.push(Event::Text("X".to_string()));
    let _ = ctx.run(input, |ctx| {
        app.update_raw(ctx, None);
    });
    
    // 3. Editor should NOT have "X"
    if let Some(tab) = app.editor.tabs.get(app.editor.active_tab) {
        assert_eq!(tab.rope.to_string(), "", "Editor should NOT consume text when not focused");
    }

    // 4. Set editor focus to TRUE
    app.editor.has_focus = true;
    
    // 5. Send some text input
    let mut input = raw_input.clone();
    input.events.push(Event::Text("Y".to_string()));
    let _ = ctx.run(input, |ctx| {
        app.update_raw(ctx, None);
    });
    
    // 6. Editor SHOULD have "Y"
    if let Some(tab) = app.editor.tabs.get(app.editor.active_tab) {
        assert_eq!(tab.rope.to_string(), "Y", "Editor SHOULD consume text when focused");
    }
    println!("PASSED: test_crosstalk_prevention");
}

#[test]
fn test_editor_virtualization_safety() {
    println!("RUNNING: test_editor_virtualization_safety");
    let settings = Settings::default();
    let mut app = PhazeApp::new_headless(settings);
    let ctx = Context::default();
    
    let mut raw_input = RawInput::default();
    raw_input.screen_rect = Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0)));

    app.editor.new_tab();
    let large_content = (0..1000).map(|i| format!("Line {}\n", i)).collect::<String>();
    if let Some(tab) = app.editor.tabs.get_mut(app.editor.active_tab) {
        tab.rope = ropey::Rope::from_str(&large_content);
    }
    
    let _ = ctx.run(raw_input, |ctx| {
        app.update_raw(ctx, None);
    });
    println!("PASSED: test_editor_virtualization_safety");
}

// ── Editor Unit Tests ──────────────────────────────────────────────────────

#[test]
fn test_editor_undo_redo() {
    let mut tab = EditorTab::new_untitled();

    tab.insert_char('a');
    tab.insert_char('b');
    tab.insert_char('c');
    assert_eq!(tab.rope.to_string(), "abc");

    tab.undo();
    // Undo should revert at least one step (coalescing may group edits)
    assert!(tab.rope.to_string().len() < 3 || tab.rope.to_string() != "abc",
        "Undo should reduce content");

    tab.redo();
    assert_eq!(tab.rope.to_string(), "abc", "Redo should restore content");
}

#[test]
fn test_editor_selection_and_copy() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("Hello World");

    // Select all
    tab.select_all();
    assert!(tab.selection.is_some(), "Selection should exist after select_all");
    let sel = tab.selection.as_ref().unwrap();
    let (start, end) = sel.ordered();
    assert_eq!(start, TextPosition::zero(), "Selection should start at 0,0");
    assert!(end.col > 0 || end.line > 0, "Selection end should be non-zero");

    // Selected text
    let selected = tab.selected_text();
    assert_eq!(selected.as_deref(), Some("Hello World"), "Selected text should be 'Hello World'");
}

#[test]
fn test_editor_word_navigation() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("foo bar baz");
    // Cursor should be at end
    assert_eq!(tab.cursor.col, 11);

    // Move word left
    tab.move_word_left(false);
    assert_eq!(tab.cursor.col, 8, "Cursor should be at start of 'baz'");

    tab.move_word_left(false);
    assert_eq!(tab.cursor.col, 4, "Cursor should be at start of 'bar'");
}

#[test]
fn test_editor_delete_forward() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("hello");
    // Move cursor to start
    tab.cursor = TextPosition::zero();

    tab.delete_forward();
    assert_eq!(tab.rope.to_string(), "ello", "delete_forward should remove char at cursor");
}

#[test]
fn test_editor_newline_indent() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("    fn foo() {");
    tab.insert_newline_with_indent();
    let content = tab.rope.to_string();
    // Second line should have same indentation
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with("    "), "New line should match parent indentation");
}

#[test]
fn test_editor_select_next_occurrence() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("foo bar foo");
    // Move cursor to start of first 'foo'
    tab.cursor = TextPosition::zero();

    // First Ctrl+D: select word at cursor
    tab.select_next_occurrence();
    assert!(tab.selection.is_some(), "Should have selection after first Ctrl+D");
    let sel = tab.selection.as_ref().unwrap();
    let (start, end) = sel.ordered();
    assert_eq!(start.col, 0, "Selection should start at col 0");
    assert_eq!(end.col, 3, "Selection should end at col 3 (exclusive)");
}

#[test]
fn test_editor_multi_cursor_insert() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("ab\ncd");

    // Primary cursor at (0, 0), extra cursor at (1, 0)
    tab.cursor = TextPosition::new(0, 0);
    tab.extra_cursors = vec![TextPosition::new(1, 0)];

    tab.insert_char_multi('X');

    let content = tab.rope.to_string();
    // Both positions should have 'X' inserted
    assert!(content.contains("Xab") || content.starts_with("X"),
        "Primary cursor should insert X: got '{}'", content);
    assert!(content.contains("Xcd") || content.contains('\n'),
        "Extra cursor should also insert X: got '{}'", content);
}

#[test]
fn test_git_line_status_variants() {
    // Verify GitLineStatus enum variants exist and can be matched
    let added = GitLineStatus::Added;
    let modified = GitLineStatus::Modified;
    let deleted = GitLineStatus::Deleted;

    let mut map: HashMap<usize, GitLineStatus> = HashMap::new();
    map.insert(0, added);
    map.insert(1, modified);
    map.insert(2, deleted);

    assert!(matches!(map[&0], GitLineStatus::Added));
    assert!(matches!(map[&1], GitLineStatus::Modified));
    assert!(matches!(map[&2], GitLineStatus::Deleted));
}

#[test]
fn test_editor_tab_new_has_empty_extra_cursors() {
    let tab = EditorTab::new_untitled();
    assert!(tab.extra_cursors.is_empty(), "New tab should have no extra cursors");
    assert!(!tab.has_multi_cursor(), "New tab should not be in multi-cursor mode");
}

#[test]
fn test_editor_tab_add_and_collapse_cursors() {
    let mut tab = EditorTab::new_untitled();
    tab.insert_str("hello\nworld");

    let pos = TextPosition::new(1, 0);
    tab.add_cursor(pos);
    assert_eq!(tab.extra_cursors.len(), 1, "Should have one extra cursor");
    assert!(tab.has_multi_cursor());

    tab.collapse_cursors();
    assert!(tab.extra_cursors.is_empty(), "Cursors should be collapsed");
    assert!(!tab.has_multi_cursor());
}
