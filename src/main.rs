mod app;
mod config;
mod event;
mod theme;
mod ui;

use std::io;
use std::time::Duration;

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use ratatui::{Terminal, backend::CrosstermBackend};

use crate::app::App;
use crate::config::Config;
use crate::theme::Theme;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let config = Config::load();
    let theme = Theme::load();

    let mut app = App::new(config.remember)?;
    app.load_pinned()?;

    // Draw the first frame so the user sees the UI immediately,
    // then init the image picker (which can be slow on some terminals).
    ui::draw_ui(&mut terminal, &mut app, &config, &theme)?;
    app.init_picker();

    // Main loop
    loop {
        if crossterm::event::poll(Duration::from_millis(16))?
            && !event::handle_events(&mut app, &mut terminal, &config, &theme)?
        {
            break;
        }

        ui::draw_ui(&mut terminal, &mut app, &config, &theme)?;
    }

    if config.remember {
        app.save_session()?;
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
