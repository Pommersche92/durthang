//! Sidebar panel system — automap minimap and user notes.
//!
//! # F-key layout
//!   F1      → focus back to the game input  
//!   F3      → toggle right sidebar visibility
//!   F4      → cycle focus to the next visible panel
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

use crate::config::{PanelConfig, PanelKind, SidebarLayout, SidebarSide};
use crate::map::{Direction, WorldMap};

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

    /// Automap world state.
    pub automap: WorldMap,

    // --- Interaction state ---

    /// Cursor row within the focused panel list.
    pub panel_cursor: usize,
    /// Cursor row in the options overlay.
    pub options_cursor: usize,

    // --- Notes editing state ---

    /// Whether the notes panel is currently in text-editing mode.
    pub notes_editing: bool,
    /// `true` while entering a brand-new note (so Esc removes it instead of reverting).
    pub notes_is_new: bool,
    /// Inline edit buffer for the note being written.
    pub notes_edit_buf: String,
    /// Byte offset of the cursor inside `notes_edit_buf`.
    pub notes_edit_cursor: usize,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self::new(SidebarLayout::default())
    }
}

impl SidebarState {
    pub fn new(mut layout: SidebarLayout) -> Self {
        migrate_layout(&mut layout);
        Self {
            layout,
            focused_panel: None,
            options_open: false,
            automap: WorldMap::default(),
            panel_cursor: 0,
            options_cursor: 0,
            notes_editing: false,
            notes_is_new: false,
            notes_edit_buf: String::new(),
            notes_edit_cursor: 0,
        }
    }

    /// Returns `true` if any panel is assigned to the given sidebar side.
    pub fn has_side_panels(&self, side: &SidebarSide) -> bool {
        self.layout.panels.iter().any(|p| p.side.as_ref() == Some(side))
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

    /// Cycles keyboard focus to the next visible right-sidebar panel (wraps; `None` → first).
    /// Resets `panel_cursor` to 0 whenever focus changes.
    pub fn focus_next_panel(&mut self) {
        let visible: Vec<PanelKind> = self.layout.panels.iter()
            .filter(|p| p.side == Some(SidebarSide::Right) && self.layout.right_visible)
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
    // Automap
    // ------------------------------------------------------------------

    pub fn map_apply_gmcp(&mut self, msg: &str) -> bool {
        self.automap.apply_gmcp_message(msg)
    }

    pub fn map_apply_output_line(&mut self, raw: &str) {
        self.automap.apply_exits_heuristic_from_output(raw);
    }

    pub fn map_set_position(&mut self, room_id: &str, x: i32, y: i32, z: i32) {
        self.automap.set_room_position(room_id, x, y, z);
    }

    pub fn map_link_rooms(&mut self, from_id: &str, dir: Direction, to_id: &str) {
        self.automap.link_rooms(from_id, dir, to_id);
    }

    pub fn map_current_room_id(&self) -> Option<String> {
        self.automap.current_room_id.clone()
    }

    fn active_panel_len(&self) -> usize {
        match &self.focused_panel {
            Some(PanelKind::Notes)   => self.layout.notes.len(),
            Some(PanelKind::Automap) => 0,
            _                        => 0,
        }
    }
}

/// Remove legacy panel kinds and ensure Automap + Notes are present on the right side.
fn migrate_layout(layout: &mut SidebarLayout) {
    // Drop panels whose kind no longer exists in active code.
    layout.panels.retain(|p| matches!(p.kind, PanelKind::Automap | PanelKind::Notes));

    // Guarantee Automap is present.
    if !layout.panels.iter().any(|p| p.kind == PanelKind::Automap) {
        layout.panels.insert(0, PanelConfig {
            kind: PanelKind::Automap,
            side: Some(SidebarSide::Right),
            height_pct: 35,
        });
    }

    // Guarantee Notes panel is present.
    if !layout.panels.iter().any(|p| p.kind == PanelKind::Notes) {
        layout.panels.push(PanelConfig {
            kind: PanelKind::Notes,
            side: Some(SidebarSide::Right),
            height_pct: 65,
        });
    }

    // Make sure Automap appears before Notes (minimap on top).
    let ai = layout.panels.iter().position(|p| p.kind == PanelKind::Automap);
    let ni = layout.panels.iter().position(|p| p.kind == PanelKind::Notes);
    if let (Some(ai), Some(ni)) = (ai, ni) {
        if ai > ni { layout.panels.swap(ai, ni); }
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

    // If in text-editing mode, delegate all keys to the notes editor.
    if state.notes_editing {
        return handle_notes_edit_key(state, key);
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

        // Notes: Add a new note ('a' or 'n').
        KeyCode::Char('a') | KeyCode::Char('n')
            if matches!(state.focused_panel, Some(PanelKind::Notes)) =>
        {
            let idx = state.layout.notes.len();
            state.layout.notes.push(String::new());
            state.panel_cursor      = idx;
            state.notes_editing     = true;
            state.notes_is_new      = true;
            state.notes_edit_buf    = String::new();
            state.notes_edit_cursor = 0;
            SidebarKeyResult::Consumed
        }

        // Notes: Edit the selected note (Enter or 'e').
        KeyCode::Enter | KeyCode::Char('e')
            if matches!(state.focused_panel, Some(PanelKind::Notes)) =>
        {
            if let Some(existing) = state.layout.notes.get(state.panel_cursor) {
                state.notes_editing     = true;
                state.notes_is_new      = false;
                state.notes_edit_buf    = existing.clone();
                state.notes_edit_cursor = existing.len();
            }
            SidebarKeyResult::Consumed
        }

        // Notes: Delete the selected note ('d' or Delete).
        KeyCode::Char('d') | KeyCode::Delete
            if matches!(state.focused_panel, Some(PanelKind::Notes)) =>
        {
            if state.panel_cursor < state.layout.notes.len() {
                state.layout.notes.remove(state.panel_cursor);
                if state.panel_cursor > 0 && state.panel_cursor >= state.layout.notes.len() {
                    state.panel_cursor -= 1;
                }
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }

        // Notes: Move the selected note up ('K').
        KeyCode::Char('K')
            if matches!(state.focused_panel, Some(PanelKind::Notes)) =>
        {
            let i = state.panel_cursor;
            if i > 0 && i < state.layout.notes.len() {
                state.layout.notes.swap(i - 1, i);
                state.panel_cursor -= 1;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }

        // Notes: Move the selected note down ('J').
        KeyCode::Char('J')
            if matches!(state.focused_panel, Some(PanelKind::Notes)) =>
        {
            let i = state.panel_cursor;
            if i + 1 < state.layout.notes.len() {
                state.layout.notes.swap(i, i + 1);
                state.panel_cursor += 1;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }

        _ => SidebarKeyResult::Unhandled,
    }
}

/// Handle key events while the Notes panel is in inline text-editing mode.
fn handle_notes_edit_key(state: &mut SidebarState, key: KeyEvent) -> SidebarKeyResult {
    match key.code {
        // Commit the edit.
        KeyCode::Enter => {
            let idx = state.panel_cursor;
            if idx < state.layout.notes.len() {
                state.layout.notes[idx] = std::mem::take(&mut state.notes_edit_buf);
            }
            state.notes_editing     = false;
            state.notes_edit_cursor = 0;
            SidebarKeyResult::SaveLayout
        }
        // Abort the edit; remove the note if it was freshly added.
        KeyCode::Esc => {
            if state.notes_is_new && state.panel_cursor < state.layout.notes.len() {
                state.layout.notes.remove(state.panel_cursor);
                if state.panel_cursor > 0 { state.panel_cursor -= 1; }
                state.notes_editing     = false;
                state.notes_edit_cursor = 0;
                return SidebarKeyResult::SaveLayout;
            }
            state.notes_editing     = false;
            state.notes_edit_cursor = 0;
            SidebarKeyResult::Consumed
        }
        // Backspace: delete the character before the cursor.
        KeyCode::Backspace => {
            if state.notes_edit_cursor > 0 {
                let mut pos = state.notes_edit_cursor - 1;
                while !state.notes_edit_buf.is_char_boundary(pos) { pos -= 1; }
                state.notes_edit_buf.remove(pos);
                state.notes_edit_cursor = pos;
            }
            SidebarKeyResult::Consumed
        }
        // Delete: delete the character after the cursor.
        KeyCode::Delete => {
            let pos = state.notes_edit_cursor;
            if pos < state.notes_edit_buf.len() {
                state.notes_edit_buf.remove(pos);
            }
            SidebarKeyResult::Consumed
        }
        // Move cursor left one character.
        KeyCode::Left => {
            if state.notes_edit_cursor > 0 {
                let mut pos = state.notes_edit_cursor - 1;
                while !state.notes_edit_buf.is_char_boundary(pos) { pos -= 1; }
                state.notes_edit_cursor = pos;
            }
            SidebarKeyResult::Consumed
        }
        // Move cursor right one character.
        KeyCode::Right => {
            let pos = state.notes_edit_cursor;
            if pos < state.notes_edit_buf.len() {
                let mut new_pos = pos + 1;
                while new_pos < state.notes_edit_buf.len()
                    && !state.notes_edit_buf.is_char_boundary(new_pos)
                {
                    new_pos += 1;
                }
                state.notes_edit_cursor = new_pos;
            }
            SidebarKeyResult::Consumed
        }
        // Home: jump to the start.
        KeyCode::Home => {
            state.notes_edit_cursor = 0;
            SidebarKeyResult::Consumed
        }
        // End: jump to the end.
        KeyCode::End => {
            state.notes_edit_cursor = state.notes_edit_buf.len();
            SidebarKeyResult::Consumed
        }
        // Regular character input.
        KeyCode::Char(c) => {
            state.notes_edit_buf.insert(state.notes_edit_cursor, c);
            state.notes_edit_cursor += c.len_utf8();
            SidebarKeyResult::Consumed
        }
        _ => SidebarKeyResult::Consumed,
    }
}

fn handle_options_key(state: &mut SidebarState, key: KeyEvent) -> SidebarKeyResult {
    let n_panels = state.layout.panels.len();
    // Rows: 0..n_panels = panels, n_panels = right width, n_panels+1 = close
    let n_rows = n_panels + 2;

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
            if state.options_cursor == n_panels + 1 {
                state.options_open = false;
                return SidebarKeyResult::SaveLayout;
            }
            SidebarKeyResult::Consumed
        }
        // → on panel row: toggle visibility (None ↔ Right).
        // → on width row: increase width.
        KeyCode::Right => {
            let i = state.options_cursor;
            if i < n_panels {
                let p = &mut state.layout.panels[i];
                p.side = match &p.side {
                    None                    => Some(SidebarSide::Right),
                    Some(SidebarSide::Right) => None,
                    Some(SidebarSide::Left)  => None,
                };
                SidebarKeyResult::SaveLayout
            } else if i == n_panels && state.layout.right_width < 60 {
                state.layout.right_width += 1;
                SidebarKeyResult::SaveLayout
            } else {
                SidebarKeyResult::Consumed
            }
        }
        // ← on panel row: toggle visibility (None ↔ Right).
        // ← on width row: decrease width.
        KeyCode::Left => {
            let i = state.options_cursor;
            if i < n_panels {
                let p = &mut state.layout.panels[i];
                p.side = match &p.side {
                    None                    => Some(SidebarSide::Right),
                    Some(SidebarSide::Right) => None,
                    Some(SidebarSide::Left)  => None,
                };
                SidebarKeyResult::SaveLayout
            } else if i == n_panels && state.layout.right_width > 12 {
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
        PanelKind::Automap => draw_automap_panel(frame, &state.automap, area, focused),
        PanelKind::Notes   => draw_notes_panel(frame, state, area, focused),
        _                  => {}
    }
}

fn draw_automap_panel(frame: &mut Frame, map: &WorldMap, area: Rect, focused: bool) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(Span::styled(
            " Automap ",
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(cur) = map.current_room() else {
        frame.render_widget(
            Paragraph::new("No map data yet\n(waiting for GMCP Room.Info or Exits: lines)")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    };

    if inner.width < 3 || inner.height < 3 {
        return;
    }

    let radius_x = (inner.width.saturating_sub(1) / 2) as i32;
    let radius_y = (inner.height.saturating_sub(1) / 2) as i32;

    let mut rows: Vec<Vec<char>> = vec![vec![' '; inner.width as usize]; inner.height as usize];

    for dy in -radius_y..=radius_y {
        for dx in -radius_x..=radius_x {
            let wx = cur.x + dx;
            let wy = cur.y + dy;
            if let Some(room) = map.room_at(wx, wy, cur.z) {
                let sx = (dx + radius_x) as usize;
                let sy = (dy + radius_y) as usize;
                if sy < rows.len() && sx < rows[sy].len() {
                    rows[sy][sx] = if room.id == cur.id { '@' } else { '.' };
                }
            }
        }
    }

    let mut lines: Vec<Line> = rows
        .into_iter()
        .map(|r| Line::raw(r.into_iter().collect::<String>()))
        .collect();

    // Bottom legend line in the panel body if there's enough height.
    if !lines.is_empty() {
        let legend = format!("@ {}  z:{}", cur.name, cur.z);
        let idx = lines.len() - 1;
        lines[idx] = Line::from(vec![
            Span::styled("@", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {}", legend.trim_start_matches('@').trim_start())),
        ]);
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_notes_panel(frame: &mut Frame, state: &SidebarState, area: Rect, focused: bool) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let hint = if state.notes_editing {
        "Enter:save  Esc:cancel"
    } else {
        "n:new  e:edit  d:del  K/J:move"
    };

    let mut block = Block::default()
        .title(Span::styled(
            " Notes ",
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    if focused {
        block = block.title_bottom(Span::styled(
            format!(" {hint} "),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.layout.notes.is_empty() && !state.notes_editing {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  (no notes — press n to add one)",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = state
        .layout
        .notes
        .iter()
        .enumerate()
        .map(|(i, note)| {
            let is_cursor = i == state.panel_cursor && focused;
            if is_cursor && state.notes_editing {
                // Show the edit buffer with a block cursor.
                let before = &state.notes_edit_buf[..state.notes_edit_cursor];
                let rest   = &state.notes_edit_buf[state.notes_edit_cursor..];
                let mut chars = rest.chars();
                let cursor_char = chars.next().unwrap_or(' ');
                let after: &str = chars.as_str();
                ListItem::new(Line::from(vec![
                    Span::raw(before.to_string()),
                    Span::styled(
                        cursor_char.to_string(),
                        Style::default().bg(Color::White).fg(Color::Black),
                    ),
                    Span::raw(after.to_string()),
                ]))
            } else {
                let style = if is_cursor {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default()
                };
                ListItem::new(Span::styled(note.clone(), style))
            }
        })
        .collect();

    let mut list_state = ListState::default();
    if focused && !state.layout.notes.is_empty() {
        list_state.select(Some(
            state.panel_cursor.min(state.layout.notes.len().saturating_sub(1)),
        ));
    }
    frame.render_stateful_widget(List::new(items), inner, &mut list_state);
}

fn draw_options_modal(frame: &mut Frame, state: &SidebarState, parent: Rect) {
    let n_panels = state.layout.panels.len();
    // rows: n_panels + separator + right_w + close + 2 borders
    let modal_h: u16 = (n_panels as u16 + 5).min(parent.height);
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
            None                    => " -- ",
            Some(SidebarSide::Left) => "Left",
            Some(SidebarSide::Right) => " Rt ",
        };
        let h_str = if pc.side.is_some() {
            format!("{:3}%", pc.height_pct)
        } else {
            "    ".to_string()
        };
        let hint = if selected { "  \u{2190}\u{2192}:vis  +/-:h%" } else { "" };
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

    // Width row
    let rw_sel   = state.options_cursor == n_panels;
    let rw_style = if rw_sel { Style::default().bg(Color::White).fg(Color::Black) } else { Style::default() };
    lines.push(Line::from(vec![
        Span::styled("Width   ", Style::default().fg(Color::Cyan)),
        Span::styled(format!("{:>3}", state.layout.right_width), rw_style),
        Span::styled(if rw_sel { "  \u{2190}\u{2192} adjust" } else { "" }, Style::default().fg(Color::DarkGray)),
    ]));

    // Close row
    let c_sel   = state.options_cursor == n_panels + 1;
    let c_style = if c_sel {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(Span::styled(" [Close]  Esc to save & close", c_style)));

    frame.render_widget(Paragraph::new(lines), inner);
}
