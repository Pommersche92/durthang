//! Game view — the main screen shown while connected to a MUD server.
//!
//! Layout:
//!   ┌─ Server Name ──────────────────────────────┐
//!   │                                            │
//!   │  [scrollable output / scrollback buffer]   │
//!   │                                            │
//!   └─────────────── ↑42 lines ─────────────────┘
//!   ▶ input line with cursor█
//!   ● ServerName / CharName   lat 12ms   Ctrl+Q disconnect

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};
use tracing::warn;

use crate::config::{Alias, SidebarSide, Trigger as TriggerCfg};
use crate::map::{Direction, WorldMap};
use crate::ui::sidebar::{self, SidebarKeyResult, SidebarState};

// ---------------------------------------------------------------------------
// Public action type
// ---------------------------------------------------------------------------

/// Actions returned by [`handle_key`] to the main event loop.
#[derive(Debug)]
pub enum GameAction {
    /// Send a line to the server.
    SendLine(String),
    /// Gracefully disconnect and return to the selection screen.
    Disconnect,
    /// Quit the application.
    Quit,
    /// Copy text to the system clipboard via the OSC 52 escape sequence.
    CopyToClipboard(String),
    /// Persist a new (or updated) alias for the connected character.
    AddAlias { name: String, expansion: String },
    /// Delete an alias by name.
    RemoveAlias(String),
    /// Persist a new trigger for the connected character.
    AddTrigger { pattern: String, color: Option<String>, send: Option<String> },
    /// Delete a trigger whose id starts with the given prefix.
    RemoveTrigger(String),
    /// The sidebar layout was changed and should be persisted to the character config.
    SaveSidebarLayout,
}

// ---------------------------------------------------------------------------
// Compiled trigger (runtime representation)
// ---------------------------------------------------------------------------

struct CompiledTrigger {
    id: String,
    regex: Regex,
    color: Option<Color>,
    send: Option<String>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of lines kept in the scrollback buffer.
const SCROLLBACK_MAX: usize = 5_000;
/// Number of latency samples used for the rolling average.
const LATENCY_AVG_SAMPLES: usize = 8;

// ---------------------------------------------------------------------------
// GameState
// ---------------------------------------------------------------------------

pub struct GameState {
    /// Decoded + ANSI-parsed scrollback lines.
    pub lines: VecDeque<Line<'static>>,
    /// Number of logical lines we're scrolled up from the bottom (0 = live view).
    pub scroll_offset: usize,
    /// Current input line content.
    pub input: String,
    /// Byte offset of the cursor within `input`.
    pub input_cursor: usize,
    /// Sent-command history (oldest → newest).
    pub history: Vec<String>,
    /// Index into `history` while browsing (None = live edit).
    pub history_idx: Option<usize>,
    /// Snapshot of input before we started history browsing.
    history_snapshot: String,
    /// Latest latency reading in ms.
    pub latency: Option<u64>,
    /// Recent latency samples for the rolling average shown in the status bar.
    latency_samples: VecDeque<u64>,
    /// True while the connection is alive.
    pub connected: bool,
    /// Partial prompt line received without a trailing newline.
    pub prompt: Option<Line<'static>>,
    /// Height of the output inner area (updated every draw call, used for PgUp/PgDn).
    last_output_height: usize,
    // ---- Phase 6 additions ----
    /// Per-character aliases loaded at session start.
    pub aliases: Vec<Alias>,
    /// Compiled trigger patterns for this session.
    triggers: Vec<CompiledTrigger>,
    /// Lines queued for auto-send by trigger actions; drained by the main loop.
    pub auto_send_queue: Vec<String>,
    /// Whether copy mode is currently active.
    pub copy_mode: bool,
    /// Fullscreen map overlay mode.
    pub map_fullscreen: bool,
    /// Manual pan offset for fullscreen map viewport.
    pub map_pan: (i32, i32, i32),
    /// Sidebar panel state (data + focus + layout).
    pub sidebar: SidebarState,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::new(),
            scroll_offset: 0,
            input: String::new(),
            input_cursor: 0,
            history: Vec::new(),
            history_idx: None,
            history_snapshot: String::new(),
            latency: None,
            latency_samples: VecDeque::new(),
            connected: false,
            prompt: None,
            last_output_height: 20,
            aliases: Vec::new(),
            triggers: Vec::new(),
            auto_send_queue: Vec::new(),
            copy_mode: false,
            map_fullscreen: false,
            map_pan: (0, 0, 0),
            sidebar: SidebarState::default(),
        }
    }

    /// Parse an ANSI string, apply trigger highlighting, and append to the scrollback buffer.
    /// Any trigger auto-sends are pushed to `auto_send_queue`.
    pub fn push_line(&mut self, s: &str) {
        let mut line = ansi_to_line(s);
        // Apply triggers.
        for trigger in &self.triggers {
            if trigger.regex.is_match(s) {
                if let Some(color) = trigger.color {
                    line = Line::from(
                        line.spans
                            .into_iter()
                            .map(|sp| Span::styled(sp.content, sp.style.fg(color)))
                            .collect::<Vec<_>>(),
                    );
                }
                if let Some(cmd) = &trigger.send {
                    if !cmd.is_empty() {
                        self.auto_send_queue.push(cmd.clone());
                    }
                }
            }
        }
        self.lines.push_back(line);
        while self.lines.len() > SCROLLBACK_MAX {
            self.lines.pop_front();
        }
        // A full line supersedes any pending prompt.
        self.prompt = None;
    }

    /// Store a partial line (MUD prompt) shown just below the last full line.
    pub fn push_prompt(&mut self, s: &str) {
        self.prompt = Some(ansi_to_line(s));
    }

    // ------------------------------------------------------------------
    // Scrollback
    // ------------------------------------------------------------------

    pub fn scroll_up(&mut self, n: usize) {
        let max = self.lines.len().saturating_sub(1);
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(max);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = self.lines.len().saturating_sub(1);
    }

    // ------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            None => {
                // Save live input before we start browsing.
                self.history_snapshot = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        self.input = self.history[idx].clone();
        self.input_cursor = self.input.len();
    }

    fn history_next(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_idx = None;
                self.input = self.history_snapshot.clone();
                self.input_cursor = self.input.len();
            }
            Some(i) => {
                let idx = i + 1;
                self.history_idx = Some(idx);
                self.input = self.history[idx].clone();
                self.input_cursor = self.input.len();
            }
        }
    }

    // ------------------------------------------------------------------
    // Input confirm
    // ------------------------------------------------------------------

    /// Finalise the input, push to history, expand aliases, and return the
    /// action to perform.
    fn confirm_input(&mut self) -> Option<GameAction> {
        let raw = self.input.trim_end().to_string();
        self.input.clear();
        self.input_cursor = 0;
        self.history_idx = None;
        self.history_snapshot.clear();

        if raw.is_empty() {
            return None;
        }

        // Always record raw input in history.
        if self.history.last().map(|l| l != &raw).unwrap_or(true) {
            self.history.push(raw.clone());
        }

        // Meta-commands start with '/'.
        if raw.starts_with('/') {
            return self.handle_meta_command(&raw);
        }

        // Expand aliases.
        let line = self.expand_alias(&raw);

        // Echo into scrollback.
        let echo = Line::from(vec![
            Span::styled("▶ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(line.clone()),
        ]);
        self.lines.push_back(echo);
        while self.lines.len() > SCROLLBACK_MAX {
            self.lines.pop_front();
        }

        Some(GameAction::SendLine(line))
    }

    // ------------------------------------------------------------------
    // Reset on new connection
    // ------------------------------------------------------------------

    pub fn on_connect(&mut self) {
        self.connected = true;
        self.latency = None;
        self.latency_samples.clear();
        self.map_fullscreen = false;
        self.map_pan = (0, 0, 0);
        self.scroll_to_bottom();
        self.prompt = None;
    }

    pub fn on_disconnect(&mut self) {
        self.connected = false;
        self.latency = None;
        self.latency_samples.clear();
        self.map_fullscreen = false;
        self.map_pan = (0, 0, 0);
    }

    pub fn record_latency(&mut self, ms: u64) {
        self.latency = Some(ms);
        self.latency_samples.push_back(ms);
        while self.latency_samples.len() > LATENCY_AVG_SAMPLES {
            self.latency_samples.pop_front();
        }
    }

    fn latency_avg(&self) -> Option<u64> {
        if self.latency_samples.is_empty() {
            return None;
        }
        let sum: u64 = self.latency_samples.iter().copied().sum();
        let count = u64::try_from(self.latency_samples.len()).unwrap_or(1);
        Some(sum / count)
    }

    // ------------------------------------------------------------------
    // Aliases & triggers
    // ------------------------------------------------------------------

    pub fn set_aliases(&mut self, aliases: Vec<Alias>) {
        self.aliases = aliases;
    }

    /// Compile and install trigger rules for this session.
    /// Invalid regexes are logged as warnings and skipped.
    pub fn set_triggers(&mut self, triggers: Vec<TriggerCfg>) {
        self.triggers = triggers
            .into_iter()
            .filter_map(|t| match Regex::new(&t.pattern) {
                Ok(regex) => Some(CompiledTrigger {
                    id: t.id,
                    regex,
                    color: t.color.as_deref().and_then(parse_color_name),
                    send: t.send,
                }),
                Err(e) => {
                    warn!("Invalid trigger regex {:?}: {e}", t.pattern);
                    None
                }
            })
            .collect();
    }

    fn expand_alias(&self, line: &str) -> String {
        // Exact-line match first, then first-word match (with tail appended).
        if let Some(a) = self.aliases.iter().find(|a| a.name == line) {
            return a.expansion.clone();
        }
        let first = line.split_whitespace().next().unwrap_or("");
        if let Some(a) = self.aliases.iter().find(|a| a.name == first) {
            let tail = &line[first.len()..];
            return format!("{}{tail}", a.expansion);
        }
        line.to_string()
    }

    // ------------------------------------------------------------------
    // System messages
    // ------------------------------------------------------------------

    /// Push a client-side informational message into the scrollback buffer.
    pub fn push_system(&mut self, msg: &str) {
        let line = Line::from(vec![
            Span::styled("\u{00bb} ", Style::default().fg(Color::Cyan)),
            Span::styled(msg.to_string(), Style::default().fg(Color::Cyan)),
        ]);
        self.lines.push_back(line);
        while self.lines.len() > SCROLLBACK_MAX {
            self.lines.pop_front();
        }
    }

    // ------------------------------------------------------------------
    // Meta-command parsing  (/alias, /trigger, /disconnect, /quit, …)
    // ------------------------------------------------------------------

    fn handle_meta_command(&mut self, input: &str) -> Option<GameAction> {
        let stripped = &input[1..];
        let (cmd, args) = stripped
            .split_once(' ')
            .map(|(c, a)| (c, a.trim()))
            .unwrap_or((stripped, ""));

        match cmd.to_lowercase().as_str() {
            "disconnect" | "disc" => Some(GameAction::Disconnect),
            "quit" | "exit"      => Some(GameAction::Quit),

            // ------ Sidebar visibility (/sidebar) ----------------------------
            "sidebar" | "sb" => {
                match args {
                    "right" | "r" => {
                        self.sidebar.toggle_right();
                        let st = if self.sidebar.layout.right_visible { "shown" } else { "hidden" };
                        self.push_system(&format!("Right sidebar {st}."));
                        Some(GameAction::SaveSidebarLayout)
                    }
                    _ => {
                        let r = if self.sidebar.layout.right_visible { "shown" } else { "hidden" };
                        self.push_system(&format!("Right sidebar: {r}  /sidebar right to toggle"));
                        None
                    }
                }
            }

            // ------ Automap controls (/map) ---------------------------------
            "map" => {
                let (sub, rest) = args
                    .split_once(' ')
                    .map(|(s, r)| (s, r.trim()))
                    .unwrap_or((args, ""));

                match sub {
                    "" | "show" => {
                        if let Some(room) = self.sidebar.automap.current_room() {
                            self.push_system(&format!(
                                "Map current: id={}  name='{}'  pos=({}, {}, {})  exits={} ",
                                room.id,
                                room.name,
                                room.x,
                                room.y,
                                room.z,
                                room.exits.len()
                            ));
                        } else {
                            self.push_system("Map: no current room yet.");
                        }
                        None
                    }
                    "setpos" => {
                        let parts: Vec<&str> = rest.split_whitespace().collect();
                        if parts.len() < 3 {
                            self.push_system("Usage: /map setpos <room_id|current> <x> <y> [z]");
                            return None;
                        }
                        let room_id = if parts[0] == "current" {
                            self.sidebar.map_current_room_id().unwrap_or_else(|| "current".to_string())
                        } else {
                            parts[0].to_string()
                        };
                        let x = parts[1].parse::<i32>();
                        let y = parts[2].parse::<i32>();
                        let z = if parts.len() >= 4 {
                            parts[3].parse::<i32>()
                        } else {
                            Ok(0)
                        };
                        match (x, y, z) {
                            (Ok(x), Ok(y), Ok(z)) => {
                                self.sidebar.map_set_position(&room_id, x, y, z);
                                self.push_system(&format!("Map: set {room_id} -> ({x}, {y}, {z})"));
                            }
                            _ => self.push_system("Usage: /map setpos <room_id|current> <x> <y> [z]"),
                        }
                        None
                    }
                    "full" | "fullscreen" | "fs" => {
                        self.map_fullscreen = !self.map_fullscreen;
                        if self.map_fullscreen {
                            self.map_pan = (0, 0, 0);
                            self.push_system("Map fullscreen enabled.  Arrows pan, u/d z-level, c center, Esc/F6 close.");
                        } else {
                            self.push_system("Map fullscreen disabled.");
                        }
                        None
                    }
                    "link" => {
                        let parts: Vec<&str> = rest.split_whitespace().collect();
                        if parts.len() != 3 {
                            self.push_system("Usage: /map link <from_id|current> <dir> <to_id>");
                            return None;
                        }
                        let from_id = if parts[0] == "current" {
                            self.sidebar.map_current_room_id().unwrap_or_else(|| "current".to_string())
                        } else {
                            parts[0].to_string()
                        };
                        let Some(dir) = Direction::parse(parts[1]) else {
                            self.push_system("Direction must be one of n/s/e/w/u/d.");
                            return None;
                        };
                        let to_id = parts[2].to_string();
                        self.sidebar.map_link_rooms(&from_id, dir, &to_id);
                        self.push_system(&format!("Map: linked {from_id} --{}--> {to_id}", dir.as_str()));
                        None
                    }
                    _ => {
                        self.push_system("Usage: /map [show|fullscreen|setpos <room_id|current> <x> <y> [z]|link <from_id|current> <dir> <to_id>]");
                        None
                    }
                }
            }

            "alias" | "al" => {
                if args.is_empty() {
                    if self.aliases.is_empty() {
                        self.push_system("No aliases defined.  Usage: /alias <name> <expansion>");
                    } else {
                        self.push_system("Aliases:");
                        let msgs: Vec<String> = self.aliases.iter()
                            .map(|a| format!("  {} \u{2192} {}", a.name, a.expansion))
                            .collect();
                        for m in msgs { self.push_system(&m); }
                    }
                    None
                } else {
                    match args.split_once(' ') {
                        None => {
                            match self.aliases.iter().find(|a| a.name == args) {
                                Some(a) => self.push_system(&format!("  {} \u{2192} {}", a.name, a.expansion)),
                                None    => self.push_system(&format!("No alias named '{args}'."),),
                            }
                            None
                        }
                        Some((name, expansion)) => Some(GameAction::AddAlias {
                            name: name.to_string(),
                            expansion: expansion.to_string(),
                        }),
                    }
                }
            }

            "unalias" | "unal" => {
                if args.is_empty() {
                    self.push_system("Usage: /unalias <name>");
                    None
                } else {
                    Some(GameAction::RemoveAlias(args.to_string()))
                }
            }

            "trigger" | "trig" => {
                let (sub, rest) = args
                    .split_once(' ')
                    .map(|(s, r)| (s, r.trim()))
                    .unwrap_or((args, ""));

                match sub {
                    "" | "list" => {
                        if self.triggers.is_empty() {
                            self.push_system("No triggers defined.  Usage: /trigger add <pattern> [color=NAME] [send=CMD]");
                        } else {
                            self.push_system("Triggers:");
                            let items: Vec<String> = self.triggers.iter().map(|t| {
                                let id_short = &t.id[..8.min(t.id.len())];
                                let send = t.send.as_deref().unwrap_or("-");
                                format!("  [{id_short}]  /{}/ send={send}", t.regex.as_str())
                            }).collect();
                            for s in items { self.push_system(&s); }
                        }
                        None
                    }
                    "add" => {
                        if rest.is_empty() {
                            self.push_system("Usage: /trigger add <pattern> [color=NAME] [send=CMD]");
                            None
                        } else {
                            let (pattern, color, send) = parse_trigger_args(rest);
                            Some(GameAction::AddTrigger { pattern, color, send })
                        }
                    }
                    "del" | "rm" | "remove" | "delete" => {
                        if rest.is_empty() {
                            self.push_system("Usage: /trigger del <id>");
                            None
                        } else {
                            Some(GameAction::RemoveTrigger(rest.to_string()))
                        }
                    }
                    _ => {
                        self.push_system("Usage: /trigger list | /trigger add <pattern> [color=NAME] [send=CMD] | /trigger del <id>");
                        None
                    }
                }
            }

            _ => {
                self.push_system(&format!("Unknown command: {input}"));
                self.push_system("  Available: /alias, /unalias, /trigger, /sidebar, /map, /disconnect, /quit");
                None
            }
        }
    }
} // end impl GameState

// ---------------------------------------------------------------------------
// ANSI / VT100 parser
// ---------------------------------------------------------------------------

/// Convert a string that may contain ANSI SGR escape codes into a ratatui
/// `Line<'static>` with per-span styles applied.
fn ansi_to_line(input: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let bytes = input.as_bytes();
    let mut text_start = 0;
    let mut i = 0;

    while i < input.len() {
        // Look for ESC [  (CSI — Control Sequence Introducer)
        if bytes[i] == b'\x1b' && i + 1 < input.len() && bytes[i + 1] == b'[' {
            // Flush accumulated plain text.
            if text_start < i {
                spans.push(Span::styled(input[text_start..i].to_string(), style));
            }

            // Find the end of the escape sequence.
            // Per ANSI X3.64 the final byte is in the range 0x40–0x7E ('@'–'~').
            // Parameter/intermediate bytes (0x20–0x3F) precede it.  We also
            // stop if we hit another ESC (0x1B) — that means this CSI was
            // truncated and a new escape sequence starts.
            let seq_start = i + 2;
            let seq_end = bytes[seq_start..]
                .iter()
                .position(|&b| (0x40..=0x7E).contains(&b) || b == 0x1b)
                .map(|j| seq_start + j)
                .unwrap_or(input.len());

            if seq_end < input.len() && bytes[seq_end] != 0x1b {
                let terminator = bytes[seq_end];
                let params = &input[seq_start..seq_end];
                if terminator == b'm' {
                    style = apply_sgr(style, params);
                }
                // All other escape sequences (cursor movement, mode switches, etc.)
                // are silently skipped.
                i = seq_end + 1;
            } else {
                // Unterminated CSI (hit another ESC or end of input) — skip
                // the broken params; the next iteration picks up the new ESC.
                i = seq_end;
            }
            text_start = i;
        } else if bytes[i] == b'\x1b' {
            // Non-CSI ESC sequence (no '[' after ESC).
            // These are always terminal control sequences, never printable text.
            // Skip the ESC plus its payload:
            //   - ESC <single byte>          (e.g. ESC= ESC> ESC7 ESC8)
            //   - ESC ( X  ESC ) X  etc.    (G0/G1 charset designation, 3 bytes total)
            if text_start < i {
                spans.push(Span::styled(input[text_start..i].to_string(), style));
            }
            i += 1; // skip ESC
            if i < bytes.len() {
                let next = bytes[i];
                i += 1; // skip the byte immediately after ESC
                // Charset-designation sequences have one additional designator byte.
                // Don't consume the next byte if it is ESC — that starts a
                // new escape sequence and must not be swallowed as a charset code.
                if matches!(next, b'(' | b')' | b'*' | b'+') && i < bytes.len() && bytes[i] != 0x1b {
                    i += 1; // skip the charset code (e.g. 'B', '0', 'U', …)
                }
            }
            text_start = i;
        } else {
            // Advance by one UTF-8 character.
            i += input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
    }

    // Flush remaining text.
    if text_start < input.len() {
        spans.push(Span::styled(input[text_start..].to_string(), style));
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    Line::from(spans)
}

/// Apply one or more SGR parameter codes to a style and return the result.
fn apply_sgr(mut style: Style, params: &str) -> Style {
    if params.is_empty() {
        return Style::default();
    }

    let codes: Vec<u32> = params
        .split(';')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            5 | 6 => style = style.add_modifier(Modifier::SLOW_BLINK),
            7 => style = style.add_modifier(Modifier::REVERSED),
            9 => style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => {
                style = style
                    .remove_modifier(Modifier::BOLD)
                    .remove_modifier(Modifier::DIM);
            }
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            25 => style = style.remove_modifier(Modifier::SLOW_BLINK),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            29 => style = style.remove_modifier(Modifier::CROSSED_OUT),

            // Standard foreground colors (ANSI 30-37 → ratatui equivalents)
            30 => style = style.fg(Color::Black),
            31 => style = style.fg(Color::Red),
            32 => style = style.fg(Color::Green),
            33 => style = style.fg(Color::Yellow),
            34 => style = style.fg(Color::Blue),
            35 => style = style.fg(Color::Magenta),
            36 => style = style.fg(Color::Cyan),
            37 => style = style.fg(Color::Gray), // "white" in standard palette
            38 => {
                if i + 2 < codes.len() && codes[i + 1] == 5 {
                    // 256-color fg: ESC[38;5;Nm
                    style = style.fg(Color::Indexed(codes[i + 2] as u8));
                    i += 2;
                } else if i + 4 < codes.len() && codes[i + 1] == 2 {
                    // True-color fg: ESC[38;2;R;G;Bm
                    style = style.fg(Color::Rgb(
                        codes[i + 2] as u8,
                        codes[i + 3] as u8,
                        codes[i + 4] as u8,
                    ));
                    i += 4;
                }
            }
            39 => style = style.fg(Color::Reset),

            // Standard background colors
            40 => style = style.bg(Color::Black),
            41 => style = style.bg(Color::Red),
            42 => style = style.bg(Color::Green),
            43 => style = style.bg(Color::Yellow),
            44 => style = style.bg(Color::Blue),
            45 => style = style.bg(Color::Magenta),
            46 => style = style.bg(Color::Cyan),
            47 => style = style.bg(Color::Gray),
            48 => {
                if i + 2 < codes.len() && codes[i + 1] == 5 {
                    style = style.bg(Color::Indexed(codes[i + 2] as u8));
                    i += 2;
                } else if i + 4 < codes.len() && codes[i + 1] == 2 {
                    style = style.bg(Color::Rgb(
                        codes[i + 2] as u8,
                        codes[i + 3] as u8,
                        codes[i + 4] as u8,
                    ));
                    i += 4;
                }
            }
            49 => style = style.bg(Color::Reset),

            // Bright / high-intensity foreground (ANSI 90-97)
            90 => style = style.fg(Color::DarkGray),
            91 => style = style.fg(Color::LightRed),
            92 => style = style.fg(Color::LightGreen),
            93 => style = style.fg(Color::LightYellow),
            94 => style = style.fg(Color::LightBlue),
            95 => style = style.fg(Color::LightMagenta),
            96 => style = style.fg(Color::LightCyan),
            97 => style = style.fg(Color::White), // bright white

            // Bright background (ANSI 100-107)
            100 => style = style.bg(Color::DarkGray),
            101 => style = style.bg(Color::LightRed),
            102 => style = style.bg(Color::LightGreen),
            103 => style = style.bg(Color::LightYellow),
            104 => style = style.bg(Color::LightBlue),
            105 => style = style.bg(Color::LightMagenta),
            106 => style = style.bg(Color::LightCyan),
            107 => style = style.bg(Color::White),

            _ => {} // Unknown SGR codes are silently ignored.
        }
        i += 1;
    }

    style
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

/// Handle a key event in game mode.
///
/// Returns `Some(GameAction)` when an action needs to be taken by the caller.
/// Returns `None` for all other keystrokes (pure editing operations).
pub fn handle_key(state: &mut GameState, key: KeyEvent) -> Option<GameAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if key.code == KeyCode::F(6) {
        state.map_fullscreen = !state.map_fullscreen;
        if state.map_fullscreen {
            state.map_pan = (0, 0, 0);
        }
        return None;
    }

    if state.map_fullscreen {
        return handle_map_fullscreen_mode(state, key);
    }

    // F1 — return keyboard focus to the game input.
    if key.code == KeyCode::F(1) {
        state.sidebar.focused_panel = None;
        return None;
    }

    // F-key sidebar controls.
    if let KeyCode::F(n) = key.code {
        match n {
            3 => {
                if state.sidebar.toggle_right() {
                    return Some(GameAction::SaveSidebarLayout);
                }
                return None;
            }
            4 => {
                state.sidebar.focus_next_panel();
                return None;
            }
            _ => return None,
        }
    }

    // Ctrl+Q — disconnect and return to the selection screen.
    if ctrl && key.code == KeyCode::Char('q') {
        return Some(GameAction::Disconnect);
    }

    // In copy mode most keys have different bindings.
    if state.copy_mode {
        return handle_copy_mode(state, key);
    }

    // When a sidebar panel is focused, route input there.
    if state.sidebar.focused_panel.is_some() {
        return match sidebar::handle_sidebar_key(&mut state.sidebar, key) {
            SidebarKeyResult::FocusGame  => { state.sidebar.focused_panel = None; None }
            SidebarKeyResult::SaveLayout => Some(GameAction::SaveSidebarLayout),
            SidebarKeyResult::Consumed | SidebarKeyResult::Unhandled => None,
        };
    }

    match key.code {
        // ---- Confirm / send ----
        KeyCode::Enter => {
            state.scroll_to_bottom();
            state.confirm_input()
        }

        // ---- Scrollback ----
        KeyCode::PageUp => {
            let n = state.last_output_height.max(1);
            state.scroll_up(n);
            None
        }
        KeyCode::PageDown => {
            let n = state.last_output_height.max(1);
            state.scroll_down(n);
            None
        }
        KeyCode::Home if ctrl => {
            state.scroll_to_top();
            None
        }
        KeyCode::End if ctrl => {
            state.scroll_to_bottom();
            None
        }

        // ---- History navigation (Up/Down without Ctrl) ----
        KeyCode::Up if !ctrl => {
            state.history_prev();
            None
        }
        KeyCode::Down if !ctrl => {
            state.history_next();
            None
        }

        // ---- Scrollback with Ctrl+Up / Ctrl+Down ----
        KeyCode::Up if ctrl => {
            state.scroll_up(1);
            None
        }
        KeyCode::Down if ctrl => {
            state.scroll_down(1);
            None
        }

        // ---- Enter copy mode ----
        KeyCode::Char('y') if ctrl => {
            state.copy_mode = true;
            None
        }

        // ---- Cursor movement ----
        KeyCode::Home if !ctrl => {
            state.input_cursor = 0;
            None
        }
        KeyCode::End if !ctrl => {
            state.input_cursor = state.input.len();
            None
        }
        KeyCode::Left => {
            if state.input_cursor > 0 {
                let prev = state.input[..state.input_cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                state.input_cursor = prev;
            }
            None
        }
        KeyCode::Right => {
            if state.input_cursor < state.input.len() {
                let ch = state.input[state.input_cursor..].chars().next().unwrap();
                state.input_cursor += ch.len_utf8();
            }
            None
        }

        // ---- Editing ----
        KeyCode::Backspace => {
            if state.input_cursor > 0 {
                let prev = state.input[..state.input_cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                state.input.remove(prev);
                state.input_cursor = prev;
            }
            None
        }
        KeyCode::Delete => {
            if state.input_cursor < state.input.len() {
                state.input.remove(state.input_cursor);
            }
            None
        }

        // ---- Readline-style shortcuts ----
        KeyCode::Char('u') if ctrl => {
            state.input.drain(..state.input_cursor);
            state.input_cursor = 0;
            None
        }
        KeyCode::Char('k') if ctrl => {
            state.input.truncate(state.input_cursor);
            None
        }
        KeyCode::Char('a') if ctrl => {
            state.input_cursor = 0;
            None
        }
        KeyCode::Char('e') if ctrl => {
            state.input_cursor = state.input.len();
            None
        }
        KeyCode::Char('w') if ctrl => {
            // Delete the word before the cursor (like readline Ctrl+W).
            let before = &state.input[..state.input_cursor];
            let trim_end = before.trim_end_matches(|c: char| !c.is_whitespace()).len();
            let new_end = before[..trim_end.min(before.len())]
                .trim_end_matches(|c: char| c.is_whitespace())
                .len();
            state.input.drain(new_end..state.input_cursor);
            state.input_cursor = new_end;
            None
        }

        // ---- Regular character input ----
        KeyCode::Char(c) if !ctrl => {
            state.input.insert(state.input_cursor, c);
            state.input_cursor += c.len_utf8();
            state.history_idx = None;
            None
        }

        _ => None,
    }
}

/// Handle a key event while copy mode is active.
fn handle_copy_mode(state: &mut GameState, key: KeyEvent) -> Option<GameAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.copy_mode = false;
            None
        }
        KeyCode::Up                 => { state.scroll_up(1);  None }
        KeyCode::Down               => { state.scroll_down(1); None }
        KeyCode::PageUp             => { let n = state.last_output_height.max(1); state.scroll_up(n); None }
        KeyCode::PageDown           => { let n = state.last_output_height.max(1); state.scroll_down(n); None }
        KeyCode::Home if ctrl       => { state.scroll_to_top(); None }
        KeyCode::End  if ctrl       => { state.scroll_to_bottom(); None }
        KeyCode::Char('y')          => {
            // Yank (copy) the bottom-most visible line.
            let buf_len = state.lines.len();
            let scroll  = state.scroll_offset.min(buf_len.saturating_sub(1));
            let idx     = buf_len.saturating_sub(1 + scroll);
            let text: String = state
                .lines
                .get(idx)
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .unwrap_or_default();
            state.copy_mode = false;
            if text.is_empty() { None } else { Some(GameAction::CopyToClipboard(text)) }
        }
        _ => None,
    }
}

fn handle_map_fullscreen_mode(state: &mut GameState, key: KeyEvent) -> Option<GameAction> {
    match key.code {
        KeyCode::Esc | KeyCode::F(6) => {
            state.map_fullscreen = false;
            None
        }
        KeyCode::Left => {
            state.map_pan.0 -= 1;
            None
        }
        KeyCode::Right => {
            state.map_pan.0 += 1;
            None
        }
        KeyCode::Up => {
            state.map_pan.1 -= 1;
            None
        }
        KeyCode::Down => {
            state.map_pan.1 += 1;
            None
        }
        KeyCode::PageUp | KeyCode::Char('u') => {
            state.map_pan.2 += 1;
            None
        }
        KeyCode::PageDown | KeyCode::Char('d') => {
            state.map_pan.2 -= 1;
            None
        }
        KeyCode::Char('c') => {
            state.map_pan = (0, 0, 0);
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Trigger / color helpers
// ---------------------------------------------------------------------------

fn parse_color_name(name: &str) -> Option<Color> {
    match name.to_lowercase().as_str() {
        "black"                    => Some(Color::Black),
        "red"                      => Some(Color::Red),
        "green"                    => Some(Color::Green),
        "yellow"                   => Some(Color::Yellow),
        "blue"                     => Some(Color::Blue),
        "magenta"                  => Some(Color::Magenta),
        "cyan"                     => Some(Color::Cyan),
        "gray" | "grey"            => Some(Color::Gray),
        "dark_gray" | "darkgray"   => Some(Color::DarkGray),
        "light_red"                => Some(Color::LightRed),
        "light_green"              => Some(Color::LightGreen),
        "light_yellow"             => Some(Color::LightYellow),
        "light_blue"               => Some(Color::LightBlue),
        "light_magenta"            => Some(Color::LightMagenta),
        "light_cyan"               => Some(Color::LightCyan),
        "white"                    => Some(Color::White),
        _                          => None,
    }
}

/// Parse `color=NAME` and `send=CMD` key=value suffixes from a trigger add string.
/// Returns (pattern, color, send).
fn parse_trigger_args(s: &str) -> (String, Option<String>, Option<String>) {
    let mut rest  = s.to_string();
    let mut color = None;
    let mut send  = None;

    // `send=` is parsed first because it may contain spaces (consumed to end).
    if let Some(idx) = find_kv(&rest, "send") {
        send = Some(rest[idx + "send=".len()..].trim().to_string());
        rest = rest[..idx].trim().to_string();
    }
    if let Some(idx) = find_kv(&rest, "color") {
        let val: &str = rest[idx + "color=".len()..].split_whitespace().next().unwrap_or("");
        if !val.is_empty() { color = Some(val.to_string()); }
        rest = rest[..idx].trim().to_string();
    }

    (rest, color, send)
}

/// Find the byte position of `key=` in `s`, only at a word boundary.
fn find_kv(s: &str, key: &str) -> Option<usize> {
    let needle = format!("{key}=");
    if s.starts_with(&needle) { return Some(0); }
    s.rfind(&format!(" {needle}")).map(|p| p + 1)
}

fn build_map_lines(map: &WorldMap, width: u16, height: u16, pan: (i32, i32, i32)) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let Some(cur) = map.current_room() else {
        return vec![Line::from("No map data yet")];
    };

    let half_w = (width.saturating_sub(1) / 2) as i32;
    let half_h = (height.saturating_sub(1) / 2) as i32;
    let z = cur.z + pan.2;

    let mut rows: Vec<Vec<char>> = vec![vec![' '; width as usize]; height as usize];
    for sy in 0..height as i32 {
        for sx in 0..width as i32 {
            let wx = cur.x + pan.0 + (sx - half_w);
            let wy = cur.y + pan.1 + (sy - half_h);
            if let Some(room) = map.room_at(wx, wy, z) {
                rows[sy as usize][sx as usize] = if room.id == cur.id && z == cur.z { '@' } else { '.' };
            }
        }
    }

    rows
        .into_iter()
        .map(|r| Line::raw(r.into_iter().collect::<String>()))
        .collect()
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub fn draw(frame: &mut Frame, state: &mut GameState, server_name: &str, char_name: &str) {
    let area = frame.area();

    // Top-level vertical split: [content area | status bar (1 row)]
    let [content_area, status_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // Horizontal split: game column | optional right sidebar.
    let has_right  = state.sidebar.has_side_panels(&SidebarSide::Right);
    let show_right = has_right && state.sidebar.layout.right_visible;

    const MIN_GAME_W: u16 = 20;
    let right_w = if show_right {
        state.sidebar.layout.right_width
            .min(content_area.width.saturating_sub(MIN_GAME_W))
    } else { 0 };

    let (game_col, right_area_opt) = if right_w > 0 {
        let c = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(right_w),
        ]).split(content_area);
        (c[0], Some(c[1]))
    } else {
        (content_area, None)
    };

    // Game column: [output area | input block (3 rows: border + content + border)]
    let [output_area, input_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(game_col);

    // The bordered output block and input block each consume 2 rows for their borders.
    let output_inner_h = output_area.height.saturating_sub(2) as usize;
    state.last_output_height = output_inner_h.max(1);

    // --- Compute visible line window ---
    let buf_len = state.lines.len();
    // Clamp scroll so we never go past the top.
    let scroll = state.scroll_offset.min(buf_len.saturating_sub(1));

    // Suppress the live prompt while in copy mode so the highlighted line is always scrollback.
    let has_prompt = state.prompt.is_some() && scroll == 0 && !state.copy_mode;
    // Reserve one row for the prompt when it's visible.
    let rows_for_lines =
        output_inner_h.saturating_sub(if has_prompt { 1 } else { 0 });

    let line_end = buf_len.saturating_sub(scroll);
    let line_start = line_end.saturating_sub(rows_for_lines);

    let mut text_lines: Vec<Line<'static>> = state
        .lines
        .iter()
        .skip(line_start)
        .take(line_end - line_start)
        .cloned()
        .collect();

    if has_prompt {
        if let Some(p) = &state.prompt {
            text_lines.push(p.clone());
        }
    }

    // Scroll indicator shown in the bottom border of the output block.
    let scroll_hint = if scroll > 0 {
        format!(
            " ↑{scroll}/{buf_len} lines — PgDn / Ctrl+End to return "
        )
    } else {
        String::new()
    };

    // Highlight the bottom-most visible line in copy mode.
    if state.copy_mode && !text_lines.is_empty() {
        let last = text_lines.len() - 1;
        if let Some(line) = text_lines.get_mut(last) {
            *line = Line::from(
                line.spans.iter()
                    .map(|sp| Span::styled(
                        sp.content.clone(),
                        sp.style.bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD),
                    ))
                    .collect::<Vec<_>>(),
            );
        }
    }

    let output_block = if state.copy_mode {
        Block::bordered()
            .title(format!(" {} ", server_name))
            .title_bottom(
                Line::from(Span::styled(
                    " COPY MODE  \u{2191}\u{2193} scroll  y yank  Esc exit ",
                    Style::default().fg(Color::Black).bg(Color::Yellow),
                ))
            )
            .border_style(Style::default().fg(Color::Yellow))
    } else {
        Block::bordered()
            .title(format!(" {} ", server_name))
            .title_bottom(scroll_hint.as_str())
    };

    let output_para = Paragraph::new(Text::from(text_lines))
        .block(output_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(output_para, output_area);

    // --- Input line (bordered) ---
    let before_cursor = &state.input[..state.input_cursor];
    let (cursor_ch, after_cursor) = if state.input_cursor < state.input.len() {
        let c = state.input[state.input_cursor..].chars().next().unwrap();
        let end = state.input_cursor + c.len_utf8();
        (c.to_string(), state.input[end..].to_string())
    } else {
        (" ".to_string(), String::new())
    };
    let input_line = Line::from(vec![
        Span::styled("▶ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw(before_cursor.to_string()),
        Span::styled(
            cursor_ch,
            Style::default().bg(Color::White).fg(Color::Black),
        ),
        Span::raw(after_cursor),
    ]);
    frame.render_widget(
        Paragraph::new(input_line).block(Block::bordered()),
        input_area,
    );

    // --- Sidebars ---
    sidebar::draw(frame, &state.sidebar, area, None, right_area_opt);

    if state.map_fullscreen {
        frame.render_widget(Clear, content_area);
        let block = Block::bordered()
            .title(" Map ")
            .title_bottom(" Arrows pan  u/d z-level  c center  Esc/F6 close ")
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(content_area);
        let lines = build_map_lines(&state.sidebar.automap, inner.width, inner.height, state.map_pan);
        frame.render_widget(block, content_area);
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().fg(Color::White)),
            inner,
        );
    }

    // --- Status bar ---
    // Use REVERSED on the paragraph base so the bar adapts to dark/light themes.
    // Latency values are rendered as colored badges (explicit bg + black fg +
    // REVERSED removed) so their color is always visible in any theme.
    let base_style = Style::default().add_modifier(Modifier::REVERSED);
    let conn_icon = if state.connected {
        Span::styled("● ", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ ", Style::default().fg(Color::Red))
    };
    let disc_hint = if !state.connected { "  disconnected" } else { "" };
    let latency_badge_color = |value: Option<u64>| match value {
        Some(ms) if ms <= 120 => Color::Rgb(60, 180, 60),
        Some(ms) if ms <= 250 => Color::Rgb(200, 160, 0),
        Some(_) => Color::Rgb(200, 50, 50),
        None => Color::Reset,
    };
    // Badge style: colored bg, black text, no REVERSED so colors are exact.
    let badge = |color: Color, text: String, bold: bool| -> Span<'static> {
        let mut style = Style::default()
            .fg(Color::Black)
            .bg(color)
            .remove_modifier(Modifier::REVERSED);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        Span::styled(text, style)
    };
    let current_badge_color = latency_badge_color(state.latency);
    //let avg_badge_color     = latency_badge_color(state.latency_avg());
    let latency_spans: Vec<Span<'static>> = match (state.latency, state.latency_avg()) {
        (Some(current), Some(avg)) => vec![
            Span::raw("  "),
            badge(current_badge_color, format!(" {current}ms "), true),
            //Span::raw(" (ø"),
            //badge(avg_badge_color, format!(" {avg}ms "), false),
            //Span::raw(")"),
        ],
        (Some(current), None) => vec![
            Span::raw("  "),
            badge(current_badge_color, format!(" {current}ms "), true),
        ],
        _ => vec![Span::raw("  --")],
    };
    let mut status_spans: Vec<Span<'static>> = vec![
        conn_icon,
        Span::raw(format!(" {server_name}  /  {char_name}")),
    ];
    status_spans.extend(latency_spans);
    status_spans.push(Span::raw(format!(
        "{disc_hint}   \u{2191}\u{2193} hist   PgUp/Dn scroll   Ctrl+Y copy   F6 map   Ctrl+Q disc   F2/F3:sidebars  F4:panel"
    )));
    let status_line = Line::from(status_spans);
    frame.render_widget(
        Paragraph::new(status_line).style(base_style),
        status_area,
    );
}
