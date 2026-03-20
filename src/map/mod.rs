// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Automap module — in-memory world graph, GMCP parsing, and JSON persistence.
//!
//! The automap tracks player movement through a MUD world as an undirected
//! weighted graph of [`Room`] nodes.  Each node records a position in a
//! three-dimensional integer grid `(x, y, z)` and a set of named exits that
//! link it to neighbouring rooms.
//!
//! # Data flow
//!
//! 1. The network layer forwards raw server output and GMCP messages to the UI.
//! 2. [`WorldMap::apply_gmcp_message`] handles `Room.Info` GMCP packets
//!    (authoritative room id, name, and exits from servers that support it).
//! 3. [`WorldMap::apply_exits_heuristic_from_output`] provides a best-effort
//!    fallback for servers without GMCP by parsing lines like
//!    `"Exits: north, east"` from the raw text stream.
//! 4. Room coordinates are inferred automatically from [`Direction::delta`]
//!    when a player transitions between two rooms.
//!
//! # Persistence
//!
//! Maps are serialised as pretty-printed JSON and stored in
//! `$XDG_DATA_HOME/durthang/<server_id>.map.json`.  [`load_server_map`] and
//! [`save_server_map`] handle reading and writing these files.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::PathBuf,
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One of the six cardinal directions supported by the automap.
///
/// The coordinate system is:
/// * `North` → `y - 1`
/// * `South` → `y + 1`
/// * `East`  → `x + 1`
/// * `West`  → `x - 1`
/// * `Up`    → `z + 1`
/// * `Down`  → `z - 1`
///
/// See [`Direction::delta`] for the exact `(dx, dy, dz)` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl Direction {
    /// Parse a direction from a string token as sent by the player or
    /// received from the server.
    ///
    /// Accepts both full names (`"north"`) and single-letter abbreviations
    /// (`"n"`), case-insensitively.  Returns `None` for unrecognised tokens.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "n" | "north" => Some(Self::North),
            "s" | "south" => Some(Self::South),
            "e" | "east" => Some(Self::East),
            "w" | "west" => Some(Self::West),
            "u" | "up" => Some(Self::Up),
            "d" | "down" => Some(Self::Down),
            _ => None,
        }
    }

    /// Return the lowercase English name of the direction (e.g. `"north"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    /// Return the logically opposite direction (`North` ↔ `South`, etc.).
    pub fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }

    /// Return the unit vector `(dx, dy, dz)` corresponding to this direction.
    ///
    /// Intended for calculating the position of a neighbouring room relative
    /// to the current one when the player moves in this direction.
    pub fn delta(self) -> (i32, i32, i32) {
        match self {
            Self::North => (0, -1, 0),
            Self::South => (0, 1, 0),
            Self::East => (1, 0, 0),
            Self::West => (-1, 0, 0),
            Self::Up => (0, 0, 1),
            Self::Down => (0, 0, -1),
        }
    }
}

/// A single room node in the world graph.
///
/// Rooms are identified by a server-assigned string `id` (from GMCP
/// `Room.Info`) or by a heuristic placeholder id when no GMCP data is
/// available.  The `(x, y, z)` coordinates are assigned locally by the
/// client and are never exchanged with the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub exits: HashMap<Direction, String>,
}

/// The in-memory world graph for a single server.
///
/// `WorldMap` stores every known room in a [`HashMap`] keyed by room id.  It
/// also remembers which room the player is currently in so that
/// [`apply_room_info`](WorldMap::apply_room_info) can infer exit coordinates
/// automatically.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorldMap {
    pub rooms: HashMap<String, Room>,
    pub current_room_id: Option<String>,
}

/// A parsed room-info update, typically originating from a GMCP `Room.Info`
/// packet.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomInfoUpdate {
    pub id: String,
    pub name: String,
    pub exits: HashMap<Direction, String>,
}

impl WorldMap {
    /// Return a reference to the room the player is currently in, or `None`
    /// if `current_room_id` has not been set yet.
    pub fn current_room(&self) -> Option<&Room> {
        self.current_room_id
            .as_ref()
            .and_then(|id| self.rooms.get(id))
    }

    /// Attempt to parse `gmcp` as a `Room.Info` GMCP message and apply it.
    ///
    /// Returns `true` on success (the map was updated), `false` if the
    /// message was not recognised or could not be parsed.
    pub fn apply_gmcp_message(&mut self, gmcp: &str) -> bool {
        let Some(update) = parse_room_info_from_gmcp(gmcp) else {
            return false;
        };
        self.apply_room_info(update);
        true
    }

    /// Apply a parsed [`RoomInfoUpdate`] to the map.
    ///
    /// * Inserts a new room entry if the id is not yet known.
    /// * Updates the room name and exit list unconditionally.
    /// * When the player was previously in a different room, calls
    ///   [`try_position_relative`](WorldMap::try_position_relative) to assign
    ///   grid coordinates to the newly entered room.
    /// * Clears any pending partial prompt, since a full room update implies
    ///   we received a complete server response.
    pub fn apply_room_info(&mut self, update: RoomInfoUpdate) {
        let prev_id = self.current_room_id.clone();

        let current = self.rooms.entry(update.id.clone()).or_insert_with(|| Room {
            id: update.id.clone(),
            name: update.name.clone(),
            x: 0,
            y: 0,
            z: 0,
            exits: HashMap::new(),
        });
        current.name = update.name;
        current.exits = update.exits.clone();

        if let Some(prev_id) = prev_id {
            if prev_id != update.id {
                self.try_position_relative(&prev_id, &update.id);
            }
        }

        self.current_room_id = Some(update.id);
    }

    /// Heuristic exit detection from raw server output.
    ///
    /// Scans `raw_line` for a pattern like `"Exits: north, east"` and, if
    /// found, registers stub exit connections from the current room.  This is
    /// the fallback path for MUD servers that do not send GMCP `Room.Info`.
    /// Stub rooms created here are given placeholder ids of the form
    /// `"heur:<current_room_id>:<direction>"`.
    pub fn apply_exits_heuristic_from_output(&mut self, raw_line: &str) {
        let clean = strip_ansi(raw_line);
        let Some(exits) = parse_exits_line(&clean) else {
            return;
        };
        if exits.is_empty() {
            return;
        }

        if self.current_room_id.is_none() {
            let id = "heuristic:start".to_string();
            self.rooms.entry(id.clone()).or_insert_with(|| Room {
                id: id.clone(),
                name: "(unknown)".to_string(),
                x: 0,
                y: 0,
                z: 0,
                exits: HashMap::new(),
            });
            self.current_room_id = Some(id);
        }

        let cur_id = self.current_room_id.clone().unwrap_or_default();
        let (cx, cy, cz) = self
            .rooms
            .get(&cur_id)
            .map(|r| (r.x, r.y, r.z))
            .unwrap_or((0, 0, 0));

        let mut room = self.rooms.remove(&cur_id).unwrap_or(Room {
            id: cur_id.clone(),
            name: "(unknown)".to_string(),
            x: cx,
            y: cy,
            z: cz,
            exits: HashMap::new(),
        });

        for dir in exits {
            let neighbor_id = format!("heur:{}:{}", cur_id, dir.as_str());
            room.exits.entry(dir).or_insert_with(|| neighbor_id.clone());

            let (dx, dy, dz) = dir.delta();
            self.rooms
                .entry(neighbor_id.clone())
                .or_insert_with(|| Room {
                    id: neighbor_id.clone(),
                    name: "(unseen)".to_string(),
                    x: cx + dx,
                    y: cy + dy,
                    z: cz + dz,
                    exits: HashMap::new(),
                });
            if let Some(nr) = self.rooms.get_mut(&neighbor_id) {
                nr.exits
                    .entry(dir.opposite())
                    .or_insert_with(|| cur_id.clone());
            }
        }

        self.rooms.insert(cur_id, room);
    }

    /// Manually override the grid position of a room.
    ///
    /// If the room does not yet exist in the map it is created with an
    /// empty exit list and an id equal to `room_id`.
    pub fn set_room_position(&mut self, room_id: &str, x: i32, y: i32, z: i32) {
        let room = self
            .rooms
            .entry(room_id.to_string())
            .or_insert_with(|| Room {
                id: room_id.to_string(),
                name: room_id.to_string(),
                x,
                y,
                z,
                exits: HashMap::new(),
            });
        room.x = x;
        room.y = y;
        room.z = z;
    }

    /// Create a bidirectional link between two rooms and infer their relative
    /// positions.
    ///
    /// Both rooms are created with empty exits and id-derived names if they
    /// do not already exist.  The reverse exit (`dir.opposite()`) is inserted
    /// in `to_id` pointing back at `from_id`.
    pub fn link_rooms(&mut self, from_id: &str, dir: Direction, to_id: &str) {
        let from = self
            .rooms
            .entry(from_id.to_string())
            .or_insert_with(|| Room {
                id: from_id.to_string(),
                name: from_id.to_string(),
                x: 0,
                y: 0,
                z: 0,
                exits: HashMap::new(),
            });
        from.exits.insert(dir, to_id.to_string());

        let to = self.rooms.entry(to_id.to_string()).or_insert_with(|| Room {
            id: to_id.to_string(),
            name: to_id.to_string(),
            x: 0,
            y: 0,
            z: 0,
            exits: HashMap::new(),
        });
        to.exits.insert(dir.opposite(), from_id.to_string());

        self.try_position_relative(from_id, to_id);
    }

    /// Find the first room whose grid coordinates are exactly `(x, y, z)`.
    ///
    /// Linear scan over all known rooms; intended for debugging and
    /// low-frequency lookups only.
    pub fn room_at(&self, x: i32, y: i32, z: i32) -> Option<&Room> {
        self.rooms
            .values()
            .find(|r| r.x == x && r.y == y && r.z == z)
    }

    /// Attempt to assign the grid position of `to_id` relative to `from_id`
    /// (or vice-versa) by consulting the exit maps of both rooms.
    ///
    /// The heuristic only moves a room that is still at the origin `(0,0,0)`,
    /// to avoid overwriting user-set or GMCP-derived coordinates.
    fn try_position_relative(&mut self, from_id: &str, to_id: &str) {
        let Some(from) = self.rooms.get(from_id).cloned() else {
            return;
        };

        let mut visited = HashSet::new();
        visited.insert((from_id.to_string(), to_id.to_string()));

        for (dir, target) in &from.exits {
            if target == to_id {
                let (dx, dy, dz) = dir.delta();
                if let Some(to) = self.rooms.get_mut(to_id) {
                    if to.x == 0 && to.y == 0 && to.z == 0 {
                        to.x = from.x + dx;
                        to.y = from.y + dy;
                        to.z = from.z + dz;
                    }
                }
                return;
            }
        }

        if let Some(to) = self.rooms.get(to_id).cloned() {
            for (dir, target) in &to.exits {
                if target == from_id {
                    let (dx, dy, dz) = dir.delta();
                    if let Some(fr) = self.rooms.get_mut(from_id) {
                        if fr.x == 0 && fr.y == 0 && fr.z == 0 {
                            fr.x = to.x + dx;
                            fr.y = to.y + dy;
                            fr.z = to.z + dz;
                        }
                    }
                    return;
                }
            }
        }
    }
}

/// Parse a GMCP `Room.Info` payload into a [`RoomInfoUpdate`].
///
/// The expected format is `"Room.Info {\"num\":123, \"name\":\"...\", \"exits\":{...}}"`.
/// Both numeric and string room ids are accepted.  Returns `None` if the
/// topic is not `Room.Info`, if the JSON cannot be parsed, or if the
/// required `id`/`num` field is missing.
pub fn parse_room_info_from_gmcp(gmcp: &str) -> Option<RoomInfoUpdate> {
    let (topic, payload) = gmcp.split_once(' ').unwrap_or((gmcp, ""));
    if topic.trim() != "Room.Info" {
        return None;
    }

    let json: Value = serde_json::from_str(payload.trim()).ok()?;
    let id = json
        .get("num")
        .or_else(|| json.get("id"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })?;
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown)")
        .to_string();

    let mut exits = HashMap::new();
    if let Some(exits_obj) = json.get("exits").and_then(|v| v.as_object()) {
        for (k, v) in exits_obj {
            if let Some(dir) = Direction::parse(k) {
                let target = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                exits.insert(dir, target);
            }
        }
    }

    Some(RoomInfoUpdate { id, name, exits })
}

/// Parse a raw (ANSI-stripped) server line for an exits listing.
///
/// Recognises lines of the form:
/// ```text
/// Exits: north, east
/// Exit - south/west
/// exit = up
/// ```
/// Returns the unique set of parsed [`Direction`]s, or `None` if the line
/// does not look like an exits announcement.
pub fn parse_exits_line(line: &str) -> Option<Vec<Direction>> {
    let re = Regex::new(r"(?i)\bexits?\b\s*[:=-]\s*(.+)$").ok()?;
    let caps = re.captures(line)?;
    let tail = caps.get(1)?.as_str();
    let norm = tail
        .replace(',', " ")
        .replace(';', " ")
        .replace('/', " ")
        .replace('(', " ")
        .replace(')', " ");

    let mut dirs = Vec::new();
    for token in norm.split_whitespace() {
        if let Some(d) = Direction::parse(token) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    if dirs.is_empty() { None } else { Some(dirs) }
}

/// Strip ANSI SGR escape sequences from a string.
///
/// Only the common `ESC [ ... m` form is stripped; other control sequences
/// are passed through unchanged.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&b) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Resolve the automap data directory (`$XDG_DATA_HOME/durthang` or
/// `~/.local/share/durthang`).
fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/share")
        });
    base.join("durthang")
}

/// Return the on-disk path for the map file belonging to `server_id`.
///
/// Characters that are not alphanumeric, `_`, or `-` are replaced with `_`
/// to ensure a valid file name on all platforms.
fn map_path(server_id: &str) -> PathBuf {
    let safe = server_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    data_dir().join(format!("{safe}.map.json"))
}

/// Load the persisted [`WorldMap`] for `server_id` from disk.
///
/// Returns an empty default [`WorldMap`] when the file does not yet exist.
/// Propagates other I/O and JSON parse errors.
pub fn load_server_map(server_id: &str) -> io::Result<WorldMap> {
    let path = map_path(server_id);
    match fs::read_to_string(path) {
        Ok(s) => {
            let map = serde_json::from_str::<WorldMap>(&s)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            Ok(map)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(WorldMap::default()),
        Err(e) => Err(e),
    }
}

/// Persist `map` for `server_id` to disk as pretty-printed JSON.
///
/// The data directory is created if it does not exist yet.
pub fn save_server_map(server_id: &str, map: &WorldMap) -> io::Result<()> {
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    let path = map_path(server_id);
    let json = serde_json::to_string_pretty(map)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    fs::write(path, json)
}
