mod app;
mod config;
mod map;
mod net;
mod ui;

use app::{App, AppState};
use clap::Parser;
use config::Config;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use net::{Connection, NetEvent};
use ratatui::prelude::*;
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").expect("HOME environment variable not set");
            PathBuf::from(home).join(".local/share")
        });
    base.join("durthang")
}

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

/// Query the current terminal size and return `(cols, rows)`.
fn terminal_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(Config::default_path);

    init_logging()?;
    info!("Durthang starting up");
    info!("Using config file: {}", config_path.display());

    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::warn!("Could not load config from {}: {e}", config_path.display());
        Config::default()
    });

    let rt = tokio::runtime::Runtime::new()?;

    let mut app = App::new(config, config_path);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Enter tokio context so we can call .await helpers inside the sync loop.
    rt.block_on(async {
        run_loop(&mut app, &mut terminal).await;
    });

    info!("Durthang shutting down");
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Main async loop
// ---------------------------------------------------------------------------

async fn run_loop(app: &mut App, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    loop {
        // Drain non-blocking net events before drawing.
        drain_net_events(app);

        terminal
            .draw(|frame| match app.state {
                AppState::ServerSelect => ui::selection::draw(frame, &app.select, &app.config),
                AppState::Game => {
                    // Phase 5: game view
                    let area = frame.area();
                    let status = format!(
                        " Connected to {}  —  {}  (Phase 5: game view coming soon)  q to disconnect",
                        app.connected_server.as_deref().unwrap_or("?"),
                        app.connected_char.as_deref().unwrap_or("?"),
                    );
                    use ratatui::widgets::{Block, Paragraph};
                    frame.render_widget(
                        Paragraph::new(status).block(Block::bordered().title("Durthang")),
                        area,
                    );
                }
            })
            .expect("terminal draw failed");

        // Poll for the next crossterm event (100 ms timeout so net events are drained regularly).
        if !event::poll(std::time::Duration::from_millis(100)).expect("event poll failed") {
            continue;
        }

        match event::read().expect("event read failed") {
            Event::Resize(cols, rows) => {
                if let Some(conn) = &app.connection {
                    conn.send_naws(cols, rows).await;
                }
            }
            Event::Key(key) => {
                // Global Ctrl-C always quits.
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')
                {
                    if let Some(conn) = &app.connection {
                        conn.disconnect().await;
                    }
                    app.quit();
                    break;
                }

                match app.state {
                    AppState::ServerSelect => {
                        let quit = ui::selection::handle_key(
                            &mut app.select,
                            &mut app.config,
                            &app.config_path,
                            key,
                        );
                        if quit {
                            app.quit();
                            break;
                        }
                        if let Some((server_id, char_id)) = app.select.pending_connect.take() {
                            do_connect(app, &server_id, char_id.as_deref()).await;
                        }
                    }
                    AppState::Game => {
                        match key.code {
                            KeyCode::Char('q') => {
                                if let Some(conn) = &app.connection {
                                    conn.disconnect().await;
                                }
                                app.connection = None;
                                app.state = AppState::ServerSelect;
                            }
                            _ => { /* Phase 5: forward to game input handler */ }
                        }
                    }
                }
            }
            _ => {}
        }

        if !app.running {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Net event draining
// ---------------------------------------------------------------------------

fn drain_net_events(app: &mut App) {
    let conn = match app.connection.as_mut() {
        Some(c) => c,
        None => return,
    };

    // try_recv in a loop — non-blocking.
    loop {
        match conn.rx.try_recv() {
            Ok(NetEvent::Line(line)) => {
                // Phase 5 will append these to a scrollback buffer.
                // For now just log.
                info!(target: "game", "{line}");
            }
            Ok(NetEvent::Prompt(_)) => {
                // Phase 5: render prompt in the input line.
            }
            Ok(NetEvent::Connected) => {
                info!("Network: connected");
            }
            Ok(NetEvent::Disconnected(reason)) => {
                tracing::warn!("Disconnected: {reason}");
                app.connection = None;
                app.state = AppState::ServerSelect;
                break;
            }
            Ok(NetEvent::Latency(_)) => {}
            Err(_) => break, // channel empty or closed
        }
    }
}

// ---------------------------------------------------------------------------
// Connect helper
// ---------------------------------------------------------------------------

async fn do_connect(app: &mut App, server_id: &str, char_id: Option<&str>) {
    let server = match app.config.servers.iter().find(|s| s.id == server_id) {
        Some(s) => s.clone(),
        None => {
            tracing::warn!("Connect: server_id {server_id} not found in config");
            return;
        }
    };

    let char_name = if let Some(cid) = char_id {
        match app.config.characters.iter().find(|c| c.id == cid) {
            Some(c) => {
                info!("Connecting to {} ({}) as {}", server.name, server.host, c.name);
                c.name.clone()
            }
            None => {
                tracing::warn!("Connect: char_id {cid} not found in config");
                return;
            }
        }
    } else {
        info!("Connecting to {} ({}) without a saved character", server.name, server.host);
        String::from("(anonymous)")
    };

    let size = terminal_size();
    let conn = Connection::spawn(server.host.clone(), server.port, size);

    app.connection = Some(conn);
    app.connected_server = Some(server.name.clone());
    app.connected_char = Some(char_name);
    app.state = AppState::Game;
}


