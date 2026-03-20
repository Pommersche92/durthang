// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Top-level application state.
//!
//! This module defines [`App`] — the single, shared state object that is
//! threaded through the entire main event loop — and [`AppState`], a
//! lightweight enum that drives which UI screen is rendered and which input
//! handler receives keyboard events.
//!
//! Both types live in the synchronous part of the application; the async
//! network task is an opaque [`crate::net::Connection`] handle stored
//! inside [`App`].

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
///
/// `App` is the single source of truth for all mutable state:
/// * The current UI [`AppState`] (`ServerSelect` / `Game`).
/// * The loaded [`Config`] and the path from which it was read.
/// * The [`SelectState`] for the server/character tree.
/// * An optional active [`Connection`] and its associated display labels.
/// * The [`GameState`] holding the scrollback buffer, input line, aliases,
///   triggers, and sidebar data.
///
/// All fields are `pub` so that the main loop can read and mutate them
/// without boilerplate accessor methods.
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
    /// Construct a new [`App`] with `running = true` and all optional fields
    /// set to `None`.
    ///
    /// The [`SelectState`] and [`GameState`] are initialised to their own
    /// `new()` defaults.  The application starts on the
    /// [`AppState::ServerSelect`] screen.
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
