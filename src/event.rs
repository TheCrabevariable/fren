use std::io;

use crossterm::event::{self, Event, KeyCode};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::sync::atomic::Ordering;

use crate::app::{App, AppMode, Focus, InputAction};
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

                        _ => {}
                    }

                    app.input.clear();
                    app.mode = AppMode::Normal;
                }

                KeyCode::Esc => {
                    app.input.clear();
                    app.mode = AppMode::Normal;
                }

                KeyCode::Backspace => {
                    app.input.pop();
                }

                KeyCode::Char(c) => {
                    app.input.push(c);
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
                        Focus::Pinned => Focus::Files,
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
                        app.preview_deadline = Some(
                            std::time::Instant::now()
                                + std::time::Duration::from_millis(60)
                        );
                    }
                }
                Focus::Pinned => {
                    if app.pinned_selected + 1 < app.pinned.len() {
                        app.pinned_selected += 1;
                    }
                }
            }
            //open with enter
            KeyCode::Enter => {
                if config.keymaps.open == "enter" {
                    app.start_input(InputAction::OpenWith, None);
                }
            }

            KeyCode::Up => match app.focus {
                Focus::Files => {
                    if app.selected > 0 {
                        app.selected -= 1;

                        // reset preview state
                        app.image_loading = false;
                        app.image_path = None;

                        // debounce
                        app.preview_deadline = Some(
                            std::time::Instant::now()
                                + std::time::Duration::from_millis(60)
                        );
                    }
                }
                Focus::Pinned => {
                    if app.pinned_selected > 0 {
                        app.pinned_selected -= 1;
                    }
                }
            }
            KeyCode::Right => {
                match app.focus {
                    Focus::Files => {
                        app.cursor_memory
                            .insert(app.current_dir.clone(), app.selected);

                        app.enter()?;
                    }
                    Focus::Pinned => {
                        app.cursor_memory
                            .insert(app.current_dir.clone(), app.selected);

                        app.open_pinned()?;
                    }
                }
            }
            KeyCode::Left => app.up()?,

            //
            // Keymap Controlled Actions
            //
            KeyCode::Char(c) => {
                let pressed = c.to_string();

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
                    app.focus = match app.focus {
                        Focus::Files => Focus::Pinned,
                        Focus::Pinned => Focus::Files,
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
                    app.start_input(InputAction::ConfirmDelete, None);
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
                    app.copy_selected();
                }
                //Cut
                if pressed == config.keymaps.cut {
                    app.cut_selected();
                }
                //Paste
                if pressed == config.keymaps.paste {
                    app.paste()?;
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
            }

            _ => {}
        }
    }

    Ok(true)
}
