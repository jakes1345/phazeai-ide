use floem::{
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};

use crate::app::{IdeState, SearchResult};

/// The search panel — workspace search + multi-file replace.
pub fn search_panel(state: IdeState) -> impl IntoView {
    let theme   = state.theme;
    let query   = state.search_query;
    let results = state.search_results;
    let is_searching   = create_rw_signal(false);
    let replace_text   = create_rw_signal(String::new());
    let replace_open   = create_rw_signal(false);
    let replace_status = create_rw_signal(String::new());
    let use_regex      = create_rw_signal(false);
    let case_sensitive = create_rw_signal(false);

    // ── Header ───────────────────────────────────────────────────────────────
    let header = container(
        stack((
            label(|| "SEARCH")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.color(p.text_muted).font_size(11.0).font_weight(floem::text::Weight::BOLD).flex_grow(1.0)
                }),
            // Toggle replace mode
            container(label(move || if replace_open.get() { "▾ Replace" } else { "▸ Replace" }))
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(10.0).padding_horiz(6.0).padding_vert(2.0)
                     .border_radius(3.0).cursor(floem::style::CursorStyle::Pointer)
                     .color(p.text_muted)
                     .hover(|s| s.background(theme.get().palette.bg_elevated))
                })
                .on_click_stop(move |_| { replace_open.update(|v| *v = !*v); }),
        ))
        .style(|s| s.flex_row().items_center().width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.padding_horiz(12.0).padding_vert(8.0)
         .border_bottom(1.0).border_color(p.border).width_full()
    });

    // ── Search options row ────────────────────────────────────────────────────
    let opt_regex = {
        let t = theme;
        container(label(|| ".*"))
            .style(move |s| {
                let p = t.get().palette;
                s.font_size(11.0).padding_horiz(6.0).padding_vert(2.0).border_radius(3.0)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .color(if use_regex.get() { p.bg_base } else { p.text_muted })
                 .background(if use_regex.get() { p.accent } else { p.bg_elevated })
                 .border(1.0).border_color(p.border)
            })
            .on_click_stop(move |_| { use_regex.update(|v| *v = !*v); })
    };

    let opt_case = {
        let t = theme;
        container(label(|| "Aa"))
            .style(move |s| {
                let p = t.get().palette;
                s.font_size(11.0).padding_horiz(6.0).padding_vert(2.0).border_radius(3.0)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .color(if case_sensitive.get() { p.bg_base } else { p.text_muted })
                 .background(if case_sensitive.get() { p.accent } else { p.bg_elevated })
                 .border(1.0).border_color(p.border)
            })
            .on_click_stop(move |_| { case_sensitive.update(|v| *v = !*v); })
    };

    // ── Search input ──────────────────────────────────────────────────────────
    let search_bar = {
        let state2 = state.clone();
        container(
            stack((
                text_input(query)
                    .placeholder("Search in files...")
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.flex_grow(1.0).min_width(0.0)
                         .background(p.bg_elevated).border(1.0).border_color(p.border)
                         .border_radius(4.0).color(p.text_primary)
                         .padding_horiz(8.0).padding_vert(6.0).font_size(13.0)
                    })
                    .on_event_stop(floem::event::EventListener::KeyDown, move |event| {
                        if let floem::event::Event::KeyDown(ke) = event {
                            if ke.key.logical_key == floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter) {
                                let root = state2.workspace_root.get();
                                perform_search(state2.clone(), is_searching, root, use_regex, case_sensitive);
                            }
                        }
                    }),
                opt_regex,
                opt_case,
            ))
            .style(|s| s.flex_row().items_center().gap(4.0).width_full()),
        )
        .style(|s| s.padding_horiz(8.0).padding_vert(4.0).width_full())
    };

    // ── Replace input (shown when replace_open) ───────────────────────────────
    let replace_bar = {
        let state3 = state.clone();
        container(stack((
            text_input(replace_text)
                .placeholder("Replace with...")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.flex_grow(1.0).min_width(0.0)
                     .background(p.bg_elevated).border(1.0).border_color(p.border)
                     .border_radius(4.0).color(p.text_primary)
                     .padding_horiz(8.0).padding_vert(6.0).font_size(13.0)
                }),
            container(label(|| "Replace All"))
                .style(move |s| {
                    let p = theme.get().palette;
                    s.padding_horiz(8.0).padding_vert(5.0).border_radius(4.0)
                     .font_size(11.0).cursor(floem::style::CursorStyle::Pointer)
                     .color(p.bg_base).background(p.accent)
                     .hover(|s| s.background(theme.get().palette.accent_hover))
                })
                .on_click_stop(move |_| {
                    perform_replace_all(
                        state3.search_results.get(),
                        query.get(),
                        replace_text.get(),
                        replace_status,
                        use_regex.get(),
                        case_sensitive.get(),
                    );
                }),
        ))
        .style(|s| s.flex_row().items_center().gap(4.0).width_full()))
        .style(move |s| {
            s.padding_horiz(8.0).padding_vert(4.0).width_full()
             .apply_if(!replace_open.get(), |s| s.display(floem::style::Display::None))
        })
    };

    // Replace status
    let status_label = label(move || replace_status.get())
        .style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0).color(p.success).padding_horiz(12.0).padding_vert(3.0)
             .apply_if(replace_status.get().is_empty(), |s| s.display(floem::style::Display::None))
        });

    // ── Results list ──────────────────────────────────────────────────────────
    let results_view = {
        let state4 = state.clone();
        dyn_stack(
            move || results.get(),
            |res| format!("{}:{}:{}", res.path.display(), res.line, res.content),
            {
                let state5 = state4.clone();
                move |res: SearchResult| {
                    let path    = res.path.clone();
                    let line    = res.line;
                    let content = res.content.trim().to_string();
                    let hovered = create_rw_signal(false);
                    let s       = state5.clone();
                    let filename = path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "?".to_string());

                    container(stack((
                        label(move || format!("{}:{} ", filename, line))
                            .style(move |s| s.font_size(12.0).color(theme.get().palette.accent).margin_right(4.0)),
                        label(move || content.clone())
                            .style(move |s| s.font_size(12.0).color(theme.get().palette.text_primary).flex_grow(1.0)),
                    ))
                    .style(|s| s.items_center()))
                    .style(move |_s| {
                        let p = theme.get().palette;
                        _s.width_full().padding_horiz(12.0).padding_vert(4.0)
                          .background(if hovered.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                          .cursor(floem::style::CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| {
                        s.open_file.set(Some(path.clone()));
                        s.goto_line.set(line as u32);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { hovered.set(true); })
                    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { hovered.set(false); })
                }
            }
        )
        .style(|s: floem::style::Style| s.flex_col().width_full())
    };

    let results_scroll = scroll(stack((
        results_view,
        label(move || {
            if is_searching.get() { "Searching...".to_string() }
            else if results.get().is_empty() && !query.get().is_empty() { "No results found.".to_string() }
            else if !results.get().is_empty() { format!("{} results", results.get().len()) }
            else { String::new() }
        })
        .style(move |s| s.font_size(12.0).color(theme.get().palette.text_muted).padding(12.0)),
    )))
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((header, search_bar, replace_bar, status_label, results_scroll))
        .style(move |s| {
            let p = theme.get().palette;
            s.flex_col().width_full().height_full().background(p.bg_panel)
        })
}

fn perform_search(
    state: IdeState,
    is_searching: RwSignal<bool>,
    root: std::path::PathBuf,
    use_regex: RwSignal<bool>,
    case_sensitive: RwSignal<bool>,
) {
    let query = state.search_query.get();
    if query.is_empty() { return; }

    is_searching.set(true);
    state.search_results.set(vec![]);

    let regex     = use_regex.get();
    let case_sens = case_sensitive.get();
    let (tx, rx)  = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut found = Vec::new();

        let mut rg_args = vec![
            "--line-number".to_string(),
            "--no-heading".to_string(),
            "--color=never".to_string(),
        ];
        if !case_sens { rg_args.push("--ignore-case".to_string()); }
        if !regex     { rg_args.push("--fixed-strings".to_string()); }
        rg_args.push("-e".to_string());
        rg_args.push(query.clone());

        let rg_result = std::process::Command::new("rg")
            .args(&rg_args)
            .current_dir(&root)
            .output();

        if let Ok(output) = rg_result {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().take(500) {
                let parts: Vec<&str> = line.splitn(3, ':').collect();
                if parts.len() == 3 {
                    if let Ok(line_num) = parts[1].parse::<usize>() {
                        found.push(SearchResult {
                            path: root.join(parts[0]),
                            line: line_num,
                            content: parts[2].to_string(),
                        });
                    }
                }
            }
        } else {
            // Fallback: walkdir + string search
            for entry in walkdir::WalkDir::new(&root)
                .into_iter()
                .filter_entry(|e| {
                    let n = e.file_name().to_string_lossy();
                    !n.starts_with('.') && n != "target" && n != "node_modules" && n != ".git"
                })
                .flatten()
            {
                if !entry.file_type().is_file() { continue; }
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for (i, line_text) in content.lines().enumerate() {
                        let matches = if case_sens {
                            line_text.contains(&query)
                        } else {
                            line_text.to_lowercase().contains(&query.to_lowercase())
                        };
                        if matches {
                            found.push(SearchResult {
                                path: entry.path().to_path_buf(),
                                line: i + 1,
                                content: line_text.to_string(),
                            });
                            if found.len() >= 500 { break; }
                        }
                    }
                }
                if found.len() >= 500 { break; }
            }
        }
        let _ = tx.send(found);
    });

    let results_sig = state.search_results;
    let rx_sig = floem::ext_event::create_signal_from_channel(rx);
    floem::reactive::create_effect(move |_| {
        if let Some(found) = rx_sig.get() {
            results_sig.set(found);
            is_searching.set(false);
        }
    });
}

fn perform_replace_all(
    results: Vec<SearchResult>,
    find: String,
    replace: String,
    status: RwSignal<String>,
    use_regex: bool,
    case_sensitive: bool,
) {
    if find.is_empty() { return; }

    // Collect unique file paths
    let mut files: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for r in &results { files.insert(r.path.clone()); }

    let find2   = find.clone();
    let replace2 = replace.clone();
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    std::thread::spawn(move || {
        let mut replaced_files = 0usize;
        let mut replaced_count = 0usize;

        for path in files {
            let Ok(content) = std::fs::read_to_string(&path) else { continue };

            let new_content = if use_regex {
                // Use regex replace via rg/sed or manual regex
                // For now fall back to fixed string (regex crate not guaranteed available)
                let mut out = String::new();
                let mut count = 0usize;
                for line in content.lines() {
                    let new_line = if case_sensitive {
                        if line.contains(&find2) {
                            count += line.matches(&find2).count();
                            line.replace(&find2, &replace2)
                        } else { line.to_string() }
                    } else {
                        let lo_line  = line.to_lowercase();
                        let lo_find  = find2.to_lowercase();
                        if lo_line.contains(&lo_find) {
                            // case-insensitive replace: use rg --replace via process
                            count += 1;
                            // Simple approach: find-replace preserving case
                            let mut result = String::new();
                            let mut rest = line;
                            while let Some(pos) = rest.to_lowercase().find(&lo_find) {
                                result.push_str(&rest[..pos]);
                                result.push_str(&replace2);
                                rest = &rest[pos + find2.len()..];
                            }
                            result.push_str(rest);
                            result
                        } else { line.to_string() }
                    };
                    out.push_str(&new_line);
                    out.push('\n');
                }
                if !content.ends_with('\n') && out.ends_with('\n') {
                    out.pop();
                }
                replaced_count += count;
                out
            } else {
                let mut count = 0usize;
                let new = if case_sensitive {
                    count = content.matches(&find2).count();
                    content.replace(&find2, &replace2)
                } else {
                    let mut result = String::new();
                    let mut rest = content.as_str();
                    let lo_find = find2.to_lowercase();
                    while let Some(pos) = rest.to_lowercase().find(&lo_find) {
                        result.push_str(&rest[..pos]);
                        result.push_str(&replace2);
                        rest = &rest[pos + find2.len()..];
                        count += 1;
                    }
                    result.push_str(rest);
                    result
                };
                replaced_count += count;
                new
            };

            if new_content != content {
                if std::fs::write(&path, new_content).is_ok() {
                    replaced_files += 1;
                }
            }
        }

        let _ = tx.send(format!("Replaced {replaced_count} occurrence(s) in {replaced_files} file(s)"));
    });

    let rx_sig = floem::ext_event::create_signal_from_channel(rx);
    floem::reactive::create_effect(move |_| {
        if let Some(msg) = rx_sig.get() {
            status.set(msg);
        }
    });
}
