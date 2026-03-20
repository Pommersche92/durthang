// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! User-interface module.
//!
//! The UI is built with [`ratatui`] and is entirely synchronous — it lives on
//! the main thread alongside the event loop.  Three sub-modules share the
//! rendering surface:
//!
//! | Sub-module | Purpose |
//! |---|---|
//! | [`selection`] | Server / character tree shown at startup |
//! | [`game`] | Main game view: scrollback, input bar, status bar, sidebar |
//! | [`sidebar`] | Sidebar panel system (automap minimap, user notes) |

pub mod game;
pub mod selection;
pub mod sidebar;
