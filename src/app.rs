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
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::default(),
            running: true,
        }
    }

    /// Signal the main loop to exit cleanly.
    pub fn quit(&mut self) {
        self.running = false;
    }
}
