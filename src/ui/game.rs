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
    widgets::{Block, Paragraph, Wrap},
    Frame,
};
use tracing::warn;

use crate::config::{Alias, Trigger as TriggerCfg};
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
            connected: false,
            prompt: None,
            last_output_height: 20,
            aliases: Vec::new(),
            triggers: Vec::new(),
            auto_send_queue: Vec::new(),
            copy_mode: false,
            sidebar: SidebarState::default(),
        }
    }

    /// Parse an ANSI string, apply trigger highlighting, and append to the scrollback buffer.
    /// Any trigger auto-sends are pushed to `auto_send_queue`.
    pub fn push_line(&mut self, s: &str) {
        // Auto-populate sidebar panels from server output.
        self.sidebar.process_line(s);
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
        // Prime sidebar capture for this command's upcoming output.
        self.sidebar.on_command_sent(&line);

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
        self.scroll_to_bottom();
        self.prompt = None;
    }

    pub fn on_disconnect(&mut self) {
        self.connected = false;
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

            // ------ Character sheet (/stat) ----------------------------------
            "stat" => {
                if args.is_empty() {
                    if self.sidebar.char_sheet.is_empty() {
                        self.push_system("Char sheet empty.  Usage: /stat <key> <value>");
                    } else {
                        self.push_system("Char sheet:");
                        let msgs: Vec<String> = self.sidebar.char_sheet.iter()
                            .map(|(k, v)| format!("  {k}: {v}"))
                            .collect();
                        for m in msgs { self.push_system(&m); }
                    }
                    None
                } else if args == "clear" {
                    self.sidebar.clear_stats();
                    self.push_system("Char sheet cleared.");
                    None
                } else {
                    match args.split_once(' ') {
                        None => {
                            // /stat <key> — show single stat
                            if let Some((_, v)) = self.sidebar.char_sheet.iter().find(|(k, _)| k == args) {
                                self.push_system(&format!("{args}: {v}"));
                            } else {
                                self.push_system(&format!("Stat '{}' not found.", args));
                            }
                            None
                        }
                        Some((key, value)) => {
                            self.sidebar.set_stat(key.to_string(), value.to_string());
                            self.push_system(&format!("Stat set: {key} = {value}"));
                            None
                        }
                    }
                }
            }

            // ------ Paperdoll (/wear) ----------------------------------------
            "wear" => {
                if args.is_empty() {
                    if self.sidebar.paperdoll.is_empty() {
                        self.push_system("Paperdoll empty.  Usage: /wear <slot> <item> | /wear remove <slot>");
                    } else {
                        self.push_system("Paperdoll:");
                        let msgs: Vec<String> = self.sidebar.paperdoll.iter()
                            .map(|(s, i)| format!("  {s}: {i}"))
                            .collect();
                        for m in msgs { self.push_system(&m); }
                    }
                    None
                } else if let Some(slot) = args.strip_prefix("remove ") {
                    self.sidebar.remove_wear(slot.trim());
                    self.push_system(&format!("Removed item from slot '{}'.", slot.trim()));
                    None
                } else if args == "clear" {
                    self.sidebar.clear_paperdoll();
                    self.push_system("Paperdoll cleared.");
                    None
                } else {
                    match args.split_once(' ') {
                        None => {
                            self.push_system("Usage: /wear <slot> <item> | /wear remove <slot> | /wear clear");
                            None
                        }
                        Some((slot, item)) => {
                            self.sidebar.set_wear(slot.to_string(), item.to_string());
                            self.push_system(&format!("Wearing '{}' in slot '{}'.", item, slot));
                            None
                        }
                    }
                }
            }

            // ------ Inventory (/inv) -----------------------------------------
            "inv" | "inventory" => {
                if args.is_empty() {
                    if self.sidebar.inventory.is_empty() {
                        self.push_system("Inventory empty.  Usage: /inv add <item> | /inv remove <item> | /inv clear");
                    } else {
                        self.push_system("Inventory:");
                        let msgs: Vec<String> = self.sidebar.inventory.iter()
                            .enumerate()
                            .map(|(i, item)| format!("  {}: {}", i + 1, item))
                            .collect();
                        for m in msgs { self.push_system(&m); }
                    }
                    None
                } else if let Some(item) = args.strip_prefix("add ") {
                    self.sidebar.inv_add(item.trim().to_string());
                    self.push_system(&format!("Added '{}' to inventory.", item.trim()));
                    None
                } else if let Some(item) = args.strip_prefix("remove ") {
                    self.sidebar.inv_remove(item.trim());
                    self.push_system(&format!("Removed '{}' from inventory.", item.trim()));
                    None
                } else if args == "clear" {
                    self.sidebar.inv_clear();
                    self.push_system("Inventory cleared.");
                    None
                } else {
                    self.push_system("Usage: /inv [add <item> | remove <item> | clear]");
                    None
                }
            }

            // ------ Sidebar visibility (/sidebar) ----------------------------
            "sidebar" | "sb" => {
                match args {
                    "show"  => { self.sidebar.layout.visible = true; }
                    "hide"  => { self.sidebar.layout.visible = false; }
                    _ => { self.sidebar.layout.visible = !self.sidebar.layout.visible; }
                }
                let state = if self.sidebar.layout.visible { "shown" } else { "hidden" };
                self.push_system(&format!("Sidebar {state}."));
                Some(GameAction::SaveSidebarLayout)
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
                self.push_system("  Available: /alias, /unalias, /trigger, /stat, /wear, /inv, /sidebar, /disconnect, /quit");
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
            // Parameter/intermediate bytes (0x20–0x3F) precede it; scanning for
            // the first byte ≥ 0x40 gives us the correct split even for sequences
            // with intermediate bytes like `\x1b[>4l` (params ">4", final "l").
            let seq_start = i + 2;
            let seq_end = bytes[seq_start..]
                .iter()
                .position(|&b| (0x40..=0x7E).contains(&b))
                .map(|j| seq_start + j)
                .unwrap_or(input.len());

            if seq_end < input.len() {
                let terminator = bytes[seq_end];
                let params = &input[seq_start..seq_end];
                if terminator == b'm' {
                    style = apply_sgr(style, params);
                }
                // All other escape sequences (cursor movement, mode switches, etc.)
                // are silently skipped.
                i = seq_end + 1;
            } else {
                // Unterminated sequence — skip to end of this chunk.
                i = input.len();
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
                if matches!(next, b'(' | b')' | b'*' | b'+') && i < bytes.len() {
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

    // F1 — always returns focus to the game input.
    if key.code == KeyCode::F(1) {
        state.sidebar.active_panel = None;
        return None;
    }

    // F2–F5 — select and focus a sidebar panel.
    if let KeyCode::F(n) = key.code {
        if (2..=5).contains(&n) {
            let idx = (n - 2) as usize;
            if let Some(panel) = state.sidebar.panel_for_fkey(idx).cloned() {
                if state.sidebar.active_panel.as_ref() == Some(&panel) {
                    // Pressing the same key again removes focus.
                    state.sidebar.active_panel = None;
                } else {
                    state.sidebar.layout.visible = true;
                    state.sidebar.active_panel = Some(panel);
                    state.sidebar.panel_cursor = 0;
                }
            }
            return None;
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
    if state.sidebar.active_panel.is_some() {
        return match sidebar::handle_sidebar_key(&mut state.sidebar, key) {
            SidebarKeyResult::FocusGame  => { state.sidebar.active_panel = None; None }
            SidebarKeyResult::SaveLayout => Some(GameAction::SaveSidebarLayout),
            SidebarKeyResult::SendLine(cmd) => Some(GameAction::SendLine(cmd)),
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

    // Horizontal split: game column + optional sidebar column.
    let sidebar_w = if state.sidebar.is_visible() {
        state.sidebar.width().min(content_area.width.saturating_sub(20))
    } else {
        0
    };
    let (game_col, sidebar_col_opt) = if sidebar_w > 0 {
        let chunks = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(sidebar_w),
        ])
        .split(content_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (content_area, None)
    };

    // Game column: [output area | input line (1 row)]
    let [output_area, input_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(game_col);

    // The bordered output block consumes 2 rows for its borders.
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

    // --- Input line ---
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
    frame.render_widget(Paragraph::new(input_line), input_area);

    // --- Sidebar ---
    if let Some(sa) = sidebar_col_opt {
        sidebar::draw(frame, &state.sidebar, sa);
    }

    // --- Status bar ---
    let conn_icon = if state.connected {
        Span::styled("● ", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ disconnected  ", Style::default().fg(Color::Red))
    };
    let lat_str = match state.latency {
        Some(ms) => format!("  lat {ms}ms"),
        None => String::new(),
    };
    let info = format!(
        "{server_name}  /  {char_name}{lat_str}   \
         \u{2191}\u{2193} hist   PgUp/Dn scroll   Ctrl+Y copy   Ctrl+Q disc   F1-F5 panels"
    );
    let status_line = Line::from(vec![conn_icon, Span::raw(info)]);
    frame.render_widget(
        Paragraph::new(status_line)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        status_area,
    );
}
