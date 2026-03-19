//! Sidebar panel system — character sheet, paperdoll, inventory, automap placeholder.
//!
//! # F-key layout
//!   F1      → focus back to the game input  
//!   F2–F5   → focus a sidebar panel (configured per-character)
//!
//! # Options overlay
//!   Press `o` while a panel is focused to open it.
//!   ↑↓ to navigate, ← → to reorder panels or adjust width, Esc to save and close.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use regex::Regex;

use crate::config::{PanelConfig, PanelKind, SidebarLayout, SidebarSide};

// ---------------------------------------------------------------------------
// Key result
// ---------------------------------------------------------------------------
/// Result returned by [`handle_sidebar_key`] to the game key handler.
pub enum SidebarKeyResult {
    /// Key was fully handled; caller needs no further action.
    Consumed,
    /// Key was not recognised; caller may re-use it.
    Unhandled,
    /// Sidebar requests that focus return to the game input.
    FocusGame,
    /// A layout property changed; caller should persist to the character config.
    SaveLayout,
    /// Send a line to the connected server (panel-specific shortcut).
    SendLine(String),
}

// ---------------------------------------------------------------------------
// Output-capture system
// ---------------------------------------------------------------------------

/// How extracted data is parsed from a capture line.
#[derive(Clone, Copy)]
enum CaptureParser {
    /// `captures_iter` on the line: group 1 = key, group 2 = value.
    KeyValue,
    /// `captures_iter` on the line: group 1 = item text.
    ListItem,
}

/// A built-in rule that watches for a command sent to the server and then
/// harvests the response into a sidebar panel.
struct CaptureRule {
    panel: PanelKind,
    /// Regex matched against the trimmed+lowercased sent command.
    command_pattern: &'static str,
    /// Regex matched against incoming lines; capture begins when it fires.
    begin_pattern: &'static str,
    /// When `Some`, capture ends when a non-blank content line matches this.
    end_pattern: Option<&'static str>,
    /// Regex applied per content line to extract data.
    line_pattern: &'static str,
    parser: CaptureParser,
    /// End capture after this many *consecutive* blank lines.
    max_blank_lines: u8,
    /// Hard stop after this many total lines (safety valve).
    max_total_lines: usize,
}

/// Runtime state of an in-progress capture.
struct ActiveCapture {
    panel: PanelKind,
    /// `false` while still waiting for the `begin_re` match.
    begun: bool,
    begin_re: Regex,
    end_re: Option<Regex>,
    line_re: Regex,
    parser: CaptureParser,
    max_blank_lines: u8,
    max_total_lines: usize,
    consecutive_blanks: u8,
    total_lines: usize,
}

/// Built-in capture rules — MUME-compatible defaults.
fn default_capture_rules() -> Vec<CaptureRule> {
    vec![
        // inventory / inv / i  →  Inventory panel
        CaptureRule {
            panel: PanelKind::Inventory,
            command_pattern: r"(?i)^(inventory|inv|i)$",
            begin_pattern:   r"(?i)you are carrying",
            end_pattern:     Some(r"(?i)nothing"),
            line_pattern:    r"^\s+(.+?)\s*$",
            parser:          CaptureParser::ListItem,
            max_blank_lines: 1,
            max_total_lines: 200,
        },
        // eq / equipment  →  Paperdoll panel
        CaptureRule {
            panel: PanelKind::Paperdoll,
            command_pattern: r"(?i)^(eq|equipment)$",
            begin_pattern:   r"(?i)you are (using|wearing)",
            end_pattern:     Some(r"(?i)nothing"),
            line_pattern:    r"^<([^>]+?)>\s+(.+?)\s*$",
            parser:          CaptureParser::KeyValue,
            max_blank_lines: 1,
            max_total_lines: 50,
        },
        // info / score / sc  →  Character sheet panel
        CaptureRule {
            panel: PanelKind::CharSheet,
            command_pattern: r"(?i)^(info|score|sc)$",
            begin_pattern:   r"(?i)you are .+,? a level",
            end_pattern:     None,
            line_pattern:    r"(\w[\w ]*?):\s+(\S+)",
            parser:          CaptureParser::KeyValue,
            max_blank_lines: 2,
            max_total_lines: 60,
        },
    ]
}

/// Strip ANSI escape codes from `s` for plain-text pattern matching.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if bytes[i] != b'\x1b' {
            let ch = s[i..].chars().next().unwrap_or('\0');
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1; // skip ESC
        if i >= bytes.len() { break; }
        match bytes[i] {
            b'[' => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7Eu8).contains(&bytes[i]) { i += 1; }
                if i < bytes.len() { i += 1; }
            }
            b']' | b'P' | b'^' | b'_' | b'X' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\x07' { i += 1; break; }
                    if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i+1] == b'\\' { i += 2; break; }
                    i += 1;
                }
            }
            b'(' | b')' | b'*' | b'+' => { i += 1; if i < bytes.len() { i += 1; } }
            _ => { i += 1; }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Sidebar state
// ---------------------------------------------------------------------------

pub struct SidebarState {
    /// Layout configuration (panels order, width, visibility).
    pub layout: SidebarLayout,
    /// Which panel currently has keyboard focus.  `None` → game has focus.
    pub focused_panel: Option<PanelKind>,
    /// Whether the options overlay is open.
    pub options_open: bool,

    // --- Panel data (populated via /stat|/wear|/inv meta-commands or GMCP) ---

    /// Character sheet: ordered key → value pairs.
    pub char_sheet: Vec<(String, String)>,
    /// Paperdoll: slot → worn item pairs.
    pub paperdoll: Vec<(String, String)>,
    /// Inventory: list of item strings.
    pub inventory: Vec<String>,

    // --- Interaction state ---

    /// Cursor row within the focused panel list.
    pub panel_cursor: usize,
    /// Cursor row in the options overlay.
    pub options_cursor: usize,

    // --- Output-capture (auto-populates panels from MUD responses) ---
    capture_rules: Vec<CaptureRule>,
    active_capture: Option<ActiveCapture>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self::new(SidebarLayout::default())
    }
}

impl SidebarState {
    pub fn new(layout: SidebarLayout) -> Self {
        Self {
            layout,
            focused_panel: None,
            options_open: false,
            char_sheet: Vec::new(),
            paperdoll: Vec::new(),
            inventory: Vec::new(),
            panel_cursor: 0,
            options_cursor: 0,
            capture_rules: default_capture_rules(),
            active_capture: None,
        }
    }

    /// Returns `true` if any panel is assigned to the given sidebar side.
    pub fn has_side_panels(&self, side: &SidebarSide) -> bool {
        self.layout.panels.iter().any(|p| p.side.as_ref() == Some(side))
    }

    /// Toggles left sidebar visibility (no-op when no panels are assigned).
    /// Returns `true` if the layout actually changed.
    pub fn toggle_left(&mut self) -> bool {
        if self.has_side_panels(&SidebarSide::Left) {
            self.layout.left_visible = !self.layout.left_visible;
            if !self.layout.left_visible {
                if let Some(fp) = &self.focused_panel {
                    let on_left = self.layout.panels.iter()
                        .any(|p| p.kind == *fp && p.side == Some(SidebarSide::Left));
                    if on_left { self.focused_panel = None; }
                }
            }
            true
        } else {
            false
        }
    }

    /// Toggles right sidebar visibility (no-op when no panels are assigned).
    /// Returns `true` if the layout actually changed.
    pub fn toggle_right(&mut self) -> bool {
        if self.has_side_panels(&SidebarSide::Right) {
            self.layout.right_visible = !self.layout.right_visible;
            if !self.layout.right_visible {
                if let Some(fp) = &self.focused_panel {
                    let on_right = self.layout.panels.iter()
                        .any(|p| p.kind == *fp && p.side == Some(SidebarSide::Right));
                    if on_right { self.focused_panel = None; }
                }
            }
            true
        } else {
            false
        }
    }

    /// Cycles keyboard focus to the next visible panel (wraps; `None` → first).
    /// Resets `panel_cursor` to 0 whenever focus changes.
    pub fn focus_next_panel(&mut self) {
        let visible: Vec<PanelKind> = self.layout.panels.iter()
            .filter(|p| match &p.side {
                Some(SidebarSide::Left)  => self.layout.left_visible,
                Some(SidebarSide::Right) => self.layout.right_visible,
                None => false,
            })
            .map(|p| p.kind.clone())
            .collect();

        if visible.is_empty() {
            self.focused_panel = None;
            return;
        }

        let next = match &self.focused_panel {
            None => visible.into_iter().next(),
            Some(cur) => {
                let pos = visible.iter().position(|k| k == cur);
                match pos {
                    None    => visible.into_iter().next(),
                    Some(i) => {
                        let next_i = (i + 1) % visible.len();
                        visible.into_iter().nth(next_i)
                    }
                }
            }
        };
        self.focused_panel = next;
        self.panel_cursor  = 0;
    }

    // ------------------------------------------------------------------
    // Character sheet
    // ------------------------------------------------------------------

    pub fn set_stat(&mut self, key: String, value: String) {
        if let Some(row) = self.char_sheet.iter_mut().find(|(k, _)| k == &key) {
            row.1 = value;
        } else {
            self.char_sheet.push((key, value));
        }
    }

    pub fn remove_stat(&mut self, key: &str) {
        self.char_sheet.retain(|(k, _)| k != key);
    }

    pub fn clear_stats(&mut self) {
        self.char_sheet.clear();
    }

    // ------------------------------------------------------------------
    // Paperdoll
    // ------------------------------------------------------------------

    pub fn set_wear(&mut self, slot: String, item: String) {
        if let Some(row) = self.paperdoll.iter_mut().find(|(s, _)| s == &slot) {
            row.1 = item;
        } else {
            self.paperdoll.push((slot, item));
        }
    }

    pub fn remove_wear(&mut self, slot: &str) {
        self.paperdoll.retain(|(s, _)| s != slot);
    }

    pub fn clear_paperdoll(&mut self) {
        self.paperdoll.clear();
    }

    // ------------------------------------------------------------------
    // Inventory
    // ------------------------------------------------------------------

    pub fn inv_add(&mut self, item: String) {
        self.inventory.push(item);
    }

    pub fn inv_remove(&mut self, item: &str) {
        if let Some(pos) = self.inventory.iter().position(|i| i == item) {
            self.inventory.remove(pos);
        }
    }

    pub fn inv_clear(&mut self) {
        self.inventory.clear();
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Output-capture API
    // ------------------------------------------------------------------

    /// Call when the user sends a command to the MUD server.
    /// If the command matches a capture rule the target panel is cleared and
    /// capture of the upcoming server response is armed.
    pub fn on_command_sent(&mut self, cmd: &str) {
        let lower = cmd.trim().to_lowercase();

        // Find a matching rule and extract all data we need before mutating self.
        struct Matched {
            panel: PanelKind,
            begin_re: Regex,
            end_re: Option<Regex>,
            line_re: Regex,
            parser: CaptureParser,
            max_blank_lines: u8,
            max_total_lines: usize,
        }

        let matched = self.capture_rules.iter().find_map(|rule| {
            let cmd_re = Regex::new(rule.command_pattern).ok()?;
            if !cmd_re.is_match(&lower) { return None; }
            let begin_re = Regex::new(rule.begin_pattern).ok()?;
            let end_re   = rule.end_pattern.and_then(|p| Regex::new(p).ok());
            let line_re  = Regex::new(rule.line_pattern).ok()?;
            Some(Matched {
                panel: rule.panel.clone(),
                begin_re, end_re, line_re,
                parser: rule.parser,
                max_blank_lines: rule.max_blank_lines,
                max_total_lines: rule.max_total_lines,
            })
        });

        if let Some(m) = matched {
            // Clear the target panel (mutable borrow; capture_rules no longer borrowed).
            match m.panel {
                PanelKind::CharSheet => self.clear_stats(),
                PanelKind::Paperdoll => self.clear_paperdoll(),
                PanelKind::Inventory => self.inv_clear(),
                PanelKind::Automap   => {}
            }
            self.active_capture = Some(ActiveCapture {
                panel: m.panel,
                begun: false,
                begin_re: m.begin_re,
                end_re: m.end_re,
                line_re: m.line_re,
                parser: m.parser,
                max_blank_lines: m.max_blank_lines,
                max_total_lines: m.max_total_lines,
                consecutive_blanks: 0,
                total_lines: 0,
            });
        }
    }

    /// Call for every incoming server line (raw, may contain ANSI codes).
    /// When a capture is active this populates the appropriate panel.
    pub fn process_line(&mut self, raw: &str) {
        if self.active_capture.is_none() { return; }

        let text  = strip_ansi(raw);
        let clean = text.trim();

        // --- Safety: hard line-count stop ---
        {
            let cap = self.active_capture.as_mut().unwrap();
            cap.total_lines += 1;
            if cap.total_lines > cap.max_total_lines {
                self.active_capture = None;
                return;
            }
        }

        // --- Wait for begin line ---
        if !self.active_capture.as_ref().unwrap().begun {
            let matches = self.active_capture.as_ref().unwrap().begin_re.is_match(clean);
            if matches {
                self.active_capture.as_mut().unwrap().begun = true;
                // Fall through: the begin line may itself contain data.
            } else {
                return;
            }
        }

        // --- Blank-line handling ---
        if clean.is_empty() {
            let end = {
                let cap = self.active_capture.as_mut().unwrap();
                cap.consecutive_blanks += 1;
                cap.consecutive_blanks >= cap.max_blank_lines
            };
            if end { self.active_capture = None; }
            return;
        }
        self.active_capture.as_mut().unwrap().consecutive_blanks = 0;

        // --- Explicit end pattern ---
        let stop = self.active_capture.as_ref().unwrap()
            .end_re.as_ref()
            .map(|re| re.is_match(clean))
            .unwrap_or(false);
        if stop {
            self.active_capture = None;
            return;
        }

        // --- Extract data into local buffers (to avoid double-borrow) ---
        let (panel, parser) = {
            let cap = self.active_capture.as_ref().unwrap();
            (cap.panel.clone(), cap.parser)
        };

        let mut kv: Vec<(String, String)> = Vec::new();
        let mut items: Vec<String> = Vec::new();
        {
            let cap = self.active_capture.as_ref().unwrap();
            match parser {
                CaptureParser::KeyValue => {
                    for m in cap.line_re.captures_iter(&text) {
                        if let (Some(k), Some(v)) = (m.get(1), m.get(2)) {
                            let k = k.as_str().trim().to_string();
                            let v = v.as_str().trim().to_string();
                            if !k.is_empty() && !v.is_empty() {
                                kv.push((k, v));
                            }
                        }
                    }
                }
                CaptureParser::ListItem => {
                    for m in cap.line_re.captures_iter(&text) {
                        if let Some(item) = m.get(1) {
                            let s = item.as_str().trim().to_string();
                            if !s.is_empty() { items.push(s); }
                        }
                    }
                }
            }
        }

        // Apply collected data.
        for (k, v) in kv {
            match &panel {
                PanelKind::CharSheet => self.set_stat(k, v),
                PanelKind::Paperdoll => self.set_wear(k, v),
                _ => {}
            }
        }
        for item in items {
            if matches!(panel, PanelKind::Inventory) {
                self.inv_add(item);
            }
        }
    }

    fn active_panel_len(&self) -> usize {
        match &self.focused_panel {
            Some(PanelKind::CharSheet) => self.char_sheet.len(),
            Some(PanelKind::Paperdoll) => self.paperdoll.len(),
            Some(PanelKind::Inventory) => self.inventory.len(),
            Some(PanelKind::Automap)   => 0,
            None                       => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

/// Process a key event while a sidebar panel has focus.
pub fn handle_sidebar_key(state: &mut SidebarState, key: KeyEvent) -> SidebarKeyResult {
    if state.options_open {
        return handle_options_key(state, key);
    }

    let panel_len = state.active_panel_len();

    match key.code {
        // Return focus to game.
        KeyCode::Esc | KeyCode::F(1) => SidebarKeyResult::FocusGame,

        // Cycle focus to the next visible panel.
        KeyCode::Tab => {
            state.focus_next_panel();
            SidebarKeyResult::Consumed
        }

        // Open options overlay.
        KeyCode::Char('o') => {
            state.options_open   = true;
            state.options_cursor = 0;
            SidebarKeyResult::Consumed
        }

        // Scroll within the focused panel list.
        KeyCode::Up => {
            if state.panel_cursor > 0 {
                state.panel_cursor -= 1;
            }
            SidebarKeyResult::Consumed
        }
        KeyCode::Down => {
            if panel_len > 0 && state.panel_cursor + 1 < panel_len {
                state.panel_cursor += 1;
            }
            SidebarKeyResult::Consumed
        }

        // Paperdoll: Remove/unequip selected item (Enter or ‘r’).
        KeyCode::Enter | KeyCode::Char('r')
            if matches!(state.focused_panel, Some(PanelKind::Paperdoll)) =>
        {
            if let Some((slot, _)) = state.paperdoll.get(state.panel_cursor) {
                return SidebarKeyResult::SendLine(format!("remove {slot}"));
            }
            SidebarKeyResult::Consumed
        }

        // Inventory: Wear/equip selected item (‘w’ or Enter).
        KeyCode::Enter | KeyCode::Char('w')
            if matches!(state.focused_panel, Some(PanelKind::Inventory)) =>
        {
            if let Some(item) = state.inventory.get(state.panel_cursor) {
                return SidebarKeyResult::SendLine(format!("wear {item}"));
            }
            SidebarKeyResult::Consumed
        }

        // Inventory: Drop selected item (‘d’).
        KeyCode::Char('d')
            if matches!(state.focused_panel, Some(PanelKind::Inventory)) =>
        {
            if let Some(item) = state.inventory.get(state.panel_cursor) {
                return SidebarKeyResult::SendLine(format!("drop {item}"));
            }
            SidebarKeyResult::Consumed
        }

        _ => SidebarKeyResult::Unhandled,
    }
}

fn handle_options_key(state: &mut SidebarState, key: KeyEvent) -> SidebarKeyResult {
    let n_panels = state.layout.panels.len();
    // Rows: 0..n_panels = panels, n_panels = left width, n_panels+1 = right width, n_panels+2 = close
    let n_rows = n_panels + 3;

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.options_open = false;
            SidebarKeyResult::SaveLayout
        }
        KeyCode::Up => {
            if state.options_cursor > 0 { state.options_cursor -= 1; }
            SidebarKeyResult::Consumed
        }
        KeyCode::Down => {
            if state.options_cursor + 1 < n_rows { state.options_cursor += 1; }
            SidebarKeyResult::Consumed
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if state.options_cursor == n_panels + 2 {
                state.options_open = false;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }
        // → on panel row: cycle side assignment (None → Left → Right → None).
        // → on width rows: increase width.
        KeyCode::Right => {
            let i = state.options_cursor;
            if i < n_panels {
                let p = &mut state.layout.panels[i];
                p.side = match &p.side {
                    None                     => Some(SidebarSide::Left),
                    Some(SidebarSide::Left)  => Some(SidebarSide::Right),
                    Some(SidebarSide::Right) => None,
                };
                SidebarKeyResult::SaveLayout
            } else if i == n_panels && state.layout.left_width < 60 {
                state.layout.left_width += 1;
                SidebarKeyResult::SaveLayout
            } else if i == n_panels + 1 && state.layout.right_width < 60 {
                state.layout.right_width += 1;
                SidebarKeyResult::SaveLayout
            } else {
                SidebarKeyResult::Consumed
            }
        }
        // ← on panel row: cycle side assignment backwards.
        // ← on width rows: decrease width.
        KeyCode::Left => {
            let i = state.options_cursor;
            if i < n_panels {
                let p = &mut state.layout.panels[i];
                p.side = match &p.side {
                    None                     => Some(SidebarSide::Right),
                    Some(SidebarSide::Right) => Some(SidebarSide::Left),
                    Some(SidebarSide::Left)  => None,
                };
                SidebarKeyResult::SaveLayout
            } else if i == n_panels && state.layout.left_width > 12 {
                state.layout.left_width -= 1;
                SidebarKeyResult::SaveLayout
            } else if i == n_panels + 1 && state.layout.right_width > 12 {
                state.layout.right_width -= 1;
                SidebarKeyResult::SaveLayout
            } else {
                SidebarKeyResult::Consumed
            }
        }
        // +/= on panel row: increase height share.
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if state.options_cursor < n_panels {
                let h = &mut state.layout.panels[state.options_cursor].height_pct;
                *h = (*h).saturating_add(5).min(100);
                SidebarKeyResult::SaveLayout
            } else {
                SidebarKeyResult::Consumed
            }
        }
        // - on panel row: decrease height share.
        KeyCode::Char('-') => {
            if state.options_cursor < n_panels {
                let h = &mut state.layout.panels[state.options_cursor].height_pct;
                *h = (*h).saturating_sub(5).max(5);
                SidebarKeyResult::SaveLayout
            } else {
                SidebarKeyResult::Consumed
            }
        }
        _ => SidebarKeyResult::Unhandled,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Render both sidebar columns and the options modal (if open).
/// `term_area` is the full terminal rectangle, used to center the options modal.
pub fn draw(
    frame: &mut Frame,
    state: &SidebarState,
    term_area: Rect,
    left_area: Option<Rect>,
    right_area: Option<Rect>,
) {
    if let Some(area) = left_area {
        draw_sidebar_col(frame, state, area, &SidebarSide::Left);
    }
    if let Some(area) = right_area {
        draw_sidebar_col(frame, state, area, &SidebarSide::Right);
    }
    if state.options_open {
        draw_options_modal(frame, state, term_area);
    }
}

/// Render all panels assigned to `side` stacked vertically inside `area`.
/// Heights are allocated proportionally to each panel's `height_pct`.
fn draw_sidebar_col(frame: &mut Frame, state: &SidebarState, area: Rect, side: &SidebarSide) {
    let panels: Vec<&PanelConfig> = state.layout.panels.iter()
        .filter(|p| p.side.as_ref() == Some(side))
        .collect();

    if panels.is_empty() || area.height < 2 {
        return;
    }

    let total_pct: u32 = panels.iter().map(|p| p.height_pct as u32).sum::<u32>().max(1);
    let avail_h   = area.height;
    let mut y     = area.y;

    for (i, pc) in panels.iter().enumerate() {
        let is_last  = i == panels.len() - 1;
        let panel_h: u16 = if is_last {
            (area.y + avail_h).saturating_sub(y)
        } else {
            ((avail_h as u32 * pc.height_pct as u32) / total_pct) as u16
        };
        if panel_h == 0 { continue; }
        if y + panel_h > area.y + avail_h { break; }

        let panel_area = Rect { x: area.x, y, width: area.width, height: panel_h };
        y += panel_h;

        let focused = state.focused_panel.as_ref() == Some(&pc.kind);
        draw_panel(frame, state, &pc.kind, panel_area, focused);
    }
}

fn draw_panel(frame: &mut Frame, state: &SidebarState, kind: &PanelKind, area: Rect, focused: bool) {
    match kind {
        PanelKind::CharSheet => draw_kv_panel(
            frame, "Character Sheet",
            &state.char_sheet.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
            state.panel_cursor, area, focused, Some("r:remove"),
        ),
        PanelKind::Paperdoll => draw_kv_panel(
            frame, "Paperdoll",
            &state.paperdoll.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
            state.panel_cursor, area, focused, Some("Enter/r:remove"),
        ),
        PanelKind::Inventory => draw_list_panel(
            frame, "Inventory",
            &state.inventory.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            state.panel_cursor, area, focused, Some("Enter/w:wear  d:drop"),
        ),
        PanelKind::Automap => draw_placeholder(frame, "Automap\n\n(Phase 8)", area),
    }
}

fn draw_kv_panel(
    frame: &mut Frame,
    title: &str,
    rows: &[(&str, &str)],
    cursor: usize,
    area: Rect,
    focused: bool,
    key_hint: Option<&str>,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    if focused {
        if let Some(hint) = key_hint {
            block = block.title_bottom(Span::styled(
                format!(" {hint} "),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("  (empty)", Style::default().fg(Color::DarkGray)))
                .block(block),
            area,
        );
        return;
    }

    let key_w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0).min(14);
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, (k, v))| {
            let base = if i == cursor && focused {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{k:<key_w$}"), base.add_modifier(Modifier::BOLD)),
                Span::styled(" ", base),
                Span::styled(v.to_string(), base),
            ]))
        })
        .collect();

    let mut list_state = ListState::default();
    if focused && !rows.is_empty() {
        list_state.select(Some(cursor.min(rows.len().saturating_sub(1))));
    }
    frame.render_stateful_widget(List::new(items).block(block), area, &mut list_state);
}

fn draw_list_panel(
    frame: &mut Frame,
    title: &str,
    items_raw: &[&str],
    cursor: usize,
    area: Rect,
    focused: bool,
    key_hint: Option<&str>,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    if focused {
        if let Some(hint) = key_hint {
            block = block.title_bottom(Span::styled(
                format!(" {hint} "),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    if items_raw.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("  (empty)", Style::default().fg(Color::DarkGray)))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = items_raw
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == cursor && focused {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            ListItem::new(Span::styled(item.to_string(), style))
        })
        .collect();

    let mut list_state = ListState::default();
    if focused && !items_raw.is_empty() {
        list_state.select(Some(cursor.min(items_raw.len().saturating_sub(1))));
    }
    frame.render_stateful_widget(List::new(items).block(block), area, &mut list_state);
}

fn draw_placeholder(frame: &mut Frame, text: &str, area: Rect) {
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_options_modal(frame: &mut Frame, state: &SidebarState, parent: Rect) {
    let n_panels = state.layout.panels.len();
    // rows: n_panels + separator + left_w + right_w + close + 2 borders
    let modal_h: u16 = (n_panels as u16 + 6).min(parent.height);
    let modal_w: u16 = parent.width.min(46).max(28);
    let x = parent.x + parent.width.saturating_sub(modal_w) / 2;
    let y = parent.y + parent.height.saturating_sub(modal_h) / 2;
    let modal_area = Rect { x, y, width: modal_w, height: modal_h };

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(Span::styled(
            " Sidebar Options ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let mut lines: Vec<Line> = Vec::new();

    // --- Panel rows ---
    for (i, pc) in state.layout.panels.iter().enumerate() {
        let selected  = state.options_cursor == i;
        let row_style = if selected {
            Style::default().bg(Color::White).fg(Color::Black)
        } else {
            Style::default()
        };
        let side_str = match &pc.side {
            None                     => " -- ",
            Some(SidebarSide::Left)  => "Left",
            Some(SidebarSide::Right) => " Rt ",
        };
        let h_str = if pc.side.is_some() {
            format!("{:3}%", pc.height_pct)
        } else {
            "    ".to_string()
        };
        let hint = if selected { "  \u{2190}\u{2192}:side  +/-:h%" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(format!("{:<8} ", pc.kind.short_label()), row_style.add_modifier(Modifier::BOLD)),
            Span::styled(format!("[{side_str}]"), row_style),
            Span::styled(format!(" {h_str}"), row_style),
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Separator
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(inner.width as usize),
        Style::default().fg(Color::DarkGray),
    )));

    // Left width row
    let lw_sel   = state.options_cursor == n_panels;
    let lw_style = if lw_sel { Style::default().bg(Color::White).fg(Color::Black) } else { Style::default() };
    lines.push(Line::from(vec![
        Span::styled("Left  w ", Style::default().fg(Color::Cyan)),
        Span::styled(format!("{:>3}", state.layout.left_width), lw_style),
        Span::styled(if lw_sel { "  \u{2190}\u{2192} adjust" } else { "" }, Style::default().fg(Color::DarkGray)),
    ]));

    // Right width row
    let rw_sel   = state.options_cursor == n_panels + 1;
    let rw_style = if rw_sel { Style::default().bg(Color::White).fg(Color::Black) } else { Style::default() };
    lines.push(Line::from(vec![
        Span::styled("Right w ", Style::default().fg(Color::Cyan)),
        Span::styled(format!("{:>3}", state.layout.right_width), rw_style),
        Span::styled(if rw_sel { "  \u{2190}\u{2192} adjust" } else { "" }, Style::default().fg(Color::DarkGray)),
    ]));

    // Close row
    let c_sel   = state.options_cursor == n_panels + 2;
    let c_style = if c_sel {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(Span::styled(" [Close]  Esc to save & close", c_style)));

    frame.render_widget(Paragraph::new(lines), inner);
}
