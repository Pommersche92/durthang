mod app;
mod config;
mod map;
mod net;
mod ui;

use app::{App, AppState};
use clap::Parser;
use config::Config;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
            MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use net::{Connection, NetEvent};
use ratatui::prelude::*;
use std::{fs, io, path::PathBuf, sync::Mutex};
use tracing::info;
use ui::game::GameAction;

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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Enter tokio context so we can call .await helpers inside the sync loop.
    rt.block_on(async {
        run_loop(&mut app, &mut terminal).await;
    });

    info!("Durthang shutting down");
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
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
                    let server = app.connected_server.as_deref().unwrap_or("?");
                    let character = app.connected_char.as_deref().unwrap_or("?");
                    ui::game::draw(frame, &mut app.game, server, character);
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
            Event::Mouse(mouse) => {
                if matches!(app.state, AppState::Game) {
                    match mouse.kind {
                        MouseEventKind::ScrollUp   => app.game.scroll_up(3),
                        MouseEventKind::ScrollDown => app.game.scroll_down(3),
                        _ => {}
                    }
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
                        match ui::game::handle_key(&mut app.game, key) {
                            Some(GameAction::SendLine(line)) => {
                                if let Some(conn) = &app.connection {
                                    conn.send_line(line).await;
                                }
                            }
                            Some(GameAction::Disconnect) => {
                                if let Some(conn) = &app.connection {
                                    conn.disconnect().await;
                                }
                                app.connection = None;
                                app.game.on_disconnect();
                                app.state = AppState::ServerSelect;
                            }
                            Some(GameAction::Quit) => {
                                if let Some(conn) = &app.connection {
                                    conn.disconnect().await;
                                }
                                app.quit();
                                break;
                            }
                            Some(GameAction::CopyToClipboard(text)) => {
                                copy_to_clipboard(&text);
                            }
                            Some(GameAction::AddAlias { name, expansion }) => {
                                game_add_alias(app, name, expansion);
                            }
                            Some(GameAction::RemoveAlias(name)) => {
                                game_remove_alias(app, name);
                            }
                            Some(GameAction::AddTrigger { pattern, color, send }) => {
                                game_add_trigger(app, pattern, color, send);
                            }
                            Some(GameAction::RemoveTrigger(id_prefix)) => {
                                game_remove_trigger(app, id_prefix);
                            }
                            None => {}
                        }
                        // Drain any trigger-generated auto-sends.
                        let sends: Vec<String> = app.game.auto_send_queue.drain(..).collect();
                        for line in sends {
                            if let Some(conn) = &app.connection {
                                conn.send_line(line).await;
                            }
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

    loop {
        match conn.rx.try_recv() {
            Ok(NetEvent::Line(line)) => {
                info!(target: "game", "{line}");
                app.game.push_line(&line);
            }
            Ok(NetEvent::Prompt(prompt)) => {
                app.game.push_prompt(&prompt);
            }
            Ok(NetEvent::Connected) => {
                info!("Network: connected");
                app.game.on_connect();
            }
            Ok(NetEvent::Disconnected(reason)) => {
                tracing::warn!("Disconnected: {reason}");
                let msg = format!("\x1b[33m-- Disconnected: {reason} --\x1b[0m");
                app.game.push_line(&msg);
                app.game.on_disconnect();
                app.connection = None;
                app.state = AppState::ServerSelect;
                break;
            }
            Ok(NetEvent::Latency(ms)) => {
                app.game.latency = Some(ms);
            }
            Err(_) => break,
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

    // Resolve character name and auto-login credentials.
    let (char_name, auto_login, char_id_owned) = if let Some(cid) = char_id {
        match app.config.characters.iter().find(|c| c.id == cid) {
            Some(c) => {
                info!("Connecting to {} ({}) as {}", server.name, server.host, c.name);
                let login = c.effective_login().to_string();
                let password = config::get_password(&server.id, &login).unwrap_or(None);
                let auto_login = if c.login.is_some() || password.is_some() {
                    Some((login, password))
                } else {
                    None
                };
                (c.name.clone(), auto_login, Some(cid.to_string()))
            }
            None => {
                tracing::warn!("Connect: char_id {cid} not found in config");
                return;
            }
        }
    } else {
        info!("Connecting to {} ({}) without a saved character", server.name, server.host);
        (String::from("(anonymous)"), None, None)
    };

    let size = terminal_size();
    let conn = Connection::spawn(server.host.clone(), server.port, server.tls, auto_login, size);

    app.connection     = Some(conn);
    app.connected_server   = Some(server.name.clone());
    app.connected_char     = Some(char_name);
    app.connected_char_id  = char_id_owned;
    // Clear the game view for the new session and mark as connected.
    app.game = crate::ui::game::GameState::new();
    app.game.on_connect();

    // Load aliases and triggers for this character.
    if let Some(cid) = &app.connected_char_id {
        if let Some(ch) = app.config.characters.iter().find(|c| &c.id == cid) {
            app.game.set_aliases(ch.aliases.clone());
            app.game.set_triggers(ch.triggers.clone());
        }
    }

    app.state = AppState::Game;
}

// ---------------------------------------------------------------------------
// In-game config mutation helpers
// ---------------------------------------------------------------------------

fn save_config_quiet(app: &App) {
    if let Err(e) = app.config.save(&app.config_path) {
        tracing::warn!("Failed to save config: {e}");
    }
}

fn game_add_alias(app: &mut App, name: String, expansion: String) {
    let Some(cid) = app.connected_char_id.clone() else { return };
    if let Some(ch) = app.config.characters.iter_mut().find(|c| c.id == cid) {
        ch.aliases.retain(|a| a.name != name);
        ch.aliases.push(config::Alias { name: name.clone(), expansion: expansion.clone() });
    }
    save_config_quiet(app);
    // Sync updated list back to game state.
    if let Some(ch) = app.config.characters.iter().find(|c| c.id == cid) {
        app.game.set_aliases(ch.aliases.clone());
        app.game.push_system(&format!("Alias added: {} \u{2192} {}", name, expansion));
    }
}

fn game_remove_alias(app: &mut App, name: String) {
    let Some(cid) = app.connected_char_id.clone() else { return };
    let removed = if let Some(ch) = app.config.characters.iter_mut().find(|c| c.id == cid) {
        let before = ch.aliases.len();
        ch.aliases.retain(|a| a.name != name);
        ch.aliases.len() < before
    } else { false };
    if removed {
        save_config_quiet(app);
        if let Some(ch) = app.config.characters.iter().find(|c| c.id == cid) {
            app.game.set_aliases(ch.aliases.clone());
        }
        app.game.push_system(&format!("Alias '{}' removed.", name));
    } else {
        app.game.push_system(&format!("No alias named '{}'.", name));
    }
}

fn game_add_trigger(
    app: &mut App,
    pattern: String,
    color: Option<String>,
    send: Option<String>,
) {
    let Some(cid) = app.connected_char_id.clone() else { return };
    // Validate regex before storing.
    if let Err(e) = regex::Regex::new(&pattern) {
        app.game.push_system(&format!("Invalid regex pattern: {e}"));
        return;
    }
    let trigger = config::Trigger {
        id: uuid::Uuid::new_v4().to_string(),
        pattern,
        color,
        send,
    };
    let id_short = trigger.id[..8].to_string();
    if let Some(ch) = app.config.characters.iter_mut().find(|c| c.id == cid) {
        ch.triggers.push(trigger);
    }
    save_config_quiet(app);
    if let Some(ch) = app.config.characters.iter().find(|c| c.id == cid) {
        app.game.set_triggers(ch.triggers.clone());
        app.game.push_system(&format!("Trigger [{id_short}] added."));
    }
}

fn game_remove_trigger(app: &mut App, id_prefix: String) {
    let Some(cid) = app.connected_char_id.clone() else { return };
    let removed = if let Some(ch) = app.config.characters.iter_mut().find(|c| c.id == cid) {
        let before = ch.triggers.len();
        ch.triggers.retain(|t| !t.id.starts_with(&id_prefix));
        ch.triggers.len() < before
    } else { false };
    if removed {
        save_config_quiet(app);
        if let Some(ch) = app.config.characters.iter().find(|c| c.id == cid) {
            app.game.set_triggers(ch.triggers.clone());
        }
        app.game.push_system(&format!("Trigger '{}...' removed.", id_prefix));
    } else {
        app.game.push_system(&format!("No trigger with id prefix '{}'.", id_prefix));
    }
}

/// Copy text to the clipboard using the OSC 52 terminal escape sequence.
/// Works in most modern terminal emulators (kitty, alacritty, foot, tmux, …).
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let b64 = base64_encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{b64}\x07");
    let _ = std::io::stdout().write_all(seq.as_bytes());
    let _ = std::io::stdout().flush();
}

/// Minimal base64 encoder — no external dependency needed.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let v  = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((v >> 18) & 0x3F) as usize] as char);
        out.push(T[((v >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() >= 2 { T[((v >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() >= 3 { T[(v & 0x3F) as usize] as char       } else { '=' });
    }
    out
}
