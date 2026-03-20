// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Integration and unit test suite.
//!
//! Each sub-module targets one logical area of the codebase:
//!
//! | Module | Coverage |
//! |---|---|
//! | [`config`] | [`crate::config`] data model, TOML round-trip, sidebar migration |
//! | [`game`] | [`crate::ui::game::GameState`] alias expansion, triggers, latency |
//! | [`map`] | [`crate::map`] directions, GMCP parsing, coordinate placement |
//! | [`sidebar`] | [`crate::ui::sidebar::SidebarState`] layout, focus cycling, notes editor |

mod config;
mod game;
mod map;
mod sidebar;
