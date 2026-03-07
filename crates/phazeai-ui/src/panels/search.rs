use floem::{
    reactive::{create_memo, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};

use crate::app::{IdeState, SearchResult};

/// The search panel — workspace search + multi-file replace.
pub fn search_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let query = state.search_query;
    let results = state.search_results;
    let is_searching = create_rw_signal(false);
    let replace_text = create_rw_signal(String::new());
    let replace_open = create_rw_signal(false);
    let replace_status = create_rw_signal(String::new());
    let use_regex = create_rw_signal(false);
    let case_sensitive = create_rw_signal(false);
    let include_glob = create_rw_signal(String::new());
    let exclude_glob = create_rw_signal(String::new());

    // Tree view toggle and keyboard selection state
    let tree_view: RwSignal<bool> = create_rw_signal(false);
    let selected_idx: RwSignal<Option<usize>> = create_rw_signal(None);

    // ── Header ───────────────────────────────────────────────────────────────
    let header = container(
        stack((
            label(|| "SEARCH").style(move |s| {
                let p = theme.get().palette;
                s.color(p.text_muted)
                    .font_size(11.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .flex_grow(1.0)
            }),
            // Tree view toggle button
            container(label(move || if tree_view.get() { "⊟" } else { "⊞" }))
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(12.0)
                        .padding_horiz(6.0)
                        .padding_vert(2.0)
                        .border_radius(3.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .color(if tree_view.get() {
                            p.accent
                        } else {
                            p.text_muted
                        })
                        .hover(|s| s.background(p.bg_elevated))
                })
                .on_click_stop(move |_| {
                    tree_view.update(|v| *v = !*v);
                    selected_idx.set(None);
                }),
            // Toggle replace mode
            container(label(move || {
                if replace_open.get() {
                    "▾ Replace"
                } else {
                    "▸ Replace"
                }
            }))
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0)
                    .padding_horiz(6.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(p.text_muted)
                    .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                replace_open.update(|v| *v = !*v);
            }),
        ))
        .style(|s| s.flex_row().items_center().width_full().gap(4.0)),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.padding_horiz(12.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .width_full()
    });

    // ── Match count label ─────────────────────────────────────────────────────
    let match_count = create_memo(move |_| {
        let r = results.get();
        if r.is_empty() {
            String::new()
        } else {
            let files: std::collections::HashSet<_> = r.iter().map(|x| &x.path).collect();
            format!("{} results in {} files", r.len(), files.len())
        }
    });
    let count_label = label(move || match_count.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.0)
            .color(p.text_muted)
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .apply_if(match_count.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // ── Search options row ────────────────────────────────────────────────────
    let opt_regex = {
        let t = theme;
        container(label(|| ".*"))
            .style(move |s| {
                let p = t.get().palette;
                s.font_size(11.0)
                    .padding_horiz(6.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(if use_regex.get() {
                        p.bg_base
                    } else {
                        p.text_muted
                    })
                    .background(if use_regex.get() {
                        p.accent
                    } else {
                        p.bg_elevated
                    })
                    .border(1.0)
                    .border_color(p.border)
            })
            .on_click_stop(move |_| {
                use_regex.update(|v| *v = !*v);
            })
    };

    let opt_case = {
        let t = theme;
        container(label(|| "Aa"))
            .style(move |s| {
                let p = t.get().palette;
                s.font_size(11.0)
                    .padding_horiz(6.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(if case_sensitive.get() {
                        p.bg_base
                    } else {
                        p.text_muted
                    })
                    .background(if case_sensitive.get() {
                        p.accent
                    } else {
                        p.bg_elevated
                    })
                    .border(1.0)
                    .border_color(p.border)
            })
            .on_click_stop(move |_| {
                case_sensitive.update(|v| *v = !*v);
            })
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
                        s.flex_grow(1.0)
                            .min_width(0.0)
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
                        if let floem::event::Event::KeyDown(ke) = event {
                            if ke.key.logical_key
                                == floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter)
                            {
                                let root = state2.workspace_root.get();
                                perform_search(
                                    state2.clone(),
                                    is_searching,
                                    root,
                                    use_regex,
                                    case_sensitive,
                                    include_glob,
                                    exclude_glob,
                                );
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

    // ── File glob filters ───────────────────────────────────────────────
    let glob_bar = container(
        stack((
            text_input(include_glob)
                .placeholder("Include: *.rs, src/**")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.flex_grow(1.0)
                        .min_width(0.0)
                        .background(p.bg_elevated)
                        .border(1.0)
                        .border_color(p.border)
                        .border_radius(4.0)
                        .color(p.text_primary)
                        .padding_horiz(6.0)
                        .padding_vert(4.0)
                        .font_size(11.0)
                }),
            text_input(exclude_glob)
                .placeholder("Exclude: target/, *.lock")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.flex_grow(1.0)
                        .min_width(0.0)
                        .background(p.bg_elevated)
                        .border(1.0)
                        .border_color(p.border)
                        .border_radius(4.0)
                        .color(p.text_primary)
                        .padding_horiz(6.0)
                        .padding_vert(4.0)
                        .font_size(11.0)
                }),
        ))
        .style(|s| s.flex_row().gap(4.0).width_full()),
    )
    .style(|s| s.padding_horiz(8.0).padding_bottom(4.0).width_full());

    // ── Replace input (shown when replace_open) ───────────────────────────────
    let replace_bar = {
        let state3 = state.clone();
        container(
            stack((
                text_input(replace_text)
                    .placeholder("Replace with...")
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.flex_grow(1.0)
                            .min_width(0.0)
                            .background(p.bg_elevated)
                            .border(1.0)
                            .border_color(p.border)
                            .border_radius(4.0)
                            .color(p.text_primary)
                            .padding_horiz(8.0)
                            .padding_vert(6.0)
                            .font_size(13.0)
                    }),
                container(label(|| "Replace All"))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.padding_horiz(8.0)
                            .padding_vert(5.0)
                            .border_radius(4.0)
                            .font_size(11.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .color(p.bg_base)
                            .background(p.accent)
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
            .style(|s| s.flex_row().items_center().gap(4.0).width_full()),
        )
        .style(move |s| {
            s.padding_horiz(8.0)
                .padding_vert(4.0)
                .width_full()
                .apply_if(!replace_open.get(), |s| {
                    s.display(floem::style::Display::None)
                })
        })
    };

    // Replace status
    let status_label = label(move || replace_status.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .color(p.success)
            .padding_horiz(12.0)
            .padding_vert(3.0)
            .apply_if(replace_status.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // ── Flat results list (with keyboard selection highlighting) ──────────────
    let flat_results_view = {
        let state_flat = state.clone();
        dyn_stack(
            move || {
                results
                    .get()
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(i, _)| *i,
            move |(i, r)| {
                let path_str = r
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let is_selected = move || selected_idx.get() == Some(i);
                let path = r.path.clone();
                let line = r.line;
                let content_text = r.content.trim().to_string();
                let s = state_flat.clone();
                container(
                    stack((
                        label(move || format!("{}:{}", path_str, r.line + 1)).style(
                            move |s| {
                                let p = theme.get().palette;
                                s.font_size(10.0).color(p.accent).padding_right(6.0)
                            },
                        ),
                        label(move || content_text.clone()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(11.0).color(p.text_primary).flex_grow(1.0)
                        }),
                    ))
                    .style(|s| s.flex_row().items_center()),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.padding_horiz(8.0)
                        .padding_vert(3.0)
                        .width_full()
                        .cursor(floem::style::CursorStyle::Pointer)
                        .apply_if(is_selected(), |s| s.background(p.bg_elevated))
                        .hover(|s| s.background(p.bg_elevated))
                })
                .on_click_stop(move |_| {
                    selected_idx.set(Some(i));
                    s.open_file.set(Some(path.clone()));
                    s.goto_line.set(line as u32 + 1);
                })
            },
        )
        .style(|s: floem::style::Style| s.flex_col().width_full())
    };

    // ── Tree results list (grouped by file) ───────────────────────────────────
    let grouped_results: floem::reactive::Memo<Vec<(std::path::PathBuf, Vec<SearchResult>)>> =
        create_memo(move |_| {
            let all = results.get();
            let mut map: std::collections::BTreeMap<std::path::PathBuf, Vec<SearchResult>> =
                std::collections::BTreeMap::new();
            for r in all {
                map.entry(r.path.clone()).or_default().push(r);
            }
            map.into_iter().collect()
        });

    let tree_results_view = {
        let state4 = state.clone();
        dyn_stack(
            move || {
                // Flatten: for each file group, emit a header item then each match item.
                let mut items: Vec<(String, Option<SearchResult>)> = Vec::new();
                for (path, matches) in grouped_results.get() {
                    let _fname = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    items.push((format!("__file__{}", path.display()), None));
                    for m in matches {
                        items.push((
                            format!("{}:{}:{}", m.path.display(), m.line, m.content),
                            Some(m),
                        ));
                    }
                }
                items
            },
            |(key, _)| key.clone(),
            {
                let state5 = state4.clone();
                move |(key, result_opt): (String, Option<SearchResult>)| {
                    if key.starts_with("__file__") {
                        // File header row
                        let file_path = key.strip_prefix("__file__").unwrap_or("").to_string();
                        let display_name = std::path::Path::new(&file_path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| file_path.clone());
                        let match_count_for_file = grouped_results
                            .get()
                            .iter()
                            .find(|(p, _)| p.display().to_string() == file_path)
                            .map(|(_, v)| v.len())
                            .unwrap_or(0);
                        container(
                            stack((
                                label(move || format!("📄 {display_name}")),
                                label(move || format!("  ({match_count_for_file})")),
                            ))
                            .style(|s| s.items_center()),
                        )
                        .style(move |s| {
                            let p = theme.get().palette;
                            s.width_full()
                                .padding_horiz(8.0)
                                .padding_vert(5.0)
                                .font_size(12.0)
                                .color(p.accent)
                                .font_weight(floem::text::Weight::BOLD)
                                .background(p.bg_elevated.with_alpha(0.3))
                                .border_bottom(1.0)
                                .border_color(p.border.with_alpha(0.2))
                        })
                        .into_any()
                    } else if let Some(res) = result_opt {
                        // Match result row (indented)
                        let path = res.path.clone();
                        let line = res.line;
                        let content = res.content.trim().to_string();
                        let hovered = create_rw_signal(false);
                        let s = state5.clone();

                        container(
                            stack((
                                label(move || format!("  L{line}: ")).style(move |s| {
                                    s.font_size(11.0)
                                        .color(theme.get().palette.text_muted)
                                        .min_width(50.0)
                                }),
                                label(move || content.clone()).style(move |s| {
                                    s.font_size(12.0)
                                        .color(theme.get().palette.text_primary)
                                        .flex_grow(1.0)
                                }),
                            ))
                            .style(|s| s.items_center()),
                        )
                        .style(move |_s| {
                            let p = theme.get().palette;
                            _s.width_full()
                                .padding_left(20.0)
                                .padding_right(8.0)
                                .padding_vert(3.0)
                                .background(if hovered.get() {
                                    p.bg_elevated
                                } else {
                                    floem::peniko::Color::TRANSPARENT
                                })
                                .cursor(floem::style::CursorStyle::Pointer)
                        })
                        .on_click_stop(move |_| {
                            s.open_file.set(Some(path.clone()));
                            s.goto_line.set(line as u32);
                        })
                        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                            hovered.set(true);
                        })
                        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                            hovered.set(false);
                        })
                        .into_any()
                    } else {
                        container(label(|| "")).into_any()
                    }
                }
            },
        )
        .style(|s: floem::style::Style| s.flex_col().width_full())
    };

    // ── Status / searching label ──────────────────────────────────────────────
    let searching_label = label(move || {
        if is_searching.get() {
            "Searching...".to_string()
        } else if results.get().is_empty() && !query.get().is_empty() {
            "No results found.".to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| {
        s.font_size(12.0)
            .color(theme.get().palette.text_muted)
            .padding(12.0)
            .apply_if(
                !is_searching.get()
                    && (results.get().is_empty() && query.get().is_empty()
                        || !results.get().is_empty()),
                |s| s.display(floem::style::Display::None),
            )
    });

    // ── Combine flat + tree into a conditional container ─────────────────────
    // We wrap both in a container and show/hide based on tree_view signal.
    let flat_container = container(
        stack((flat_results_view, searching_label))
            .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.flex_col()
            .width_full()
            .apply_if(tree_view.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let tree_container = container(tree_results_view).style(move |s| {
        s.flex_col()
            .width_full()
            .apply_if(!tree_view.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let results_inner = scroll(
        stack((flat_container, tree_container)).style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full())
    .keyboard_navigable();

    // ── Keyboard navigation wrapper ───────────────────────────────────────────
    let results_area = container(results_inner)
        .on_event_stop(
            floem::event::EventListener::KeyDown,
            move |event| {
                if let floem::event::Event::KeyDown(e) = event {
                    use floem::keyboard::NamedKey;
                    let total = results.get().len();
                    match e.key.logical_key {
                        floem::keyboard::Key::Named(NamedKey::ArrowDown) => {
                            selected_idx.update(|i| {
                                *i = Some(match *i {
                                    None => 0,
                                    Some(n) => (n + 1).min(total.saturating_sub(1)),
                                });
                            });
                        }
                        floem::keyboard::Key::Named(NamedKey::ArrowUp) => {
                            selected_idx.update(|i| {
                                *i = Some(match *i {
                                    None => 0,
                                    Some(n) => n.saturating_sub(1),
                                });
                            });
                        }
                        floem::keyboard::Key::Named(NamedKey::Enter) => {
                            if let Some(idx) = selected_idx.get() {
                                if let Some(r) = results.get().get(idx).cloned() {
                                    state.open_file.set(Some(r.path.clone()));
                                    state.goto_line.set(r.line as u32 + 1);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
        )
        .keyboard_navigable()
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((
        header,
        count_label,
        search_bar,
        glob_bar,
        replace_bar,
        status_label,
        results_area,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width_full()
            .height_full()
            .background(p.bg_panel)
    })
}

fn perform_search(
    state: IdeState,
    is_searching: RwSignal<bool>,
    root: std::path::PathBuf,
    use_regex: RwSignal<bool>,
    case_sensitive: RwSignal<bool>,
    include_glob: RwSignal<String>,
    exclude_glob: RwSignal<String>,
) {
    let query = state.search_query.get();
    if query.is_empty() {
        return;
    }

    is_searching.set(true);
    state.search_results.set(vec![]);

    let regex = use_regex.get();
    let case_sens = case_sensitive.get();
    let include = include_glob.get();
    let exclude = exclude_glob.get();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut found = Vec::new();

        let mut rg_args = vec![
            "--line-number".to_string(),
            "--no-heading".to_string(),
            "--color=never".to_string(),
        ];
        if !case_sens {
            rg_args.push("--ignore-case".to_string());
        }
        if !regex {
            rg_args.push("--fixed-strings".to_string());
        }
        // Apply glob filters
        for part in include
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            rg_args.push("--glob".to_string());
            rg_args.push(part.to_string());
        }
        for part in exclude
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            rg_args.push("--glob".to_string());
            rg_args.push(format!("!{part}"));
        }
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
                if !entry.file_type().is_file() {
                    continue;
                }
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
                            if found.len() >= 500 {
                                break;
                            }
                        }
                    }
                }
                if found.len() >= 500 {
                    break;
                }
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
    if find.is_empty() {
        return;
    }

    // Collect unique file paths
    let mut files: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for r in &results {
        files.insert(r.path.clone());
    }

    let find2 = find.clone();
    let replace2 = replace.clone();
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    std::thread::spawn(move || {
        let mut replaced_files = 0usize;
        let mut replaced_count = 0usize;

        for path in files {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };

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
                        } else {
                            line.to_string()
                        }
                    } else {
                        let lo_line = line.to_lowercase();
                        let lo_find = find2.to_lowercase();
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
                        } else {
                            line.to_string()
                        }
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

        let _ = tx.send(format!(
            "Replaced {replaced_count} occurrence(s) in {replaced_files} file(s)"
        ));
    });

    let rx_sig = floem::ext_event::create_signal_from_channel(rx);
    floem::reactive::create_effect(move |_| {
        if let Some(msg) = rx_sig.get() {
            status.set(msg);
        }
    });
}
