// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Tests for the map module: Direction, GMCP parsing, exits heuristic, room linking,
//! coordinate placement, and JSON persistence round-trip.

use crate::map::{
    parse_exits_line, parse_room_info_from_gmcp, Direction, WorldMap,
};

// ---------------------------------------------------------------------------
// Direction::parse
// ---------------------------------------------------------------------------

#[test]
fn direction_parse_full_names() {
    assert_eq!(Direction::parse("north"), Some(Direction::North));
    assert_eq!(Direction::parse("south"), Some(Direction::South));
    assert_eq!(Direction::parse("east"),  Some(Direction::East));
    assert_eq!(Direction::parse("west"),  Some(Direction::West));
    assert_eq!(Direction::parse("up"),    Some(Direction::Up));
    assert_eq!(Direction::parse("down"),  Some(Direction::Down));
}

#[test]
fn direction_parse_abbreviations() {
    assert_eq!(Direction::parse("n"), Some(Direction::North));
    assert_eq!(Direction::parse("s"), Some(Direction::South));
    assert_eq!(Direction::parse("e"), Some(Direction::East));
    assert_eq!(Direction::parse("w"), Some(Direction::West));
    assert_eq!(Direction::parse("u"), Some(Direction::Up));
    assert_eq!(Direction::parse("d"), Some(Direction::Down));
}

#[test]
fn direction_parse_case_insensitive() {
    assert_eq!(Direction::parse("NORTH"), Some(Direction::North));
    assert_eq!(Direction::parse("East"),  Some(Direction::East));
    assert_eq!(Direction::parse("N"),     Some(Direction::North));
}

#[test]
fn direction_parse_unknown_returns_none() {
    assert_eq!(Direction::parse("northeast"), None);
    assert_eq!(Direction::parse(""), None);
    assert_eq!(Direction::parse("x"), None);
}

// ---------------------------------------------------------------------------
// Direction::opposite
// ---------------------------------------------------------------------------

#[test]
fn direction_opposite_is_symmetric() {
    let dirs = [
        Direction::North, Direction::South,
        Direction::East,  Direction::West,
        Direction::Up,    Direction::Down,
    ];
    for d in dirs {
        assert_eq!(d.opposite().opposite(), d);
    }
}

#[test]
fn direction_opposite_pairs() {
    assert_eq!(Direction::North.opposite(), Direction::South);
    assert_eq!(Direction::East.opposite(),  Direction::West);
    assert_eq!(Direction::Up.opposite(),    Direction::Down);
}

// ---------------------------------------------------------------------------
// Direction::delta
// ---------------------------------------------------------------------------

#[test]
fn direction_delta_north_is_y_minus_one() {
    assert_eq!(Direction::North.delta(), (0, -1, 0));
}

#[test]
fn direction_delta_south_is_y_plus_one() {
    assert_eq!(Direction::South.delta(), (0, 1, 0));
}

#[test]
fn direction_delta_east_is_x_plus_one() {
    assert_eq!(Direction::East.delta(), (1, 0, 0));
}

#[test]
fn direction_delta_up_is_z_plus_one() {
    assert_eq!(Direction::Up.delta(), (0, 0, 1));
}

#[test]
fn direction_deltas_are_unit_vectors() {
    let dirs = [
        Direction::North, Direction::South,
        Direction::East,  Direction::West,
        Direction::Up,    Direction::Down,
    ];
    for d in dirs {
        let (dx, dy, dz) = d.delta();
        let magnitude = dx.abs() + dy.abs() + dz.abs();
        assert_eq!(magnitude, 1, "{d:?} delta should be a unit vector");
    }
}

#[test]
fn opposite_deltas_cancel_out() {
    let dirs = [Direction::North, Direction::East, Direction::Up];
    for d in dirs {
        let (dx, dy, dz) = d.delta();
        let (ox, oy, oz) = d.opposite().delta();
        assert_eq!((dx + ox, dy + oy, dz + oz), (0, 0, 0));
    }
}

// ---------------------------------------------------------------------------
// parse_exits_line
// ---------------------------------------------------------------------------

#[test]
fn exits_line_space_separated() {
    let dirs = parse_exits_line("Exits: north south east").unwrap();
    assert!(dirs.contains(&Direction::North));
    assert!(dirs.contains(&Direction::South));
    assert!(dirs.contains(&Direction::East));
    assert_eq!(dirs.len(), 3);
}

#[test]
fn exits_line_comma_separated() {
    let dirs = parse_exits_line("Exits: north, east, down").unwrap();
    assert!(dirs.contains(&Direction::North));
    assert!(dirs.contains(&Direction::East));
    assert!(dirs.contains(&Direction::Down));
}

#[test]
fn exits_line_abbreviations() {
    let dirs = parse_exits_line("Exits: n s e w").unwrap();
    assert_eq!(dirs.len(), 4);
}

#[test]
fn exits_line_slash_separated() {
    let dirs = parse_exits_line("Exits: north/east/down").unwrap();
    assert!(dirs.contains(&Direction::North));
    assert!(dirs.contains(&Direction::East));
    assert!(dirs.contains(&Direction::Down));
}

#[test]
fn exits_line_with_parentheses() {
    // Some MUDs print: Exits: (north) east
    let dirs = parse_exits_line("Exits: (north) east").unwrap();
    assert!(dirs.contains(&Direction::North));
    assert!(dirs.contains(&Direction::East));
}

#[test]
fn exits_line_case_insensitive_keyword() {
    let dirs = parse_exits_line("EXITS: North East").unwrap();
    assert!(dirs.contains(&Direction::North));
    assert!(dirs.contains(&Direction::East));
}

#[test]
fn exits_line_no_duplicates() {
    let dirs = parse_exits_line("Exits: north north east").unwrap();
    let north_count = dirs.iter().filter(|&&d| d == Direction::North).count();
    assert_eq!(north_count, 1, "duplicate directions must be deduplicated");
}

#[test]
fn exits_line_returns_none_when_missing() {
    assert_eq!(parse_exits_line("This is a random output line."), None);
    assert_eq!(parse_exits_line("You see a doorway to the north."), None);
}

#[test]
fn exits_line_returns_none_when_empty_after_keyword() {
    // The regex matcher must return None when there are no valid direction tokens.
    assert_eq!(parse_exits_line("Exits: none"), None);
}

#[test]
fn exits_line_strips_ansi_codes() {
    // Raw line as the MUD server might send it with colour escapes.
    let raw = "\x1b[1mExits\x1b[0m: \x1b[32mnorth\x1b[0m east";
    // apply_exits_heuristic_from_output strips ANSI before calling parse_exits_line.
    let mut map = WorldMap::default();
    map.apply_exits_heuristic_from_output(raw);
    let cur = map.current_room().expect("a room should have been created");
    assert!(cur.exits.contains_key(&Direction::North));
    assert!(cur.exits.contains_key(&Direction::East));
}

// ---------------------------------------------------------------------------
// parse_room_info_from_gmcp
// ---------------------------------------------------------------------------

#[test]
fn gmcp_room_info_with_num_field() {
    let msg = r#"Room.Info {"num":42,"name":"The Shire","exits":{"north":"43","east":"44"}}"#;
    let update = parse_room_info_from_gmcp(msg).expect("should parse");
    assert_eq!(update.id, "42");
    assert_eq!(update.name, "The Shire");
    assert_eq!(update.exits[&Direction::North], "43");
    assert_eq!(update.exits[&Direction::East], "44");
}

#[test]
fn gmcp_room_info_with_string_id_field() {
    let msg = r#"Room.Info {"id":"room-xyz","name":"Rivendell","exits":{"south":"room-abc"}}"#;
    let update = parse_room_info_from_gmcp(msg).expect("should parse");
    assert_eq!(update.id, "room-xyz");
    assert_eq!(update.name, "Rivendell");
}

#[test]
fn gmcp_room_info_missing_name_uses_fallback() {
    let msg = r#"Room.Info {"num":1,"exits":{}}"#;
    let update = parse_room_info_from_gmcp(msg).expect("should parse");
    assert_eq!(update.name, "(unknown)");
}

#[test]
fn gmcp_wrong_topic_returns_none() {
    let msg = r#"Char.Vitals {"hp":100}"#;
    assert_eq!(parse_room_info_from_gmcp(msg), None);
}

#[test]
fn gmcp_malformed_json_returns_none() {
    let msg = "Room.Info this-is-not-json";
    assert_eq!(parse_room_info_from_gmcp(msg), None);
}

#[test]
fn gmcp_no_id_field_returns_none() {
    let msg = r#"Room.Info {"name":"Somewhere","exits":{}}"#;
    assert_eq!(parse_room_info_from_gmcp(msg), None);
}

// ---------------------------------------------------------------------------
// WorldMap
// ---------------------------------------------------------------------------

fn gmcp(num: u32, name: &str, exits: &[(&str, u32)]) -> String {
    let exit_json: String = exits.iter()
        .map(|(dir, id)| format!("\"{dir}\":{id}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"Room.Info {{"num":{num},"name":"{name}","exits":{{{exit_json}}}}}"#)
}

#[test]
fn apply_gmcp_creates_room_and_sets_current() {
    let mut map = WorldMap::default();
    assert!(map.apply_gmcp_message(&gmcp(1, "Start", &[])));
    assert_eq!(map.current_room_id.as_deref(), Some("1"));
    let room = map.rooms.get("1").unwrap();
    assert_eq!(room.name, "Start");
}

#[test]
fn apply_gmcp_two_rooms_positions_second_relative_to_first() {
    let mut map = WorldMap::default();
    map.apply_gmcp_message(&gmcp(1, "South Room", &[("north", 2)]));
    map.apply_gmcp_message(&gmcp(2, "North Room", &[("south", 1)]));
    let r1 = map.rooms.get("1").unwrap();
    let r2 = map.rooms.get("2").unwrap();
    // Going north from room 1 should place room 2 one step north (y - 1).
    assert_eq!(r2.y, r1.y - 1, "room 2 should be one step north of room 1");
}

#[test]
fn apply_gmcp_updates_room_name_on_revisit() {
    let mut map = WorldMap::default();
    map.apply_gmcp_message(&gmcp(1, "First Name", &[]));
    map.apply_gmcp_message(&gmcp(1, "Updated Name", &[]));
    assert_eq!(map.rooms["1"].name, "Updated Name");
}

#[test]
fn apply_gmcp_returns_false_for_wrong_topic() {
    let mut map = WorldMap::default();
    assert!(!map.apply_gmcp_message("Char.Vitals {}"));
}

#[test]
fn link_rooms_creates_bidirectional_exit() {
    let mut map = WorldMap::default();
    map.link_rooms("A", Direction::North, "B");
    assert_eq!(map.rooms["A"].exits[&Direction::North], "B");
    assert_eq!(map.rooms["B"].exits[&Direction::South], "A");
}

#[test]
fn link_rooms_positions_target_relative_to_source() {
    let mut map = WorldMap::default();
    // Place room A explicitly.
    map.set_room_position("A", 5, 10, 0);
    map.link_rooms("A", Direction::East, "B");
    let b = map.rooms.get("B").unwrap();
    assert_eq!((b.x, b.y, b.z), (6, 10, 0));
}

#[test]
fn room_at_finds_room_by_coordinate() {
    let mut map = WorldMap::default();
    map.apply_gmcp_message(&gmcp(99, "Special", &[]));
    map.set_room_position("99", 7, -3, 2);
    let found = map.room_at(7, -3, 2).expect("room_at should find it");
    assert_eq!(found.id, "99");
}

#[test]
fn room_at_returns_none_for_empty_coordinate() {
    let map = WorldMap::default();
    assert!(map.room_at(0, 0, 0).is_none());
}

#[test]
fn heuristic_exit_creates_neighbour_rooms() {
    let mut map = WorldMap::default();
    map.apply_exits_heuristic_from_output("Exits: north east");
    let cur = map.current_room().expect("must have a current room");
    assert!(cur.exits.contains_key(&Direction::North));
    assert!(cur.exits.contains_key(&Direction::East));
}

#[test]
fn heuristic_exit_back_links_neighbour() {
    let mut map = WorldMap::default();
    map.apply_exits_heuristic_from_output("Exits: north");
    let cur_id = map.current_room_id.clone().unwrap();
    let north_id = map.rooms[&cur_id].exits[&Direction::North].clone();
    let north_room = &map.rooms[&north_id];
    assert!(north_room.exits.contains_key(&Direction::South), "back-link must exist");
}

// ---------------------------------------------------------------------------
// Map JSON persistence round-trip
// ---------------------------------------------------------------------------

#[test]
fn worldmap_serialises_and_deserialises() {
    let mut map = WorldMap::default();
    map.apply_gmcp_message(&gmcp(1, "Home", &[("east", 2)]));
    map.apply_gmcp_message(&gmcp(2, "Garden", &[("west", 1)]));

    let json = serde_json::to_string(&map).expect("serialise must succeed");
    let map2: WorldMap = serde_json::from_str(&json).expect("deserialise must succeed");

    assert_eq!(map2.rooms.len(), map.rooms.len());
    assert_eq!(map2.current_room_id, map.current_room_id);
    assert_eq!(map2.rooms["1"].name, "Home");
    assert_eq!(map2.rooms["1"].exits[&Direction::East], "2");
}
