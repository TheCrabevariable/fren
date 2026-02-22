use std::{
    io, os::unix::fs::PermissionsExt,
    path::Path,
    path::PathBuf,
    io::{stdout, Write},
    thread,
};

use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect, Alignment},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap, Widget},
};

use ratatui_image::{StatefulImage, Resize, Image};
use ratatui_image::protocol::Protocol;
use image::io::Reader as ImageReader;
use image::imageops::FilterType;
use std::sync::mpsc;
use std::sync::atomic::Ordering;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, AppMode, ClipboardMode, Focus, InputAction};
use crate::config::Config;
use crate::theme::Theme;
use crate::app::ImageKey;
use crate::app::{IconMode};
use crate::app::quantize;
use crate::app::PreviewJob;

//
// Human readable size
//
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

//
// Human readable permissions
//
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

//
// UI Rendering
//
pub fn draw_ui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    config: &Config,
    theme: &Theme,
) -> io::Result<()> {
    let mut preview_rect = Rect::default();
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

        let preview_area = columns[2];

        let left_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(columns[0]);

        let middle_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(columns[1]);

        //
        // PINNED
        //
        let pinned_focused = app.focus == Focus::Pinned;

        let pinned_items: Vec<ListItem> = app
            .pinned
            .iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("home")
                    .to_string();

                ListItem::new(name).style(Style::default().fg(if pinned_focused {
                    theme.foreground
                } else {
                    theme.muted
                }))
            })
            .collect();

        let mut pinned_state = ListState::default();
        pinned_state.select(Some(app.pinned_selected));

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

        //
        // CLIPBOARD
        //
        let clipboard_text = if let Some((path, mode)) = &app.clipboard {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");

            match mode {
                ClipboardMode::Copy => format!("Copy: {}", name),
                ClipboardMode::Cut => format!("Cut: {}", name),
            }
        } else {
            "Empty".to_string()
        };

        let clipboard = Paragraph::new(clipboard_text).block(
            Block::default()
                .title(" Clipboard ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        );

        f.render_widget(clipboard, left_chunks[1]);

        //
        // FILES
        //
        let files_focused = app.focus == Focus::Files;

        let items: Vec<ListItem> = app
            .entries
            .iter()
            .map(|e| {
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

                let line = Line::from(vec![
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

            let path = entry.path().to_path_buf();

            match std::fs::symlink_metadata(&path) {
                Ok(meta) => {
                    // -------- File name (OWNED) --------
                    let file_name: String = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown")
                        .to_string();

                    // -------- File type (OWNED) --------
                    let file_type: String = if meta.file_type().is_symlink() {
                        "Symlink".to_string()
                    } else if meta.is_dir() {
                        "Directory".to_string()
                    } else if meta.is_file() {
                        "File".to_string()
                    } else {
                        "Other".to_string()
                    };
                    //---------- Resolution of img -----------
                    let resolution_line = if meta.is_file() {
                        if let Some((w, h)) = crate::app::get_dimensions(&path) {
                            Some(Line::from(vec![
                                Span::styled("Resolution ", Style::default().fg(theme.muted)),
                                Span::raw(format!("{}x{}", w, h)),
                            ]))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    // -------- Size (OWNED) --------
                    let size: String = if meta.is_file() {
                        format_size(meta.len())
                    } else if meta.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&path) {
                            let total: u64 = entries
                                .flatten()
                                .filter_map(|e| e.metadata().ok())
                                .filter(|m| m.is_file())
                                .map(|m| m.len())
                                .sum();

                            format_size(total)
                        } else {
                            "-".to_string()
                        }
                    } else {
                        "-".to_string()
                    };

                    // -------- Modified time (OWNED) --------
                    let modified: String = meta
                        .modified()
                        .ok()
                        .and_then(|time| {
                            let datetime: chrono::DateTime<chrono::Local> = time.into();
                            Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
                        })
                        .unwrap_or_else(|| "Unknown".to_string());

                    // -------- Permissions (OWNED) --------
                    let mode = meta.permissions().mode();
                    let perms: String = format_permissions(mode);
                    let octal: String = format!("{:o}", mode & 0o777);

                    let path_string: String = path.display().to_string();

                    let mut lines = vec![
                        Line::from(vec![
                            Span::styled("Name      ", Style::default().fg(theme.muted)),
                            Span::styled(file_name, Style::default().fg(theme.foreground)),
                        ]),
                        Line::from(vec![
                            Span::styled("Type      ", Style::default().fg(theme.muted)),
                            Span::raw(file_type),
                        ]),
                        Line::from(vec![
                            Span::styled("Size      ", Style::default().fg(theme.muted)),
                            Span::raw(size),
                        ]),
                        Line::from(vec![
                            Span::styled("Perms     ", Style::default().fg(theme.muted)),
                            Span::raw(format!("{} ({})", perms, octal)),
                        ]),
                        Line::from(vec![
                            Span::styled("Modified  ", Style::default().fg(theme.muted)),
                            Span::raw(modified),
                        ]),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("Path      ", Style::default().fg(theme.muted)),
                            Span::styled(path_string, Style::default().fg(theme.status_fg)),
                        ]),
                    ];

                    if let Some(res_line) = resolution_line {
                        lines.insert(2, res_line);
                    }
                    lines
                }
                Err(_) => {
                    vec![Line::from(Span::styled(
                        "Unable to read metadata",
                        Style::default().fg(theme.muted),
                    ))]
                }
            }
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

        let preview_block = Block::default()
            .title(" Preview ")
            .borders(Borders::ALL);

        f.render_widget(preview_block.clone(), columns[2]);
        let inner = preview_block.inner(columns[2]);

        //
        // debounce guard
        //
        let mut allow_preview = true;

        if let Some(deadline) = app.preview_deadline {
            if std::time::Instant::now() < deadline {
                allow_preview = false;
            } else {
                app.preview_deadline = None;
            }
        }

        if !allow_preview {
            let loading = Paragraph::new("…").alignment(Alignment::Center);
            f.render_widget(loading, inner);
            return;
        }


        //
        // 🔥 POLL ASYNC IMAGE RESULT
        //
        if let Some(rx) = &app.image_rx {
            while let Ok((id, result)) = rx.try_recv() {
                if id == app.image_request_id {
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

            let is_image = matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif"
            );

            let is_pdf = ext == "pdf";

            //
            // 🖼 IMAGE / PDF PREVIEW
            //
            if (is_image || is_pdf) && path.is_file() {
                let q_width = quantize(inner.width);
                let q_height = quantize(inner.height);

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

                if reload && !app.image_loading {

                    if inner.width < 10 || inner.height < 5 {
                        let loading = Paragraph::new("…").alignment(Alignment::Center);
                        f.render_widget(loading, inner);
                        return;
                    }

                    app.image_request_id = app.image_request_id.wrapping_add(1);
                    let request_id = app.image_request_id;

                    app.image_request_atomic
                        .store(request_id, Ordering::Relaxed);

                    app.image = None;
                    app.preview_deadline = Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_millis(60)
                    );

                    app.image_size = Some((inner.width, inner.height));
                    app.image_path = Some(path.clone());
                    app.image_loading = true;

                    app.preview_job_tx.send(PreviewJob {
                        request_id,
                        path: path.clone(),
                        inner,
                        is_pdf,
                    }).ok();
                }

                // render image
                if let Some(img) = &app.image {
                    let widget = Image::new(img);
                    f.render_widget(widget, inner);
                } else {
                    let loading = Paragraph::new("Loading preview…")
                        .alignment(Alignment::Center);
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
                    "png" | "jpg" | "jpeg" | "webp" | "gif"
                        | "mp3" | "wav" | "flac"
                        | "mp4" | "mkv" | "mov"
                        | "zip" | "tar" | "gz" | "rar"
                        | "exe" | "bin" | "so" | "pdf"
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
                                    if let Some(name) = e.file_name().to_str() {
                                        if !app.show_hidden && name.starts_with('.') {
                                            return false;
                                        }
                                    }
                                    true
                                })
                                .collect();

                            items.sort_by(|a, b| {
                                use std::cmp::Ordering;

                                let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
                                let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);

                                if a_dir != b_dir {
                                    return if a_dir { Ordering::Less } else { Ordering::Greater };
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

                    let preview = Paragraph::new(lines.join("\n"))
                        .wrap(Wrap { trim: false });

                    f.render_widget(preview, inner);
                }
                else if is_probably_text && path.is_file() {
                    let content = std::fs::read_to_string(&path)
                        .map(|s| {
                            s.lines()
                                .take(inner.height as usize)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_else(|_| "Unable to read file".to_string());

                    let preview = Paragraph::new(content)
                        .wrap(Wrap { trim: false });

                    f.render_widget(preview, inner);
                }
                else {
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
            .constraints([
                Constraint::Min(0),
                Constraint::Length(20),
            ])
            .split(vertical[2]);

        let left_status = Paragraph::new(Line::from(vec![
            Span::styled(
                "[Fren] ",
                Style::default()
                    .fg(theme.focus_border)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " /: help ",
            )),
        ]))
        .style(Style::default().bg(theme.status_bg).fg(theme.status_fg));

        f.render_widget(left_status, status_chunks[0]);

        let right_status = Paragraph::new(
            Line::from(Span::styled(
                format!("Sort: {:?}", app.sort_mode),
                Style::default()
                    .fg(theme.focus_border)
                    .add_modifier(Modifier::BOLD),
            ))
        )
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.status_bg).fg(theme.status_fg));

        f.render_widget(right_status, status_chunks[1]);



        //
        // INPUT MODAL
        //
        if let AppMode::Input(action) = &app.mode {
            render_dim_overlay(f, area, theme);

            let popup_area = centered_rect(60, 20, area);

            let title_text = match action {
                InputAction::Rename => " Rename ",
                InputAction::CreateFile => " Create File ",
                InputAction::CreateFolder => " Create Folder ",
                InputAction::ConfirmDelete => " Confirm Delete ",
                InputAction::OpenWith => " Open With ",
            };

            let input = Paragraph::new(app.input.as_str())
                .style(Style::default().fg(theme.foreground).bg(theme.background))
                .block(
                    Block::default()
                        .title(Span::styled(
                            title_text,
                            Style::default()
                                .fg(theme.focus_border)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.border)),
                );

            f.render_widget(Clear, popup_area);
            f.render_widget(input, popup_area);
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
fn draw_help_popup(
    f: &mut ratatui::Frame,
    area: Rect,
    config: &Config,
    theme: &Theme,
) {
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
        Line::from(format!("Create file        : {}", config.keymaps.create_file)),
        Line::from(format!("Create folder      : {}", config.keymaps.create_folder)),
        Line::from(format!("Rename             : {}", config.keymaps.rename)),
        Line::from(format!("Copy               : {}", config.keymaps.copy)),
        Line::from(format!("Cut                : {}", config.keymaps.cut)),
        Line::from(format!("Paste              : {}", config.keymaps.paste)),
        Line::from(format!("Toggle hidden      : {}", config.keymaps.toggle_hidden)),
        Line::from(format!("Pin                : {}", config.keymaps.pin)),
        Line::from(format!("Unpin              : {}", config.keymaps.unpin)),
        Line::from(format!("Sorting mode       : {}", config.keymaps.sort)),
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
        .style(
            Style::default()
                .fg(theme.foreground)
                .bg(theme.background),
        )
        .block(
            Block::default()
                .title(
                    Span::styled(
                        " Help ",
                        Style::default()
                            .fg(theme.focus_border)
                            .add_modifier(Modifier::BOLD),
                    ),
                )
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
//
// dir size
//
fn dir_size(path: &std::path::Path) -> u64 {
    let mut size = 0;

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.is_file() {
                    size += meta.len();
                } else if meta.is_dir() {
                    size += dir_size(&path);
                }
            }
        }
    }

    size
}
