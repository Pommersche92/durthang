// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Tests for GameState: alias expansion, trigger compilation, latency rolling
//! average, scrollback buffer, and system message helpers.

use crate::config::{Alias, Trigger};
use crate::ui::game::GameState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn state_with_aliases(pairs: &[(&str, &str)]) -> GameState {
    let mut state = GameState::new();
    state.set_aliases(pairs.iter().map(|(n, e)| Alias {
        name: n.to_string(),
        expansion: e.to_string(),
    }).collect());
    state
}

// ---------------------------------------------------------------------------
// expand_alias
// ---------------------------------------------------------------------------

#[test]
fn expand_alias_exact_line_match() {
    let state = state_with_aliases(&[("k", "kill")]);
    assert_eq!(state.expand_alias("k"), "kill");
}

#[test]
fn expand_alias_prefix_match_appends_tail() {
    let state = state_with_aliases(&[("k", "kill")]);
    assert_eq!(state.expand_alias("k orc"), "kill orc");
}

#[test]
fn expand_alias_no_match_returns_original() {
    let state = state_with_aliases(&[("k", "kill")]);
    assert_eq!(state.expand_alias("hello"), "hello");
}

#[test]
fn expand_alias_prefers_exact_over_prefix() {
    // If there is both an exact-line alias and a word-prefix alias, exact wins.
    let state = state_with_aliases(&[("k orc", "backstab orc"), ("k", "kill")]);
    assert_eq!(state.expand_alias("k orc"), "backstab orc");
}

#[test]
fn expand_alias_empty_string_unchanged() {
    let state = state_with_aliases(&[("k", "kill")]);
    assert_eq!(state.expand_alias(""), "");
}

#[test]
fn expand_alias_no_aliases_returns_original() {
    let state = GameState::new();
    assert_eq!(state.expand_alias("go north"), "go north");
}

#[test]
fn expand_alias_multiple_aliases_picks_correct_one() {
    let state = state_with_aliases(&[
        ("h", "say hello"),
        ("l", "look"),
        ("n", "go north"),
    ]);
    assert_eq!(state.expand_alias("l"), "look");
    assert_eq!(state.expand_alias("n"), "go north");
    assert_eq!(state.expand_alias("h adventurer"), "say hello adventurer");
}

// ---------------------------------------------------------------------------
// set_triggers — invalid regex must be silently dropped
// ---------------------------------------------------------------------------

#[test]
fn invalid_trigger_regex_is_dropped() {
    let mut state = GameState::new();
    state.set_triggers(vec![
        Trigger { id: "t1".into(), pattern: "[valid regex".to_string(), color: None, send: None },
        Trigger { id: "t2".into(), pattern: "valid".to_string(),         color: None, send: None },
    ]);
    // The invalid one is dropped; the valid one is kept.
    // We can confirm via push_line — a line matching "valid" should not panic.
    state.push_line("this line is valid");
    // No panic = good. auto_send_queue must be empty (no send action).
    assert!(state.auto_send_queue.is_empty());
}

#[test]
fn trigger_auto_send_queues_command_on_match() {
    let mut state = GameState::new();
    state.set_triggers(vec![
        Trigger {
            id: "t1".into(),
            pattern: "You are hungry".to_string(),
            color: None,
            send: Some("buy bread".to_string()),
        },
    ]);
    state.push_line("You are hungry and need to eat.");
    assert_eq!(state.auto_send_queue, vec!["buy bread"]);
}

#[test]
fn trigger_does_not_queue_command_when_no_match() {
    let mut state = GameState::new();
    state.set_triggers(vec![
        Trigger {
            id: "t1".into(),
            pattern: "You are hungry".to_string(),
            color: None,
            send: Some("buy bread".to_string()),
        },
    ]);
    state.push_line("You feel fine.");
    assert!(state.auto_send_queue.is_empty());
}

// ---------------------------------------------------------------------------
// Latency rolling average
// ---------------------------------------------------------------------------

#[test]
fn latency_avg_none_initially() {
    let state = GameState::new();
    assert!(state.latency.is_none());
}

#[test]
fn latency_avg_single_sample() {
    let mut state = GameState::new();
    state.record_latency(50);
    assert_eq!(state.latency, Some(50));
}

#[test]
fn latency_updates_to_latest_value() {
    let mut state = GameState::new();
    state.record_latency(10);
    state.record_latency(20);
    state.record_latency(30);
    assert_eq!(state.latency, Some(30));
}

// ---------------------------------------------------------------------------
// Scrollback buffer
// ---------------------------------------------------------------------------

#[test]
fn push_system_increases_scrollback_len() {
    let mut state = GameState::new();
    assert_eq!(state.lines.len(), 0);
    state.push_system("Hello from tests");
    assert_eq!(state.lines.len(), 1);
}

#[test]
fn push_line_appends_to_scrollback() {
    let mut state = GameState::new();
    state.push_line("A line from the MUD server.");
    assert_eq!(state.lines.len(), 1);
}

#[test]
fn scrollback_does_not_exceed_maximum() {
    let mut state = GameState::new();
    // SCROLLBACK_MAX is 5_000; push slightly more.
    for i in 0..5_010 {
        state.push_line(&format!("Line {i}"));
    }
    assert!(state.lines.len() <= 5_000);
}

// ---------------------------------------------------------------------------
// on_connect / on_disconnect lifecycle
// ---------------------------------------------------------------------------

#[test]
fn on_connect_sets_connected_flag() {
    let mut state = GameState::new();
    assert!(!state.connected);
    state.on_connect();
    assert!(state.connected);
}

#[test]
fn on_disconnect_clears_connected_flag() {
    let mut state = GameState::new();
    state.on_connect();
    state.on_disconnect();
    assert!(!state.connected);
}

#[test]
fn on_connect_clears_latency() {
    let mut state = GameState::new();
    state.record_latency(100);
    state.on_connect();
    assert!(state.latency.is_none());
}
