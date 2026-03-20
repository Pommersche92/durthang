use std::path::PathBuf;

use crate::config::Config;
use crate::net::Connection;
use crate::ui::game::GameState;
use crate::ui::selection::SelectState;

/// Top-level application state machine.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum AppState {
    /// Server / character selection screen (startup state).
    #[default]
    ServerSelect,
    /// Active game session.
    Game,
}

/// Central application object passed through the main loop.
pub struct App {
    pub state: AppState,
    pub running: bool,
    pub config: Config,
    pub config_path: PathBuf,
    pub select: SelectState,
    /// Active network connection, present while in `AppState::Game`.
    pub connection: Option<Connection>,
    /// Display name of the currently connected server (for the status bar).
    pub connected_server: Option<String>,
    /// Config id of the connected server (used for map persistence).
    pub connected_server_id: Option<String>,
    /// Display name of the currently connected character.
    pub connected_char: Option<String>,
    /// Config id of the connected character (used to look up aliases/triggers).
    pub connected_char_id: Option<String>,
    /// Game view state (scrollback, input, history, …).
    pub game: GameState,
}

impl App {
    pub fn new(config: Config, config_path: PathBuf) -> Self {
        Self {
            state: AppState::default(),
            running: true,
            config,
            config_path,
            select: SelectState::new(),
            connection: None,
            connected_server: None,
            connected_server_id: None,
            connected_char: None,
            connected_char_id: None,
            game: GameState::new(),
        }
    }

    /// Signal the main loop to exit cleanly.
    pub fn quit(&mut self) {
        self.running = false;
    }
}
