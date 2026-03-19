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
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

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
        }
    }

    /// Parse an ANSI string and append it to the scrollback buffer.
    pub fn push_line(&mut self, s: &str) {
        let line = ansi_to_line(s);
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

    /// Finalise the input, push to history, and return the line to send.
    fn confirm_input(&mut self) -> Option<String> {
        let line = self.input.trim_end().to_string();
        self.input.clear();
        self.input_cursor = 0;
        self.history_idx = None;
        self.history_snapshot.clear();

        // Echo input into the output buffer so the user sees what they typed.
        if !line.is_empty() {
            let echo = Line::from(vec![
                Span::styled("▶ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(line.clone()),
            ]);
            self.lines.push_back(echo);

            // Deduplicate consecutive identical history entries.
            if self.history.last().map(|l| l != &line).unwrap_or(true) {
                self.history.push(line.clone());
            }
        }

        Some(line)
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
}

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
        // Look for ESC [
        if bytes[i] == b'\x1b' && i + 1 < input.len() && bytes[i + 1] == b'[' {
            // Flush accumulated plain text.
            if text_start < i {
                spans.push(Span::styled(input[text_start..i].to_string(), style));
            }

            // Find the end of the escape sequence (the first ASCII letter).
            let seq_start = i + 2;
            let seq_end = input[seq_start..]
                .find(|c: char| c.is_ascii_alphabetic())
                .map(|j| seq_start + j)
                .unwrap_or(input.len());

            if seq_end < input.len() {
                let terminator = bytes[seq_end];
                let params = &input[seq_start..seq_end];
                if terminator == b'm' {
                    style = apply_sgr(style, params);
                }
                // All other escape sequences (cursor movement, etc.) are silently skipped.
                i = seq_end + 1;
            } else {
                // Unterminated sequence — skip to end.
                i = input.len();
            }
            text_start = i;
        } else if bytes[i] == b'\x1b' {
            // Lone ESC (not CSI) — skip it.
            if text_start < i {
                spans.push(Span::styled(input[text_start..i].to_string(), style));
            }
            i += 1;
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
/// Returns `Some(line)` when the user confirms a command that should be sent
/// to the server.  Returns `None` for all other keystrokes.
///
/// The caller is responsible for the disconnect key (Ctrl+Q) and global
/// Ctrl+C — this function only handles input editing, history, and scrollback.
pub fn handle_key(state: &mut GameState, key: KeyEvent) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

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
            // Leave history_idx so the user can still navigate without losing position,
            // but we do reset it so the next Up starts fresh from this edit.
            state.history_idx = None;
            None
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub fn draw(frame: &mut Frame, state: &mut GameState, server_name: &str, char_name: &str) {
    let area = frame.area();

    // Split: [output area | input line (1 row) | status bar (1 row)]
    let [output_area, input_area, status_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // The bordered output block consumes 2 rows for its borders.
    let output_inner_h = output_area.height.saturating_sub(2) as usize;
    state.last_output_height = output_inner_h.max(1);

    // --- Compute visible line window ---
    let buf_len = state.lines.len();
    // Clamp scroll so we never go past the top.
    let scroll = state.scroll_offset.min(buf_len.saturating_sub(1));

    let has_prompt = state.prompt.is_some() && scroll == 0;
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

    let output_block = Block::bordered()
        .title(format!(" {} ", server_name))
        .title_bottom(scroll_hint);

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
         ↑↓ history   PgUp/PgDn scroll   Ctrl+Q disconnect"
    );
    let status_line = Line::from(vec![conn_icon, Span::raw(info)]);
    frame.render_widget(
        Paragraph::new(status_line)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        status_area,
    );
}
