// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Tests for sidebar state: layout migration, panel visibility, focus cycling,
//! width/visibility toggling, and the notes inline-editor key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{PanelConfig, PanelKind, SidebarLayout, SidebarSide};
use crate::ui::sidebar::{handle_sidebar_key, SidebarKeyResult, SidebarState};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn default_state() -> SidebarState {
    SidebarState::new(SidebarLayout::default())
}

// ---------------------------------------------------------------------------
// Migration via SidebarState::new
// ---------------------------------------------------------------------------

#[test]
fn new_with_empty_layout_inserts_both_panels() {
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![],
        notes: vec![],
    };
    let state = SidebarState::new(layout);
    assert!(state.layout.panels.iter().any(|p| p.kind == PanelKind::Automap));
    assert!(state.layout.panels.iter().any(|p| p.kind == PanelKind::Notes));
}

#[test]
fn new_with_automap_side_none_assigns_right() {
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![
            PanelConfig { kind: PanelKind::Automap, side: None, height_pct: 100 },
        ],
        notes: vec![],
    };
    let state = SidebarState::new(layout);
    let p = state.layout.panels.iter().find(|p| p.kind == PanelKind::Automap).unwrap();
    assert_eq!(p.side, Some(SidebarSide::Right));
}

#[test]
fn new_with_notes_side_none_assigns_right() {
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![
            PanelConfig { kind: PanelKind::Notes, side: None, height_pct: 100 },
        ],
        notes: vec![],
    };
    let state = SidebarState::new(layout);
    let p = state.layout.panels.iter().find(|p| p.kind == PanelKind::Notes).unwrap();
    assert_eq!(p.side, Some(SidebarSide::Right));
}

#[test]
fn new_preserves_correct_layout_unchanged() {
    let state = default_state();
    assert_eq!(state.layout.panels.len(), 2);
    assert!(state.layout.right_visible);
}

// ---------------------------------------------------------------------------
// has_side_panels
// ---------------------------------------------------------------------------

#[test]
fn has_side_panels_returns_true_for_right() {
    let state = default_state();
    assert!(state.has_side_panels(&SidebarSide::Right));
}

#[test]
fn has_side_panels_returns_false_for_left_when_no_left_panels() {
    let state = default_state();
    assert!(!state.has_side_panels(&SidebarSide::Left));
}

#[test]
fn has_side_panels_returns_false_after_all_panels_hidden() {
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![
            PanelConfig { kind: PanelKind::Automap, side: None, height_pct: 50 },
            PanelConfig { kind: PanelKind::Notes,   side: None, height_pct: 50 },
        ],
        notes: vec![],
    };
    // Skip migration by building state directly. For this test we bypass
    // SidebarState::new to avoid migrate_layout re-assigning sides.
    // Instead we just confirm the logic of has_side_panels itself.
    // Since migrate_layout would fix side=None, test the scenario where
    // panels are explicitly moved off the sidebar via options overlay.
    let state = SidebarState::new(layout);
    // After migration both panels are on Right, so right side IS present.
    // Manually set both to None to simulate the overlay hiding all panels.
    let mut state = state;
    for p in &mut state.layout.panels {
        p.side = None;
    }
    assert!(!state.has_side_panels(&SidebarSide::Right));
}

// ---------------------------------------------------------------------------
// toggle_right
// ---------------------------------------------------------------------------

#[test]
fn toggle_right_hides_and_shows_sidebar() {
    let mut state = default_state();
    assert!(state.layout.right_visible);

    let changed = state.toggle_right();
    assert!(changed);
    assert!(!state.layout.right_visible);

    let changed = state.toggle_right();
    assert!(changed);
    assert!(state.layout.right_visible);
}

#[test]
fn toggle_right_returns_false_when_no_panels_assigned() {
    let mut state = default_state();
    for p in &mut state.layout.panels { p.side = None; }
    assert!(!state.toggle_right());
}

#[test]
fn toggle_right_clears_focus_on_hide() {
    let mut state = default_state();
    state.focused_panel = Some(PanelKind::Automap);
    state.toggle_right();
    assert!(state.focused_panel.is_none());
}

// ---------------------------------------------------------------------------
// focus_next_panel
// ---------------------------------------------------------------------------

#[test]
fn focus_next_panel_starts_at_first() {
    let mut state = default_state();
    state.focus_next_panel();
    assert!(state.focused_panel.is_some());
}

#[test]
fn focus_next_panel_cycles_through_all_panels() {
    let mut state = default_state();
    let n = state.layout.panels.iter().filter(|p| p.side == Some(SidebarSide::Right)).count();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..n {
        state.focus_next_panel();
        seen.insert(state.focused_panel.clone().unwrap());
    }
    assert_eq!(seen.len(), n, "must cycle through every panel exactly once");
}

#[test]
fn focus_next_panel_wraps_around() {
    let mut state = default_state();
    let n = state.layout.panels.iter().filter(|p| p.side == Some(SidebarSide::Right)).count();
    for _ in 0..=n { state.focus_next_panel(); }
    // After n+1 calls from None we should be back at the first panel.
    let first_kind = state.layout.panels.iter()
        .find(|p| p.side == Some(SidebarSide::Right))
        .unwrap().kind.clone();
    assert_eq!(state.focused_panel.as_ref().unwrap(), &first_kind);
}

#[test]
fn focus_next_panel_none_when_sidebar_hidden() {
    let mut state = default_state();
    state.layout.right_visible = false;
    state.focus_next_panel();
    assert!(state.focused_panel.is_none());
}

// ---------------------------------------------------------------------------
// Key handling — general navigation
// ---------------------------------------------------------------------------

#[test]
fn esc_key_returns_focus_game() {
    let mut state = default_state();
    state.focused_panel = Some(PanelKind::Notes);
    let result = handle_sidebar_key(&mut state, key(KeyCode::Esc));
    assert!(matches!(result, SidebarKeyResult::FocusGame));
}

#[test]
fn f1_key_returns_focus_game() {
    let mut state = default_state();
    state.focused_panel = Some(PanelKind::Notes);
    let result = handle_sidebar_key(&mut state, key(KeyCode::F(1)));
    assert!(matches!(result, SidebarKeyResult::FocusGame));
}

#[test]
fn tab_key_cycles_focus() {
    let mut state = default_state();
    state.focused_panel = None;
    let result = handle_sidebar_key(&mut state, key(KeyCode::Tab));
    assert!(matches!(result, SidebarKeyResult::Consumed));
    assert!(state.focused_panel.is_some());
}

// ---------------------------------------------------------------------------
// Notes panel — add / edit / delete / reorder
// ---------------------------------------------------------------------------

fn notes_state(notes: &[&str]) -> SidebarState {
    let mut layout = SidebarLayout::default();
    layout.notes = notes.iter().map(|s| s.to_string()).collect();
    let mut state = SidebarState::new(layout);
    state.focused_panel = Some(PanelKind::Notes);
    state
}

#[test]
fn add_note_with_a_key_enters_editing_mode() {
    let mut state = notes_state(&[]);
    handle_sidebar_key(&mut state, key(KeyCode::Char('a')));
    assert!(state.notes_editing);
    assert!(state.notes_is_new);
}

#[test]
fn add_note_then_commit_saves_content() {
    let mut state = notes_state(&[]);
    handle_sidebar_key(&mut state, key(KeyCode::Char('n')));
    // Type some characters.
    for ch in "buy food".chars() {
        handle_sidebar_key(&mut state, key(KeyCode::Char(ch)));
    }
    let result = handle_sidebar_key(&mut state, key(KeyCode::Enter));
    assert!(matches!(result, SidebarKeyResult::SaveLayout));
    assert!(!state.notes_editing);
    assert_eq!(state.layout.notes[0], "buy food");
}

#[test]
fn add_note_then_escape_removes_empty_note() {
    let mut state = notes_state(&[]);
    handle_sidebar_key(&mut state, key(KeyCode::Char('n')));
    assert!(state.notes_editing);
    let result = handle_sidebar_key(&mut state, key(KeyCode::Esc));
    assert!(matches!(result, SidebarKeyResult::SaveLayout));
    assert!(state.layout.notes.is_empty(), "empty new note should be discarded");
}

#[test]
fn delete_note_removes_it_and_saves() {
    let mut state = notes_state(&["note A", "note B"]);
    state.panel_cursor = 0;
    let result = handle_sidebar_key(&mut state, key(KeyCode::Char('d')));
    assert!(matches!(result, SidebarKeyResult::SaveLayout));
    assert_eq!(state.layout.notes.len(), 1);
    assert_eq!(state.layout.notes[0], "note B");
}

#[test]
fn move_note_up_with_shift_k() {
    let mut state = notes_state(&["first", "second"]);
    state.panel_cursor = 1;
    let result = handle_sidebar_key(&mut state, key(KeyCode::Char('K')));
    assert!(matches!(result, SidebarKeyResult::SaveLayout));
    assert_eq!(state.layout.notes[0], "second");
    assert_eq!(state.layout.notes[1], "first");
    assert_eq!(state.panel_cursor, 0);
}

#[test]
fn move_note_down_with_shift_j() {
    let mut state = notes_state(&["first", "second"]);
    state.panel_cursor = 0;
    let result = handle_sidebar_key(&mut state, key(KeyCode::Char('J')));
    assert!(matches!(result, SidebarKeyResult::SaveLayout));
    assert_eq!(state.layout.notes[0], "second");
    assert_eq!(state.layout.notes[1], "first");
    assert_eq!(state.panel_cursor, 1);
}

#[test]
fn move_note_up_at_top_is_noop() {
    let mut state = notes_state(&["only one"]);
    state.panel_cursor = 0;
    handle_sidebar_key(&mut state, key(KeyCode::Char('K')));
    assert_eq!(state.layout.notes[0], "only one");
}

#[test]
fn cursor_is_reset_when_focus_changes() {
    let mut state = default_state();
    state.panel_cursor = 5;
    state.focus_next_panel();
    assert_eq!(state.panel_cursor, 0);
}

// ---------------------------------------------------------------------------
// Notes inline editor — cursor / backspace / home / end
// ---------------------------------------------------------------------------

#[test]
fn notes_editor_backspace_deletes_preceding_char() {
    let mut state = notes_state(&["hello"]);
    state.panel_cursor = 0;
    // Enter edit mode via 'e'.
    handle_sidebar_key(&mut state, key(KeyCode::Char('e')));
    assert!(state.notes_editing);
    assert_eq!(state.notes_edit_buf, "hello");
    assert_eq!(state.notes_edit_cursor, 5);
    // Backspace removes 'o'.
    handle_sidebar_key(&mut state, key(KeyCode::Backspace));
    assert_eq!(state.notes_edit_buf, "hell");
    assert_eq!(state.notes_edit_cursor, 4);
}

#[test]
fn notes_editor_home_moves_cursor_to_start() {
    let mut state = notes_state(&["hello"]);
    state.panel_cursor = 0;
    handle_sidebar_key(&mut state, key(KeyCode::Char('e')));
    handle_sidebar_key(&mut state, key(KeyCode::Home));
    assert_eq!(state.notes_edit_cursor, 0);
}

#[test]
fn notes_editor_end_moves_cursor_to_end() {
    let mut state = notes_state(&["hello"]);
    state.panel_cursor = 0;
    handle_sidebar_key(&mut state, key(KeyCode::Char('e')));
    // Move to start first.
    handle_sidebar_key(&mut state, key(KeyCode::Home));
    assert_eq!(state.notes_edit_cursor, 0);
    handle_sidebar_key(&mut state, key(KeyCode::End));
    assert_eq!(state.notes_edit_cursor, 5);
}

#[test]
fn notes_editor_left_right_moves_cursor() {
    let mut state = notes_state(&["hi"]);
    state.panel_cursor = 0;
    handle_sidebar_key(&mut state, key(KeyCode::Char('e')));
    assert_eq!(state.notes_edit_cursor, 2);
    handle_sidebar_key(&mut state, key(KeyCode::Left));
    assert_eq!(state.notes_edit_cursor, 1);
    handle_sidebar_key(&mut state, key(KeyCode::Right));
    assert_eq!(state.notes_edit_cursor, 2);
}

#[test]
fn notes_editor_char_input_inserts_at_cursor() {
    let mut state = notes_state(&["ac"]);
    state.panel_cursor = 0;
    handle_sidebar_key(&mut state, key(KeyCode::Char('e')));
    // Cursor is at end (pos 2). Move left once to be between 'a' and 'c'.
    handle_sidebar_key(&mut state, key(KeyCode::Left));
    handle_sidebar_key(&mut state, key(KeyCode::Char('b')));
    assert_eq!(state.notes_edit_buf, "abc");
}
