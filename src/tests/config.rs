// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Tests for the config module: data model, TOML round-trip, sidebar layout defaults and
//! migration of stale panel data.

use crate::config::{
    Alias, Character, Config, PanelConfig, PanelKind, Server, SidebarLayout, SidebarSide, Trigger,
};

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[test]
fn server_new_fields() {
    let s = Server::new("MUME", "mume.org", 4242);
    assert_eq!(s.name, "MUME");
    assert_eq!(s.host, "mume.org");
    assert_eq!(s.port, 4242);
    assert!(!s.tls);
    assert!(!s.id.is_empty());
    assert!(s.notes.is_none());
}

#[test]
fn server_ids_are_unique() {
    let a = Server::new("A", "a.example", 23);
    let b = Server::new("B", "b.example", 23);
    assert_ne!(a.id, b.id);
}

// ---------------------------------------------------------------------------
// Character
// ---------------------------------------------------------------------------

#[test]
fn character_effective_login_falls_back_to_name() {
    let c = Character::new("Berejorn", "server-1");
    assert_eq!(c.effective_login(), "Berejorn");
}

#[test]
fn character_effective_login_uses_login_when_set() {
    let mut c = Character::new("Berejorn", "server-1");
    c.login = Some("Pommersche".to_string());
    assert_eq!(c.effective_login(), "Pommersche");
}

#[test]
fn character_new_defaults() {
    let c = Character::new("Hero", "srv");
    assert!(c.aliases.is_empty());
    assert!(c.triggers.is_empty());
    assert!(c.notes.is_none());
    assert!(c.password_hint.is_none());
    assert_eq!(c.server_id, "srv");
}

// ---------------------------------------------------------------------------
// Config round-trip (TOML serialise → deserialise)
// ---------------------------------------------------------------------------

#[test]
fn config_round_trip_via_temp_file() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("durthang_test_{}.toml", std::process::id()));

    let mut cfg = Config::default();
    cfg.servers.push(Server::new("TestServer", "test.example", 1234));
    let sid = cfg.servers[0].id.clone();
    let mut ch = Character::new("Tester", &sid);
    ch.aliases.push(Alias { name: "k".to_string(), expansion: "kill".to_string() });
    cfg.characters.push(ch);

    cfg.save(&path).expect("save failed");
    let loaded = Config::load(&path).expect("load failed");
    let _ = std::fs::remove_file(&path);

    assert_eq!(loaded.servers.len(), 1);
    assert_eq!(loaded.servers[0].name, "TestServer");
    assert_eq!(loaded.characters.len(), 1);
    assert_eq!(loaded.characters[0].aliases.len(), 1);
    assert_eq!(loaded.characters[0].aliases[0].name, "k");
    assert_eq!(loaded.characters[0].aliases[0].expansion, "kill");
}

#[test]
fn config_load_returns_default_when_file_missing() {
    let path = std::path::PathBuf::from("/tmp/durthang_nonexistent_file_xyz.toml");
    let cfg = Config::load(&path).expect("should not error");
    assert!(cfg.servers.is_empty());
    assert!(cfg.characters.is_empty());
}

#[test]
fn config_characters_for_server() {
    let mut cfg = Config::default();
    cfg.servers.push(Server::new("S1", "s1.example", 23));
    cfg.servers.push(Server::new("S2", "s2.example", 23));
    let sid1 = cfg.servers[0].id.clone();
    let sid2 = cfg.servers[1].id.clone();
    cfg.characters.push(Character::new("A", &sid1));
    cfg.characters.push(Character::new("B", &sid1));
    cfg.characters.push(Character::new("C", &sid2));

    let s1_chars = cfg.characters_for_server(&sid1);
    assert_eq!(s1_chars.len(), 2);
    let s2_chars = cfg.characters_for_server(&sid2);
    assert_eq!(s2_chars.len(), 1);
    assert_eq!(s2_chars[0].name, "C");
}

// ---------------------------------------------------------------------------
// SidebarLayout defaults
// ---------------------------------------------------------------------------

#[test]
fn sidebar_layout_default_is_visible_and_has_two_panels() {
    let layout = SidebarLayout::default();
    assert!(layout.right_visible);
    assert_eq!(layout.right_width, 26);
    assert_eq!(layout.panels.len(), 2);
    assert!(layout.panels.iter().any(|p| p.kind == PanelKind::Automap));
    assert!(layout.panels.iter().any(|p| p.kind == PanelKind::Notes));
    assert!(layout.notes.is_empty());
}

#[test]
fn sidebar_layout_default_panels_on_right_side() {
    let layout = SidebarLayout::default();
    for p in &layout.panels {
        assert_eq!(p.side, Some(SidebarSide::Right), "panel {:?} should be on Right", p.kind);
    }
}

// ---------------------------------------------------------------------------
// TOML deserialisation — stale / legacy config data
// ---------------------------------------------------------------------------

/// A config saved with the old multi-sidebar schema may have `automap` panels
/// without a `side` field (it was omitted via skip_serializing_if).
/// `migrate_layout` (called by SidebarState::new) must assign side = Right.
#[test]
fn stale_automap_without_side_is_fixed_on_load() {
    // Simulate reading back a TOML that has automap with no side.
    let toml_src = r#"
kind = "automap"
height_pct = 100
"#;
    let pc: PanelConfig = toml::from_str(toml_src).expect("deserialise failed");
    assert_eq!(pc.kind, PanelKind::Automap);
    assert_eq!(pc.side, None, "side should be None when omitted from TOML");

    // Now build a SidebarState from a layout containing this stale panel.
    use crate::ui::sidebar::SidebarState;
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![pc],
        notes: vec![],
    };
    let state = SidebarState::new(layout);
    let automap = state.layout.panels.iter().find(|p| p.kind == PanelKind::Automap)
        .expect("Automap panel must exist after migration");
    assert_eq!(automap.side, Some(SidebarSide::Right), "migrate_layout must set side to Right");
}

/// Panels with legacy kinds (char_sheet, paperdoll, inventory) must be removed
/// by migrate_layout, and both Automap + Notes must be inserted.
#[test]
fn legacy_panels_are_removed_and_defaults_inserted() {
    use crate::ui::sidebar::SidebarState;
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![
            PanelConfig { kind: PanelKind::CharSheet,  side: Some(SidebarSide::Left),  height_pct: 40 },
            PanelConfig { kind: PanelKind::Paperdoll,  side: Some(SidebarSide::Left),  height_pct: 30 },
            PanelConfig { kind: PanelKind::Inventory,  side: Some(SidebarSide::Right), height_pct: 30 },
        ],
        notes: vec![],
    };
    let state = SidebarState::new(layout);
    assert!(!state.layout.panels.iter().any(|p| {
        matches!(p.kind, PanelKind::CharSheet | PanelKind::Paperdoll | PanelKind::Inventory)
    }), "legacy panels must be removed");
    assert!(state.layout.panels.iter().any(|p| p.kind == PanelKind::Automap));
    assert!(state.layout.panels.iter().any(|p| p.kind == PanelKind::Notes));
}

/// Automap must always appear before Notes after migration.
#[test]
fn migrate_layout_puts_automap_before_notes() {
    use crate::ui::sidebar::SidebarState;
    // Give Notes first so migration must swap.
    let layout = SidebarLayout {
        right_visible: true,
        right_width: 26,
        panels: vec![
            PanelConfig { kind: PanelKind::Notes,   side: Some(SidebarSide::Right), height_pct: 65 },
            PanelConfig { kind: PanelKind::Automap, side: Some(SidebarSide::Right), height_pct: 35 },
        ],
        notes: vec![],
    };
    let state = SidebarState::new(layout);
    let ai = state.layout.panels.iter().position(|p| p.kind == PanelKind::Automap).unwrap();
    let ni = state.layout.panels.iter().position(|p| p.kind == PanelKind::Notes).unwrap();
    assert!(ai < ni, "Automap must come before Notes");
}

// ---------------------------------------------------------------------------
// Trigger constructor
// ---------------------------------------------------------------------------

#[test]
fn trigger_new_sets_pattern_and_empty_optionals() {
    let t = Trigger::new("You are hungry");
    assert_eq!(t.pattern, "You are hungry");
    assert!(t.color.is_none());
    assert!(t.send.is_none());
    assert!(!t.id.is_empty());
}

#[test]
fn trigger_ids_are_unique() {
    let a = Trigger::new("pat");
    let b = Trigger::new("pat");
    assert_ne!(a.id, b.id);
}
