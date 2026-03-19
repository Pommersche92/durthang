use std::path::PathBuf;

use crate::config::Config;
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
}

impl App {
    pub fn new(config: Config, config_path: PathBuf) -> Self {
        Self {
            state: AppState::default(),
            running: true,
            config,
            config_path,
            select: SelectState::new(),
        }
    }

    /// Signal the main loop to exit cleanly.
    pub fn quit(&mut self) {
        self.running = false;
    }
}
