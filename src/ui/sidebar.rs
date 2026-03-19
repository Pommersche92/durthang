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

use crate::config::{PanelKind, SidebarLayout};

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
    /// Which panel is currently focused.  `None` → game has focus.
    pub active_panel: Option<PanelKind>,
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
            active_panel: None,
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

    /// Whether the sidebar column should be rendered.
    pub fn is_visible(&self) -> bool {
        self.layout.visible
    }

    /// Preferred width of the sidebar column (unclamped).
    pub fn width(&self) -> u16 {
        self.layout.width
    }

    /// Return the panel bound to the given F-key index (0-based: F2 → 0, F3 → 1, …).
    pub fn panel_for_fkey(&self, idx: usize) -> Option<&PanelKind> {
        self.layout.panels.get(idx)
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
        match &self.active_panel {
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

        // Open options overlay.
        KeyCode::Char('o') => {
            state.options_open  = true;
            state.options_cursor = 0;
            SidebarKeyResult::Consumed
        }

        // Scroll within the panel list.
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

        // Paperdoll: Remove/unequip selected item (Enter or 'r').
        KeyCode::Enter | KeyCode::Char('r')
            if matches!(state.active_panel, Some(PanelKind::Paperdoll)) =>
        {
            if let Some((slot, _)) = state.paperdoll.get(state.panel_cursor) {
                return SidebarKeyResult::SendLine(format!("remove {slot}"));
            }
            SidebarKeyResult::Consumed
        }

        // Inventory: Wear/equip selected item ('w' or Enter).
        KeyCode::Enter | KeyCode::Char('w')
            if matches!(state.active_panel, Some(PanelKind::Inventory)) =>
        {
            if let Some(item) = state.inventory.get(state.panel_cursor) {
                return SidebarKeyResult::SendLine(format!("wear {item}"));
            }
            SidebarKeyResult::Consumed
        }

        // Inventory: Drop selected item ('d').
        KeyCode::Char('d')
            if matches!(state.active_panel, Some(PanelKind::Inventory)) =>
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
    let panel_count = state.layout.panels.len();
    // Rows: 0..panel_count = panel entries, panel_count = width row, panel_count+1 = close
    let n_items = panel_count + 2;

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.options_open = false;
            SidebarKeyResult::SaveLayout
        }
        KeyCode::Up => {
            if state.options_cursor > 0 {
                state.options_cursor -= 1;
            }
            SidebarKeyResult::Consumed
        }
        KeyCode::Down => {
            if state.options_cursor + 1 < n_items {
                state.options_cursor += 1;
            }
            SidebarKeyResult::Consumed
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let close_row = panel_count + 1;
            if state.options_cursor == close_row {
                state.options_open = false;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }
        // Right / '+' — reorder panel (higher F-key) OR increase width.
        KeyCode::Right | KeyCode::Char('+') => {
            let i = state.options_cursor;
            if i < panel_count {
                if i + 1 < panel_count {
                    state.layout.panels.swap(i, i + 1);
                    state.options_cursor += 1;
                    return SidebarKeyResult::SaveLayout;
                }
            } else if i == panel_count && state.layout.width < 60 {
                state.layout.width += 1;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }
        // Left / '-' — reorder panel (lower F-key) OR decrease width.
        KeyCode::Left | KeyCode::Char('-') => {
            let i = state.options_cursor;
            if i < panel_count {
                if i > 0 {
                    state.layout.panels.swap(i, i - 1);
                    state.options_cursor -= 1;
                    return SidebarKeyResult::SaveLayout;
                }
            } else if i == panel_count && state.layout.width > 15 {
                state.layout.width -= 1;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }
        _ => SidebarKeyResult::Unhandled,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Render the sidebar into `area`.  The caller is responsible for ensuring
/// `area` is only passed when the sidebar is visible.
pub fn draw(frame: &mut Frame, state: &SidebarState, area: Rect) {
    if area.height < 3 {
        return;
    }

    // Tab bar: 1 row.  Panel content: remainder.
    let tab_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    let content_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };

    draw_tabs(frame, state, tab_area);

    // Display the active panel when focused, otherwise the char sheet as default.
    let panel  = state.active_panel.as_ref().unwrap_or(&PanelKind::CharSheet);
    let focused = state.active_panel.is_some();

    match panel {
        PanelKind::CharSheet => draw_kv_panel(
            frame, "Char Sheet",
            &state.char_sheet.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
            state.panel_cursor,
            content_area,
            focused,
            Some("r:remove"),
        ),
        PanelKind::Paperdoll => draw_kv_panel(
            frame, "Paperdoll",
            &state.paperdoll.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
            state.panel_cursor,
            content_area,
            focused,
            Some("Enter/r:remove"),
        ),
        PanelKind::Inventory => draw_list_panel(
            frame, "Inventory",
            &state.inventory.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            state.panel_cursor,
            content_area,
            focused,
            Some("Enter/w:wear  d:drop"),
        ),
        PanelKind::Automap => draw_placeholder(
            frame,
            "Automap\n\n(Phase 8)",
            content_area,
        ),
    }

    if state.options_open {
        draw_options_modal(frame, state, area);
    }
}

fn draw_tabs(frame: &mut Frame, state: &SidebarState, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    // F1 hint (game).
    let f1_style = if state.active_panel.is_none() {
        Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray).bg(Color::DarkGray)
    };
    spans.push(Span::styled("F1", f1_style));
    spans.push(Span::styled("|", Style::default().fg(Color::DarkGray).bg(Color::DarkGray)));

    for (i, panel) in state.layout.panels.iter().enumerate() {
        let fkey = i + 2;
        let active = state.active_panel.as_ref() == Some(panel);
        let style = if active {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray).bg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("F{}:{}", fkey, panel.short_label()), style));
        spans.push(Span::styled("|", Style::default().fg(Color::DarkGray).bg(Color::DarkGray)));
    }
    if state.active_panel.is_some() {
        spans.push(Span::styled("o:opts", Style::default().fg(Color::DarkGray).bg(Color::DarkGray)));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(Color::DarkGray)),
        area,
    );
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
    let panel_count = state.layout.panels.len();
    // rows: panel_count panel entries + 1 separator + 1 width + 1 close + 2 borders
    let modal_h = (panel_count as u16 + 5).min(parent.height);
    let modal_w = parent.width.min(34).max(20);
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

    // Panel rows.
    for (i, panel) in state.layout.panels.iter().enumerate() {
        let fkey = i + 2;
        let selected = state.options_cursor == i;
        let sel_style = if selected {
            Style::default().bg(Color::White).fg(Color::Black)
        } else {
            Style::default()
        };
        let hint = if selected { " \u{2190}\u{2192} reorder" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(format!(" F{fkey} "), Style::default().fg(Color::Yellow)),
            Span::styled(format!("{:<8}", panel.short_label()), sel_style),
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Separator.
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(inner.width as usize),
        Style::default().fg(Color::DarkGray),
    )));

    // Width row.
    let w_sel = state.options_cursor == panel_count;
    let w_style = if w_sel {
        Style::default().bg(Color::White).fg(Color::Black)
    } else {
        Style::default()
    };
    lines.push(Line::from(vec![
        Span::styled(" Width ", Style::default().fg(Color::Yellow)),
        Span::styled(format!("{:>3}", state.layout.width), w_style),
        Span::styled(
            if w_sel { "  \u{2190}\u{2192} adjust" } else { "" },
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    // Close row.
    let c_sel   = state.options_cursor == panel_count + 1;
    let c_style = if c_sel {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(Span::styled(
        " [Close]  Esc to save & close",
        c_style,
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}
