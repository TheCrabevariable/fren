use std::io;

use crossterm::event::{self, Event, KeyCode};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::{App, AppMode, ConflictAction, Focus, InputAction};
use crate::config::Config;
use crate::theme::Theme;

pub fn handle_events(
    app: &mut App,
    _terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    config: &Config,
    _theme: &Theme,
) -> io::Result<bool> {
    if let Event::Key(key) = event::read()? {
        //block input
        if app.show_help {
            if let KeyCode::Esc = key.code {
                app.show_help = false;
            }
            return Ok(true);
        }

        //
        // CONFLICT MODE
        //
        if let AppMode::Conflict(_) = &app.mode {
            match key.code {
                KeyCode::Char('s') => app.apply_conflict_action(ConflictAction::Skip)?,
                KeyCode::Char('r') => app.apply_conflict_action(ConflictAction::Replace)?,
                KeyCode::Char('n') => app.apply_conflict_action(ConflictAction::RenameAuto)?,
                KeyCode::Esc => app.apply_conflict_action(ConflictAction::Cancel)?,
                _ => {}
            }
            return Ok(true);
        }

        //
        // INPUT MODE
        //
        if let AppMode::Input(action) = app.mode.clone() {
            if let InputAction::ConfirmDelete = action {
                match key.code {
                    KeyCode::Char('y') => {
                        app.trash_selected()?;
                        app.mode = AppMode::Normal;
                        app.input.clear();
                    }
                    KeyCode::Char('n') | KeyCode::Esc => {
                        app.mode = AppMode::Normal;
                        app.input.clear();
                    }
                    _ => {}
                }

                return Ok(true);
            }
            match key.code {
                KeyCode::Enter => {
                    match action {
                        InputAction::Rename => {
                            app.confirm_rename()?;
                        }

                        InputAction::CreateFile => {
                            let name = app.input.clone();
                            if !name.is_empty() {
                                app.create_file(&name)?;
                            }
                        }

                        InputAction::CreateFolder => {
                            let name = app.input.clone();
                            if !name.is_empty() {
                                app.create_folder(&name)?;
                            }
                        }

                        InputAction::OpenWith => {
                            let program = app.input.clone();
                            if !program.is_empty() {
                                app.open_with_program(&program)?;
                            }
                        }

                        InputAction::GoTo => {
                            let mut path_str = app.input.clone();
                            if path_str.starts_with('~') {
                                if let Some(home) = dirs::home_dir() {
                                    path_str =
                                        path_str.replacen("~", home.to_str().unwrap_or(""), 1);
                                }
                            }
                            let path = std::path::PathBuf::from(&path_str);
                            if path.exists() && path.is_dir() {
                                app.current_dir = path;
                                let _ = app.refresh();
                            }
                        }

                        _ => {}
                    }

                    app.input.clear();
                    app.input_cursor = 0;
                    app.mode = AppMode::Normal;
                }

                KeyCode::Esc => {
                    app.input.clear();
                    app.input_cursor = 0;
                    app.mode = AppMode::Normal;
                }

                KeyCode::Left => {
                    if app.input_cursor > 0 {
                        let mut prev = app.input_cursor - 1;
                        while !app.input.is_char_boundary(prev) {
                            prev -= 1;
                        }
                        app.input_cursor = prev;
                    }
                }

                KeyCode::Right => {
                    for (byte_idx, _) in app.input.char_indices() {
                        if byte_idx > app.input_cursor {
                            app.input_cursor = byte_idx;
                            break;
                        }
                    }
                }

                KeyCode::Home => {
                    app.input_cursor = 0;
                }

                KeyCode::End => {
                    app.input_cursor = app.input.len();
                }

                KeyCode::Backspace => {
                    if app.input_cursor > 0 {
                        let mut prev = app.input_cursor - 1;
                        while !app.input.is_char_boundary(prev) {
                            prev -= 1;
                        }
                        app.input.remove(prev);
                        app.input_cursor = prev;
                    }
                }

                KeyCode::Delete => {
                    if app.input_cursor < app.input.len() {
                        app.input.remove(app.input_cursor);
                    }
                }

                KeyCode::Char(c) => {
                    app.input.insert(app.input_cursor, c);
                    app.input_cursor += c.len_utf8();
                }

                _ => {}
            }

            return Ok(true);
        }

        //
        // NORMAL MODE
        //
        match key.code {
            // Switch Focus
            KeyCode::Tab => {
                if config.keymaps.focus == "tab" {
                    app.focus = match app.focus {
                        Focus::Files => Focus::Pinned,
                        Focus::Pinned => Focus::Storage,
                        Focus::Storage => Focus::Clipboard,
                        Focus::Clipboard => Focus::Files,
                    };
                }
            }
            //show helper
            KeyCode::Char('/') => {
                app.show_help = !app.show_help;
            }

            //
            // Navigation
            //
            KeyCode::Down => match app.focus {
                Focus::Files => {
                    if app.selected + 1 < app.entries.len() {
                        app.selected += 1;

                        // reset preview state
                        app.image_loading = false;
                        app.image_path = None;

                        // debounce
                        app.preview_deadline =
                            Some(std::time::Instant::now() + std::time::Duration::from_millis(60));
                    }
                }
                Focus::Pinned => {
                    if app.pinned_selected + 1 < app.pinned.len() {
                        app.pinned_selected += 1;
                    } else if !app.storage.is_empty() {
                        app.focus = Focus::Storage;
                        app.storage_selected = 0;
                    }
                }
                Focus::Storage => {
                    if app.storage_selected + 1 < app.storage.len() {
                        app.storage_selected += 1;
                    } else if !app.clipboard.is_empty() {
                        app.focus = Focus::Clipboard;
                        app.clipboard_selected = 0;
                    }
                }
                Focus::Clipboard => {
                    if app.clipboard_selected + 1 < app.clipboard.len() {
                        app.clipboard_selected += 1;
                    }
                }
            },

            KeyCode::Up => match app.focus {
                Focus::Files => {
                    if app.selected > 0 {
                        app.selected -= 1;

                        // reset preview state
                        app.image_loading = false;
                        app.image_path = None;

                        // debounce
                        app.preview_deadline =
                            Some(std::time::Instant::now() + std::time::Duration::from_millis(60));
                    }
                }
                Focus::Pinned => {
                    if app.pinned_selected > 0 {
                        app.pinned_selected -= 1;
                    } else if !app.storage.is_empty() {
                        app.focus = Focus::Storage;
                        app.storage_selected = app.storage.len() - 1;
                    }
                }
                Focus::Storage => {
                    if app.storage_selected > 0 {
                        app.storage_selected -= 1;
                    } else if !app.pinned.is_empty() {
                        app.focus = Focus::Pinned;
                        app.pinned_selected = app.pinned.len() - 1;
                    }
                }
                Focus::Clipboard => {
                    if app.clipboard_selected > 0 {
                        app.clipboard_selected -= 1;
                    } else if !app.storage.is_empty() {
                        app.focus = Focus::Storage;
                        app.storage_selected = app.storage.len() - 1;
                    }
                }
            },
            //open with enter
            KeyCode::Enter => match app.focus {
                Focus::Pinned => {
                    if app.pinned_selected < app.pinned.len() {
                        app.open_pinned()?;
                        app.focus = Focus::Files;
                    }
                }
                Focus::Storage => {
                    if app.storage_selected < app.storage.len() {
                        app.open_storage()?;
                        app.focus = Focus::Files;
                    }
                }
                Focus::Files => {
                    if config.keymaps.open == "enter" {
                        app.start_input(InputAction::OpenWith, None);
                    }
                }
                Focus::Clipboard => {
                    app.paste_selected()?;
                }
            },
            KeyCode::Right => match app.focus {
                Focus::Files => {
                    app.cursor_memory
                        .insert(app.current_dir.clone(), app.selected);

                    app.enter()?;
                }
                Focus::Pinned => {
                    if app.pinned_selected < app.pinned.len() {
                        app.open_pinned()?;
                        app.focus = Focus::Files;
                    }
                }
                Focus::Storage => {
                    if app.storage_selected < app.storage.len() {
                        app.open_storage()?;
                        app.focus = Focus::Files;
                    }
                }
                Focus::Clipboard => {
                    app.paste_selected()?;
                }
            },
            KeyCode::Left => match app.focus {
                Focus::Files => app.up()?,
                _ => app.focus = Focus::Files,
            },

            //
            // Keymap Controlled Actions
            //
            KeyCode::Char(c) => {
                let pressed = c.to_string();

                if pressed == config.keymaps.toggle_select && app.focus == Focus::Files {
                    app.toggle_selection();
                }

                // Quit
                if pressed == config.keymaps.quit {
                    return Ok(false);
                }

                // Rename
                if pressed == config.keymaps.rename {
                    if let Some(entry) = app.entries.get(app.selected) {
                        if let Some(name) = entry.file_name().to_str() {
                            app.start_input(InputAction::Rename, Some(name.to_string()));
                        }
                    }
                }
                if pressed == config.keymaps.focus {
                    app.focus = if app.focus == Focus::Files {
                        Focus::Pinned
                    } else {
                        Focus::Files
                    };
                }
                // Create File
                if pressed == config.keymaps.create_file {
                    app.start_input(InputAction::CreateFile, None);
                }

                // Create Folder
                if pressed == config.keymaps.create_folder {
                    app.start_input(InputAction::CreateFolder, None);
                }

                // Trash
                if pressed == config.keymaps.trash {
                    if app.focus == Focus::Clipboard {
                        app.remove_clipboard_item();
                    } else {
                        app.start_input(InputAction::ConfirmDelete, None);
                    }
                }

                // Open With
                if pressed == config.keymaps.open {
                    app.start_input(InputAction::OpenWith, None);
                }

                // Sort
                if pressed == config.keymaps.sort {
                    app.cycle_sort()?;
                }

                // Copy
                if pressed == config.keymaps.copy {
                    if app.focus == Focus::Clipboard {
                        app.recopy_clipboard_item();
                    } else {
                        app.copy_selected();
                    }
                }
                //Cut
                if pressed == config.keymaps.cut {
                    app.cut_selected();
                }
                //Paste
                if pressed == config.keymaps.paste {
                    if app.focus == Focus::Clipboard {
                        app.paste_selected()?;
                    } else {
                        app.paste()?;
                    }
                }
                // Toggle Hidden
                if pressed == config.keymaps.toggle_hidden {
                    app.toggle_hidden()?;
                }

                if pressed == config.keymaps.pin && app.focus == Focus::Files {
                    app.pin_selected();
                }

                if pressed == config.keymaps.unpin && app.focus == Focus::Pinned {
                    app.unpin_selected();
                }

                if pressed == config.keymaps.go_to {
                    app.start_input(InputAction::GoTo, None);
                }
            }

            _ => {}
        }
    }

    Ok(true)
}
