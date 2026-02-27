use floem::{
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};

use crate::{
    app::{IdeState, SearchResult},
};

/// The search panel.
pub fn search_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let query = state.search_query;
    let results = state.search_results;
    let is_searching = create_rw_signal(false);

    // Header
    let header = container(
        label(|| "SEARCH")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.color(p.text_muted)
                 .font_size(11.0)
                 .font_weight(floem::text::Weight::BOLD)
            }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(12.0)
         .padding_vert(8.0)
         .border_bottom(1.0)
         .border_color(p.border)
         .width_full()
    });

    // Search input
    let search_bar = {
        let state = state.clone();
        container(
            text_input(query)
                .placeholder("Search in files...")
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.width_full()
                     .background(p.bg_elevated)
                     .border(1.0)
                     .border_color(p.border)
                     .border_radius(4.0)
                     .color(p.text_primary)
                     .padding_horiz(8.0)
                     .padding_vert(6.0)
                     .font_size(13.0)
                })
                .on_event_stop(floem::event::EventListener::KeyDown, move |event| {
                    if let floem::event::Event::KeyDown(key_event) = event {
                        if key_event.key.logical_key == floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter) {
                            let root = state.workspace_root.get();
                            perform_search(state.clone(), is_searching, root);
                        }
                    }
                })
        )
    };

    // Results list
    let results_view = {
        let state = state; // moved
        dyn_stack(
            move || results.get(),
            |res| format!("{}:{}:{}", res.path.display(), res.line, res.content),
            {
                let state = state.clone();
                move |res: SearchResult| {
                    let path = res.path.clone();
                    let line = res.line;
                    let content = res.content.trim().to_string();
                    let is_hovered = create_rw_signal(false);
                    let state = state.clone();
                    
                    let filename = path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    container(
                        stack((
                            label(move || format!("{}:{} ", filename, line))
                                .style(move |s| {
                                    let t = theme.get();
                                    let p = &t.palette;
                                    s.font_size(12.0).color(p.accent).margin_right(4.0)
                                }),
                            label(move || content.clone())
                                .style(move |s| {
                                    let t = theme.get();
                                    let p = &t.palette;
                                    s.font_size(12.0).color(p.text_primary).flex_grow(1.0)
                                }),
                        ))
                        .style(|s| s.items_center())
                    )
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        let hovered = is_hovered.get();
                        s.width_full()
                         .padding_horiz(12.0)
                         .padding_vert(4.0)
                         .background(if hovered { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                         .cursor(floem::style::CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| {
                        state.open_file.set(Some(path.clone()));
                        // TODO: Jump to line logic in editor
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                        is_hovered.set(true);
                    })
                    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                        is_hovered.set(false);
                    })
                }
            }
        )
    }
    .style(|s: floem::style::Style| s.flex_col().width_full());

    let results_scroll = scroll(stack((
        results_view,
        // Status indicator
        label(move || {
            if is_searching.get() { "Searching..." }
            else if results.get().is_empty() && !query.get().is_empty() { "No results found." }
            else { "" }
        })
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(12.0).color(p.text_muted).padding(12.0)
        })
    )))
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((header, search_bar, results_scroll))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_col()
             .width_full()
             .height_full()
             .background(p.bg_panel)
        })
}

fn perform_search(state: IdeState, is_searching: RwSignal<bool>, root: std::path::PathBuf) {
    let query = state.search_query.get();
    if query.is_empty() { return; }

    is_searching.set(true);
    let (tx, rx) = std::sync::mpsc::channel();

    // Spawn background search
    std::thread::spawn(move || {
        let mut found = Vec::new();

        // Try ripgrep first
        let rg_ok = std::process::Command::new("rg")
            .args([
                "--line-number",
                "--no-heading",
                "--color=never",
                "--max-count=1",
                "--max-depth=50",
                "-e", &query,
            ])
            .current_dir(&root)
            .output();

        if let Ok(output) = rg_ok {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().take(200) {
                // rg format: path:line_num:content
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
            // Fallback: walkdir + contains
            for entry in walkdir::WalkDir::new(&root)
                .into_iter()
                .filter_entry(|e: &walkdir::DirEntry| {
                    let name = e.file_name().to_string_lossy();
                    !name.starts_with('.') && name != "target" && name != "node_modules"
                })
                .flatten()
            {
                if entry.file_type().is_file() {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        for (i, line) in content.lines().enumerate() {
                            if line.to_lowercase().contains(&query.to_lowercase()) {
                                found.push(SearchResult {
                                    path: entry.path().to_path_buf(),
                                    line: i + 1,
                                    content: line.to_string(),
                                });
                                if found.len() >= 200 { break; }
                            }
                        }
                    }
                }
                if found.len() >= 200 { break; }
            }
        }

        let _ = tx.send(found);
    });

    // Handle results via effect
    let results_sig = state.search_results;
    let rx_sig = floem::ext_event::create_signal_from_channel(rx);
    floem::reactive::create_effect(move |_| {
        if let Some(found) = rx_sig.get() {
            results_sig.set(found);
            is_searching.set(false);
        }
    });
}
