mod app;
mod config;
mod map;
mod net;
mod ui;

use app::App;
use clap::Parser;
use config::Config;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Paragraph},
};
use std::{fs, io, path::PathBuf, sync::Mutex};
use tracing::info;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(version, about = "Durthang — a terminal MUD client")]
struct Cli {
    /// Path to the configuration file.
    /// Defaults to $XDG_CONFIG_HOME/durthang/config.toml
    /// (typically ~/.config/durthang/config.toml).
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
}

/// Returns the XDG data directory for durthang (`~/.local/share/durthang`).
fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").expect("HOME environment variable not set");
            PathBuf::from(home).join(".local/share")
        });
    base.join("durthang")
}

/// Initialise structured logging to `<data_dir>/durthang.log`.
/// Log level can be overridden via the `RUST_LOG` environment variable.
fn init_logging() -> io::Result<()> {
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    let log_file = fs::File::create(dir.join("durthang.log"))?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log_file))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    Ok(())
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(Config::default_path);

    init_logging()?;
    info!("Durthang starting up");
    info!("Using config file: {}", config_path.display());

    let _config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::warn!("Could not load config from {}: {e}", config_path.display());
        Config::default()
    });

    let mut app = App::new();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Main loop
    while app.running {
        terminal.draw(|frame| {
            let area = frame.area();
            let paragraph = Paragraph::new("Durthang — press 'q' to quit")
                .block(Block::bordered().title("Durthang"));
            frame.render_widget(paragraph, area);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => app.quit(),
                _ => {}
            }
        }
    }

    info!("Durthang shutting down");

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
