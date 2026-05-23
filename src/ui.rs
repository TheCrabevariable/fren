use std::{io, os::unix::fs::PermissionsExt, path::PathBuf};

use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use ratatui_image::Image;
use std::sync::atomic::Ordering;
use unicode_width::UnicodeWidthStr;

use crate::app::ImageKey;
use crate::app::PreviewJob;
use crate::app::quantize;
use crate::app::{App, AppMode, ClipboardMode, Focus, InputAction};
use crate::config::Config;
use crate::theme::Theme;

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let size = bytes as f64;

    if size < KB {
        format!("{} B", bytes)
    } else if size < MB {
        format!("{:.2} KB", size / KB)
    } else if size < GB {
        format!("{:.2} MB", size / MB)
    } else {
        format!("{:.2} GB", size / GB)
    }
}

fn format_permissions(mode: u32) -> String {
    let flags = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];

    flags
        .iter()
        .map(|(bit, ch)| if mode & bit != 0 { *ch } else { '-' })
        .collect()
}

pub fn draw_ui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    config: &Config,
    theme: &Theme,
) -> io::Result<()> {
    terminal.draw(|f| {
        let area = f.area();

        let bg_block = Block::default().style(Style::default().bg(theme.background));
        f.render_widget(bg_block, area);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        //
        // HEADER
        //
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "[Fren] ",
                Style::default()
                    .fg(theme.focus_border)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.current_dir.display().to_string()),
        ]))
        .style(Style::default().bg(theme.background).fg(theme.foreground));

        f.render_widget(header, vertical[0]);

        //
        // MAIN COLUMNS
        //
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(30),
                Constraint::Percentage(50),
            ])
            .split(vertical[1]);

        let _preview_area = columns[2];

        let left_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(56),
                Constraint::Percentage(14),
                Constraint::Percentage(30),
            ])
            .split(columns[0]);

        let middle_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(columns[1]);

        //
        // SIDEBAR (Pinned + Storage)
        //
        let pinned_focused = app.focus == Focus::Pinned;
        let storage_focused = app.focus == Focus::Storage;

        // Pinned list
        let mut pinned_items: Vec<ListItem> = Vec::new();
        for p in &app.pinned {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("home")
                .to_string();
            pinned_items.push(
                ListItem::new(name).style(Style::default().fg(if pinned_focused {
                    theme.foreground
                } else {
                    theme.muted
                })),
            );
        }

        let mut pinned_state = ListState::default();
        if !app.pinned.is_empty() {
            pinned_state.select(Some(app.pinned_selected));
        }

        let pinned_list = List::new(pinned_items)
            .block(
                Block::default()
                    .title(Span::styled(
                        " Pinned ",
                        Style::default()
                            .fg(if pinned_focused {
                                theme.focus_border
                            } else {
                                theme.muted
                            })
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(theme.focus_border)
                    .fg(theme.background)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" ");

        f.render_stateful_widget(pinned_list, left_chunks[0], &mut pinned_state);

        // Storage list
        let mut storage_items: Vec<ListItem> = Vec::new();
        for p in &app.storage {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("drive")
                .to_string();
            storage_items.push(ListItem::new(name).style(Style::default().fg(
                if storage_focused {
                    theme.foreground
                } else {
                    theme.muted
                },
            )));
        }

        let mut storage_state = ListState::default();
        if !app.storage.is_empty() {
            storage_state.select(Some(app.storage_selected));
        }

        let storage_list = List::new(storage_items)
            .block(
                Block::default()
                    .title(Span::styled(
                        " Storage ",
                        Style::default()
                            .fg(if storage_focused {
                                theme.focus_border
                            } else {
                                theme.muted
                            })
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(theme.focus_border)
                    .fg(theme.background)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" ");

        f.render_stateful_widget(storage_list, left_chunks[1], &mut storage_state);

        //
        // CLIPBOARD
        //
        let clipboard_focused = app.focus == Focus::Clipboard;

        let mut clipboard_items: Vec<ListItem> = Vec::new();
        for (path, mode) in &app.clipboard {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");
            let label = match mode {
                ClipboardMode::Copy => format!("C: {}", name),
                ClipboardMode::Cut => format!("X: {}", name),
            };
            clipboard_items.push(ListItem::new(label).style(Style::default().fg(
                if clipboard_focused {
                    theme.foreground
                } else {
                    theme.muted
                },
            )));
        }

        if clipboard_items.is_empty() {
            clipboard_items.push(ListItem::new("Empty").style(Style::default().fg(theme.muted)));
        }

        let mut clip_state = ListState::default();
        let clip_idx = if app.clipboard.is_empty() {
            None
        } else {
            Some(app.clipboard_selected.min(app.clipboard.len() - 1))
        };
        clip_state.select(clip_idx);

        let clipboard_list = List::new(clipboard_items)
            .block(
                Block::default()
                    .title(Span::styled(
                        " Clipboard ",
                        Style::default()
                            .fg(if clipboard_focused {
                                theme.focus_border
                            } else {
                                theme.muted
                            })
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(theme.focus_border)
                    .fg(theme.background)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" ");

        f.render_stateful_widget(clipboard_list, left_chunks[2], &mut clip_state);

        //
        // FILES
        //
        let files_focused = app.focus == Focus::Files;

        let items: Vec<ListItem> = app
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();

                let icon = App::icon_for(&path, app.icon_mode);

                let base_color = if path.is_dir() {
                    theme.directory
                } else {
                    theme.foreground
                };

                let color = if files_focused {
                    base_color
                } else {
                    theme.muted
                };

                let sel = if app.selected_indices.contains(&i) {
                    "* "
                } else {
                    "  "
                };

                let line = Line::from(vec![
                    Span::raw(sel),
                    Span::styled(icon, Style::default().fg(theme.muted)),
                    Span::styled(name, Style::default().fg(color)),
                ]);

                ListItem::new(line)
            })
            .collect();

        let mut state = ListState::default();
        state.select(Some(app.selected));

        let list = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(
                        " Files ",
                        Style::default()
                            .fg(if files_focused {
                                theme.focus_border
                            } else {
                                theme.muted
                            })
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            )
            .highlight_style(
                Style::default()
                    .bg(theme.focus_border)
                    .fg(theme.background)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" ");

        f.render_stateful_widget(list, middle_chunks[0], &mut state);
        //
        //metadata
        //
        let metadata_block = Block::default()
            .title(Span::styled(
                " Metadata ",
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border));

        let metadata_area = middle_chunks[1];

        let metadata_lines: Vec<Line> = if let Some(entry) = app.entries.get(app.selected) {
            if app.selected != app.meta_selected {
                app.meta_cache.clear();
                let path = entry.path();

                if let Ok(meta) = std::fs::symlink_metadata(&path) {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown");

                    let file_type = if meta.file_type().is_symlink() {
                        "Symlink"
                    } else if meta.is_dir() {
                        "Directory"
                    } else if meta.is_file() {
                        "File"
                    } else {
                        "Other"
                    };

                    let size = if meta.is_file() {
                        format_size(meta.len())
                    } else if meta.is_dir() {
                        "dir".to_string()
                    } else {
                        "-".to_string()
                    };

                    let modified = meta
                        .modified()
                        .ok()
                        .map(|time| {
                            let datetime: chrono::DateTime<chrono::Local> = time.into();
                            datetime.format("%Y-%m-%d %H:%M:%S").to_string()
                        })
                        .unwrap_or_else(|| "Unknown".to_string());

                    let mode = meta.permissions().mode();
                    let perms = format_permissions(mode);
                    let octal = format!("{:o}", mode & 0o777);

                    app.meta_cache.push(("Name".into(), file_name.to_string()));
                    app.meta_cache.push(("Type".into(), file_type.to_string()));
                    if meta.is_file()
                        && let Some((w, h)) = crate::app::get_dimensions(&path)
                    {
                        app.meta_cache
                            .push(("Resolution".into(), format!("{}x{}", w, h)));
                    }
                    app.meta_cache.push(("Size".into(), size));
                    app.meta_cache
                        .push(("Perms".into(), format!("{} ({})", perms, octal)));
                    app.meta_cache.push(("Modified".into(), modified));
                    app.meta_cache
                        .push(("Path".into(), path.display().to_string()));
                } else {
                    app.meta_cache
                        .push(("Error".into(), "Unable to read metadata".into()));
                }

                app.meta_selected = app.selected;
            }

            let mut lines: Vec<Line> = Vec::with_capacity(app.meta_cache.len() + 1);
            for (label, value) in &app.meta_cache {
                if label == "Error" {
                    lines.push(Line::from(Span::styled(
                        value,
                        Style::default().fg(theme.muted),
                    )));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{:12}", label), Style::default().fg(theme.muted)),
                        Span::styled(value.clone(), Style::default().fg(theme.foreground)),
                    ]));
                }
            }
            lines
        } else {
            vec![Line::from(Span::styled(
                "No file selected",
                Style::default().fg(theme.muted),
            ))]
        };

        let metadata = Paragraph::new(metadata_lines)
            .style(Style::default().bg(theme.background).fg(theme.foreground))
            .block(metadata_block)
            .wrap(Wrap { trim: true });

        f.render_widget(metadata, metadata_area);

        //
        // PREVIEW PANEL
        //

        let preview_block = Block::default().title(" Preview ").borders(Borders::ALL);

        f.render_widget(preview_block.clone(), columns[2]);
        let inner = preview_block.inner(columns[2]);

        //
        // 🔥 POLL ASYNC IMAGE RESULT
        //
        if let Some(rx) = &app.image_rx {
            while let Ok((id, result)) = rx.try_recv() {
                if id == app.image_request_id {
                    if result.is_none()
                        && let Some(path) = &app.image_path
                    {
                        app.preview_failed.insert(path.clone());
                    }
                    app.image = result;
                    app.image_loading = false;
                }
            }
        }

        if let Some(entry) = app.entries.get(app.selected) {
            let path: PathBuf = entry.path();

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif");

            let is_pdf = ext == "pdf";

            //
            // 🖼 IMAGE / PDF PREVIEW
            //
            if (is_image || is_pdf) && path.is_file() {
                let key = ImageKey {
                    path: path.clone(),
                    width: quantize(inner.width),
                    height: quantize(inner.height),
                };

                if let Some(cached) = app.image_cache.lock().unwrap().get(&key).cloned() {
                    app.image = Some(cached);
                    app.image_loading = false;
                    app.image_path = Some(path.clone());
                    app.image_size = Some((inner.width, inner.height));
                }

                let size_changed = app.image_size != Some((inner.width, inner.height));
                let path_changed = app.image_path.as_ref() != Some(&path);
                let reload = size_changed || path_changed;

                let failed = app.preview_failed.contains(&path);

                if reload && !app.image_loading && !failed {
                    if inner.width < 10 || inner.height < 5 {
                        let loading = Paragraph::new("…").alignment(Alignment::Center);
                        f.render_widget(loading, inner);
                        return;
                    }

                    // Try synchronous cache path first (avoids worker round-trip)
                    if !is_pdf && let Some(protocol) = app.try_protocol_from_cache(&path, inner) {
                        let key = ImageKey {
                            path: path.clone(),
                            width: quantize(inner.width),
                            height: quantize(inner.height),
                        };
                        app.image_cache.lock().unwrap().put(key, protocol.clone());
                        app.image = Some(protocol);
                        app.image_loading = false;
                        app.image_path = Some(path.clone());
                        app.image_size = Some((inner.width, inner.height));
                    } else {
                        app.image_request_id = app.image_request_id.wrapping_add(1);
                        let request_id = app.image_request_id;
                        app.image_request_atomic
                            .store(request_id, Ordering::Relaxed);

                        app.image = None;

                        app.image_size = Some((inner.width, inner.height));
                        app.image_path = Some(path.clone());
                        app.image_loading = true;

                        app.preview_job_tx
                            .send(PreviewJob {
                                request_id,
                                path: path.clone(),
                                inner,
                                is_pdf,
                            })
                            .ok();
                    }
                }

                if let Some(img) = &app.image {
                    let widget = Image::new(img);
                    f.render_widget(widget, inner);
                } else if failed {
                    let no_preview =
                        Paragraph::new("No preview available").alignment(Alignment::Center);
                    f.render_widget(no_preview, inner);
                } else {
                    let loading = Paragraph::new("Loading preview…").alignment(Alignment::Center);
                    f.render_widget(loading, inner);
                }
            } else {
                //
                // 📄 TEXT PREVIEW
                //
                app.image = None;
                app.image_path = None;
                app.image_loading = false;
                app.image_size = None;

                let is_binary_ext = matches!(
                    ext.as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "webp"
                        | "gif"
                        | "mp3"
                        | "wav"
                        | "flac"
                        | "mp4"
                        | "mkv"
                        | "mov"
                        | "zip"
                        | "tar"
                        | "gz"
                        | "rar"
                        | "exe"
                        | "bin"
                        | "so"
                        | "pdf"
                );

                let is_probably_text = !is_binary_ext;

                //
                // 📁 DIRECTORY / TEXT / FALLBACK PREVIEW (FIXED)
                //

                if path.is_dir() {
                    use std::fs;

                    let mut lines = Vec::new();

                    match fs::read_dir(&path) {
                        Ok(read_dir) => {
                            let mut items: Vec<_> = read_dir
                                .flatten()
                                .filter(|e| {
                                    if let Some(name) = e.file_name().to_str()
                                        && !app.show_hidden
                                        && name.starts_with('.')
                                    {
                                        return false;
                                    }
                                    true
                                })
                                .collect();

                            items.sort_by(|a, b| {
                                use std::cmp::Ordering;

                                let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
                                let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);

                                if a_dir != b_dir {
                                    return if a_dir {
                                        Ordering::Less
                                    } else {
                                        Ordering::Greater
                                    };
                                }

                                a.file_name().cmp(&b.file_name())
                            });

                            for entry in items.into_iter().take(inner.height as usize) {
                                let name = entry.file_name().to_string_lossy().to_string();
                                let icon = App::icon_for(&entry.path(), app.icon_mode);
                                lines.push(format!("{}{}", icon, name));
                            }

                            if lines.is_empty() {
                                lines.push("(empty directory)".into());
                            }
                        }
                        Err(_) => {
                            lines.push("Unable to read directory".into());
                        }
                    }

                    let preview = Paragraph::new(lines.join("\n")).wrap(Wrap { trim: false });

                    f.render_widget(preview, inner);
                } else if is_probably_text && path.is_file() {
                    let content = std::fs::read_to_string(&path)
                        .map(|s| {
                            s.lines()
                                .take(inner.height as usize)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_else(|_| "Unable to read file".to_string());

                    let preview = Paragraph::new(content).wrap(Wrap { trim: false });

                    f.render_widget(preview, inner);
                } else {
                    let preview = Paragraph::new("No preview available")
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: false });

                    f.render_widget(preview, inner);
                }
            }
        }

        //
        // STATUS BAR
        //
        let status_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Min(20)])
            .split(vertical[2]);

        let dir_count = app.entries.iter().filter(|e| e.path().is_dir()).count();
        let file_count = app.entries.len().saturating_sub(dir_count);
        let sel_count = app.selected_indices.len();
        let counts = if sel_count > 0 {
            format!("{}s {}f {}d  ", sel_count, file_count, dir_count)
        } else {
            format!("{}f {}d  ", file_count, dir_count)
        };

        let left_status = Paragraph::new(Line::from(vec![
            Span::styled(
                "[Fren] ",
                Style::default()
                    .fg(theme.focus_border)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("/: help "),
        ]))
        .style(Style::default().bg(theme.status_bg).fg(theme.status_fg));

        f.render_widget(left_status, status_chunks[0]);

        let right_status = Paragraph::new(Line::from(vec![
            Span::styled(counts, Style::default().fg(theme.status_fg)),
            Span::styled(
                format!("Sort: {:?}", app.sort_mode),
                Style::default()
                    .fg(theme.focus_border)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.status_bg).fg(theme.status_fg));

        f.render_widget(right_status, status_chunks[1]);

        //
        // INPUT MODAL
        //
        if let AppMode::Input(action) = &app.mode {
            render_dim_overlay(f, area, theme);

            let is_open_with = matches!(action, InputAction::OpenWith);
            let has_quick = is_open_with && !config.quick_apps.is_empty();
            let popup_height = if has_quick { 35 } else { 20 };

            let popup_area = centered_rect(60, popup_height, area);

            let title_text = match action {
                InputAction::Rename => " Rename ",
                InputAction::CreateFile => " Create File ",
                InputAction::CreateFolder => " Create Folder ",
                InputAction::ConfirmDelete => " Confirm Delete ",
                InputAction::OpenWith => " Open With ",
                InputAction::GoTo => " Go To Path ",
            };

            let inner_block = Block::default()
                .title(Span::styled(
                    title_text,
                    Style::default()
                        .fg(theme.focus_border)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border));

            let inner_area = inner_block.inner(popup_area);

            let input = Paragraph::new(app.input.as_str())
                .style(Style::default().fg(theme.foreground).bg(theme.background));

            f.render_widget(Clear, popup_area);
            f.render_widget(inner_block, popup_area);
            f.render_widget(input, inner_area);

            let cursor_x = inner_area.x
                + UnicodeWidthStr::width(&app.input[..app.input_cursor.min(app.input.len())])
                    as u16;
            f.set_cursor_position((cursor_x, inner_area.y));

            if has_quick {
                let list_y = inner_area.y + 2;
                let list_area = Rect {
                    x: inner_area.x,
                    y: list_y,
                    width: inner_area.width,
                    height: inner_area.height.saturating_sub(2),
                };

                let mut quick_items: Vec<ListItem> = Vec::new();
                for (i, qa) in config.quick_apps.iter().enumerate() {
                    let is_sel = i == app.quick_app_selected;
                    let style = if is_sel {
                        Style::default()
                            .bg(theme.focus_border)
                            .fg(theme.background)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.foreground)
                    };
                    quick_items.push(ListItem::new(qa.name.as_str()).style(style));
                }

                let mut list_state = ListState::default();
                list_state.select(Some(
                    app.quick_app_selected
                        .min(config.quick_apps.len().saturating_sub(1)),
                ));

                let quick_list = List::new(quick_items).block(
                    Block::default()
                        .title(Span::styled(
                            " Quick Apps ",
                            Style::default()
                                .fg(theme.focus_border)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.border)),
                );

                f.render_stateful_widget(quick_list, list_area, &mut list_state);
            }
        }

        //
        // CONFLICT DIALOG
        //
        if let AppMode::Conflict(state) = &app.mode {
            render_dim_overlay(f, area, theme);

            let popup_area = centered_rect(50, 25, area);

            let (source, _dest, _mode) = &state.pending[state.index];
            let name = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");

            let lines = vec![
                Line::from(Span::raw("")),
                Line::from(Span::styled(
                    format!(" \"{}\" already exists", name),
                    Style::default().fg(theme.foreground),
                )),
                Line::from(Span::raw("")),
                Line::from(Span::styled(
                    "  [S]kip  [R]eplace  re[n]ame  [Esc] cancel",
                    Style::default().fg(theme.muted),
                )),
                Line::from(Span::raw("")),
            ];

            let dialog = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(Span::styled(
                            " File Conflict ",
                            Style::default()
                                .fg(theme.focus_border)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.border)),
                )
                .alignment(Alignment::Center);

            f.render_widget(Clear, popup_area);
            f.render_widget(dialog, popup_area);
        }

        if app.show_help {
            draw_help_popup(f, area, config, theme);
        }
    })?;

    Ok(())
}

//
// Dim overlay
//
fn render_dim_overlay(f: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let overlay = Block::default().style(
        Style::default()
            .bg(theme.background)
            .add_modifier(Modifier::DIM),
    );

    f.render_widget(overlay, area);
}

//
// Help popup
//
fn draw_help_popup(f: &mut ratatui::Frame, area: Rect, config: &Config, theme: &Theme) {
    render_dim_overlay(f, area, theme);

    let help_text = vec![
        Line::from(Span::styled(
            "Fren Keybindings",
            Style::default()
                .fg(theme.focus_border)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Open               : {}", config.keymaps.open)),
        Line::from(format!("Delete             : {}", config.keymaps.trash)),
        Line::from(format!(
            "Create file        : {}",
            config.keymaps.create_file
        )),
        Line::from(format!(
            "Create folder      : {}",
            config.keymaps.create_folder
        )),
        Line::from(format!("Rename             : {}", config.keymaps.rename)),
        Line::from(format!("Copy               : {}", config.keymaps.copy)),
        Line::from(format!("Cut                : {}", config.keymaps.cut)),
        Line::from(format!("Paste              : {}", config.keymaps.paste)),
        Line::from(format!(
            "Toggle hidden      : {}",
            config.keymaps.toggle_hidden
        )),
        Line::from(format!("Pin                : {}", config.keymaps.pin)),
        Line::from(format!("Unpin              : {}", config.keymaps.unpin)),
        Line::from(format!("Sorting mode       : {}", config.keymaps.sort)),
        Line::from(format!("Go to path         : {}", config.keymaps.go_to)),
        Line::from(format!(
            "Select/Deselect   : {}",
            if config.keymaps.toggle_select == " " {
                "Space".to_string()
            } else {
                config.keymaps.toggle_select.clone()
            }
        )),
        Line::from(format!("Focus switch       : {}", config.keymaps.focus)),
        Line::from(format!("Quit               : {}", config.keymaps.quit)),
        Line::from(""),
        Line::from(Span::styled(
            "Press ESC to close",
            Style::default().fg(theme.muted),
        )),
    ];

    let max_width = help_text
        .iter()
        .map(|line| line.to_string().width() as u16)
        .max()
        .unwrap_or(0)
        + 4;

    let height = help_text.len() as u16 + 2;

    let popup_area = Rect {
        x: area.x + (area.width.saturating_sub(max_width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width: max_width.min(area.width),
        height: height.min(area.height),
    };

    let paragraph = Paragraph::new(help_text)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .block(
            Block::default()
                .title(Span::styled(
                    " Help ",
                    Style::default()
                        .fg(theme.focus_border)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        );

    f.render_widget(Clear, popup_area);
    f.render_widget(paragraph, popup_area);
}

//
// Centered rect
//
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
