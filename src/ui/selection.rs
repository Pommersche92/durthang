// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Server / character selection screen.
//!
//! Layout (single-panel tree-view):
//!   ┌─ Servers & Characters ─────────────────────┐
//!   │ ▼ My MUD                                   │
//!   │   ├ Thorien  (hint: main account)          │
//!   │   └ Erevan                                 │
//!   │ ▶ Another MUD  (collapsed)                 │
//!   └────────────────────────────────────────────┘
//!   ↑↓ move  Space/←/► expand/collapse  n add char  N add server
//!   Enter connect  e edit  d delete  q quit

use std::collections::HashSet;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph},
};
use tracing::warn;

use crate::config::{self, Character, Config, Server};

// ---------------------------------------------------------------------------
// Flat tree representation
// ---------------------------------------------------------------------------

/// One visible row in the flat tree list.
///
/// The tree is re-built from scratch on every key event and frame draw,
/// so this type acts as a simple view projection rather than a persistent
/// data model.
#[derive(Debug)]
enum TreeRow {
    Server {
        server_id: String,
        name: String,
        collapsed: bool,
        char_count: usize,
    },
    Character {
        server_id: String,
        char_id: String,
        name: String,
        login: Option<String>,
        hint: Option<String>,
        notes: Option<String>,
        /// Whether this is the last character under its server (for the connector glyph).
        is_last: bool,
    },
}

/// Build the flat, visible tree from the current config and collapsed set.
///
/// Each server is always present; its characters are omitted when the server
/// row is in the collapsed set.  The `is_last` flag on character rows
/// controls whether `└` or `├` is used as the tree connector glyph.
fn build_tree(config: &Config, collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    for server in &config.servers {
        let chars = config.characters_for_server(&server.id);
        let is_collapsed = collapsed.contains(&server.id);
        rows.push(TreeRow::Server {
            server_id: server.id.clone(),
            name: server.name.clone(),
            collapsed: is_collapsed,
            char_count: chars.len(),
        });
        if !is_collapsed {
            let last = chars.len().saturating_sub(1);
            for (i, ch) in chars.iter().enumerate() {
                rows.push(TreeRow::Character {
                    server_id: server.id.clone(),
                    char_id: ch.id.clone(),
                    name: ch.name.clone(),
                    login: ch.login.clone(),
                    hint: ch.password_hint.clone(),
                    notes: ch.notes.clone(),
                    is_last: i == last,
                });
            }
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// Dialog types
// ---------------------------------------------------------------------------

/// A single editable field inside a [`TextDialog`].
pub struct DialogField {
    pub label: &'static str,
    pub value: String,
    pub masked: bool,
}

impl DialogField {
    /// Create an empty, unmasked field with the given label.
    fn plain(label: &'static str) -> Self {
        Self {
            label,
            value: String::new(),
            masked: false,
        }
    }
    /// Create an unmasked field with the given label and a pre-filled value.
    fn prefilled(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            masked: false,
        }
    }
    /// Create an empty field whose value is rendered as asterisks.
    fn secret(label: &'static str) -> Self {
        Self {
            label,
            value: String::new(),
            masked: true,
        }
    }
}

/// A modal dialog with one or more labelled text fields and a focused index.
pub struct TextDialog {
    pub title: &'static str,
    pub fields: Vec<DialogField>,
    pub focused: usize,
}

/// The item to be deleted when the user confirms a delete dialog.
pub enum DeleteTarget {
    Server(String),
    Character(String),
}

/// All possible modal dialogs that can be open on the selection screen.
pub enum Dialog {
    AddServer(TextDialog),
    EditServer {
        server_id: String,
        inner: TextDialog,
    },
    AddCharacter {
        server_id: String,
        inner: TextDialog,
    },
    EditCharacter {
        char_id: String,
        inner: TextDialog,
    },
    ConfirmDelete {
        target: DeleteTarget,
    },
}

impl Dialog {
    fn add_server() -> Self {
        Dialog::AddServer(TextDialog {
            title: "Add Server",
            fields: vec![
                DialogField::plain("Name"),
                DialogField::plain("Host"),
                DialogField::prefilled("Port", "23"),
                DialogField::prefilled("TLS (y/n)", "n"),
            ],
            focused: 0,
        })
    }

    fn edit_server(s: &Server) -> Self {
        Dialog::EditServer {
            server_id: s.id.clone(),
            inner: TextDialog {
                title: "Edit Server",
                fields: vec![
                    DialogField::prefilled("Name", &s.name),
                    DialogField::prefilled("Host", &s.host),
                    DialogField::prefilled("Port", s.port.to_string()),
                    DialogField::prefilled("TLS (y/n)", if s.tls { "y" } else { "n" }),
                ],
                focused: 0,
            },
        }
    }

    fn add_character(server_id: &str) -> Self {
        Dialog::AddCharacter {
            server_id: server_id.to_string(),
            inner: TextDialog {
                title: "Add Character",
                fields: vec![
                    DialogField::plain("Name"),
                    DialogField::plain("Login (empty = same as Name)"),
                    DialogField::secret("Password"),
                    DialogField::plain("Password hint (optional)"),
                    DialogField::plain("Notes (race, class, …)"),
                ],
                focused: 0,
            },
        }
    }

    fn edit_character(c: &Character) -> Self {
        Dialog::EditCharacter {
            char_id: c.id.clone(),
            inner: TextDialog {
                title: "Edit Character",
                fields: vec![
                    DialogField::prefilled("Name", &c.name),
                    DialogField::prefilled(
                        "Login (empty = same as Name)",
                        c.login.as_deref().unwrap_or(""),
                    ),
                    DialogField::secret("Password (empty = keep current)"),
                    DialogField::prefilled(
                        "Password hint",
                        c.password_hint.as_deref().unwrap_or(""),
                    ),
                    DialogField::prefilled(
                        "Notes (race, class, …)",
                        c.notes.as_deref().unwrap_or(""),
                    ),
                ],
                focused: 0,
            },
        }
    }

    fn as_text_dialog(&self) -> Option<&TextDialog> {
        match self {
            Dialog::AddServer(d) => Some(d),
            Dialog::EditServer { inner, .. } => Some(inner),
            Dialog::AddCharacter { inner, .. } => Some(inner),
            Dialog::EditCharacter { inner, .. } => Some(inner),
            Dialog::ConfirmDelete { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Selection state
// ---------------------------------------------------------------------------

pub struct SelectState {
    /// Flat cursor index into the currently visible tree.
    pub cursor: usize,
    /// Server IDs whose children are hidden.
    pub collapsed: HashSet<String>,
    pub dialog: Option<Dialog>,
    /// Set when the user presses Enter on a character or a server with no characters.
    /// Inner Option<String> is the char_id; None means connect without a saved character.
    pub pending_connect: Option<(String, Option<String>)>, // (server_id, char_id?)
}

impl SelectState {
    pub fn new() -> Self {
        Self {
            cursor: 0,
            collapsed: HashSet::new(),
            dialog: None,
            pending_connect: None,
        }
    }

    /// Clamp cursor to the current visible tree length.
    fn clamp(&mut self, tree_len: usize) {
        if tree_len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(tree_len - 1);
        }
    }

    /// After a config change, move cursor to the row with the given server id.
    fn move_to_server(&mut self, config: &Config, server_id: &str) {
        let tree = build_tree(config, &self.collapsed);
        if let Some(pos) = tree
            .iter()
            .position(|r| matches!(r, TreeRow::Server { server_id: sid, .. } if sid == server_id))
        {
            self.cursor = pos;
        }
        self.clamp(tree.len());
    }

    /// After a config change, move cursor to the row with the given character id.
    fn move_to_char(&mut self, config: &Config, char_id: &str) {
        let tree = build_tree(config, &self.collapsed);
        if let Some(pos) = tree
            .iter()
            .position(|r| matches!(r, TreeRow::Character { char_id: cid, .. } if cid == char_id))
        {
            self.cursor = pos;
        }
        self.clamp(tree.len());
    }
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

enum TextInputResult {
    Continue,
    Confirm,
    Cancel,
}

fn text_input(d: &mut TextDialog, key: KeyEvent) -> TextInputResult {
    match key.code {
        KeyCode::Esc => TextInputResult::Cancel,
        KeyCode::Enter => {
            if d.focused + 1 < d.fields.len() {
                d.focused += 1;
                TextInputResult::Continue
            } else {
                TextInputResult::Confirm
            }
        }
        KeyCode::Tab | KeyCode::Down => {
            if d.focused + 1 < d.fields.len() {
                d.focused += 1;
            }
            TextInputResult::Continue
        }
        KeyCode::BackTab | KeyCode::Up => {
            if d.focused > 0 {
                d.focused -= 1;
            }
            TextInputResult::Continue
        }
        KeyCode::Backspace => {
            d.fields[d.focused].value.pop();
            TextInputResult::Continue
        }
        KeyCode::Char(c) => {
            d.fields[d.focused].value.push(c);
            TextInputResult::Continue
        }
        _ => TextInputResult::Continue,
    }
}

fn save_config(config: &Config, path: &Path) {
    if let Err(e) = config.save(path) {
        warn!("Failed to save config: {e}");
    }
}

fn handle_dialog_key(
    dialog: &mut Dialog,
    state: &mut SelectState,
    config: &mut Config,
    config_path: &Path,
    key: KeyEvent,
) -> bool {
    match dialog {
        Dialog::ConfirmDelete { target } => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                match target {
                    DeleteTarget::Server(id) => {
                        let id = id.clone();
                        config.characters.retain(|c| c.server_id != id);
                        config.servers.retain(|s| s.id != id);
                    }
                    DeleteTarget::Character(id) => {
                        let id = id.clone();
                        config.characters.retain(|c| c.id != id);
                    }
                }
                save_config(config, config_path);
                let tree_len = build_tree(config, &state.collapsed).len();
                state.clamp(tree_len);
                // Move cursor up one if we're now past the end.
                if state.cursor > 0 && tree_len > 0 {
                    state.cursor = state.cursor.saturating_sub(1);
                    state.clamp(tree_len);
                }
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => true,
            _ => false,
        },

        Dialog::AddServer(d) => match text_input(d, key) {
            TextInputResult::Cancel => true,
            TextInputResult::Continue => false,
            TextInputResult::Confirm => {
                let name = d.fields[0].value.trim().to_string();
                let host = d.fields[1].value.trim().to_string();
                let port = d.fields[2].value.trim().parse::<u16>().unwrap_or(23);
                let tls = matches!(
                    d.fields[3].value.trim().to_lowercase().as_str(),
                    "y" | "yes" | "true" | "1"
                );
                if !name.is_empty() && !host.is_empty() {
                    let mut server = Server::new(name, host, port);
                    server.tls = tls;
                    let sid = server.id.clone();
                    config.servers.push(server);
                    save_config(config, config_path);
                    state.move_to_server(config, &sid);
                }
                true
            }
        },

        Dialog::EditServer {
            server_id,
            inner: d,
        } => {
            let server_id = server_id.clone();
            match text_input(d, key) {
                TextInputResult::Cancel => true,
                TextInputResult::Continue => false,
                TextInputResult::Confirm => {
                    let name = d.fields[0].value.trim().to_string();
                    let host = d.fields[1].value.trim().to_string();
                    let port = d.fields[2].value.trim().parse::<u16>().unwrap_or(23);
                    let tls = matches!(
                        d.fields[3].value.trim().to_lowercase().as_str(),
                        "y" | "yes" | "true" | "1"
                    );
                    if let Some(s) = config.servers.iter_mut().find(|s| s.id == server_id) {
                        if !name.is_empty() {
                            s.name = name;
                        }
                        if !host.is_empty() {
                            s.host = host;
                        }
                        if port > 0 {
                            s.port = port;
                        }
                        s.tls = tls;
                    }
                    save_config(config, config_path);
                    true
                }
            }
        }

        Dialog::AddCharacter {
            server_id,
            inner: d,
        } => {
            let server_id = server_id.clone();
            match text_input(d, key) {
                TextInputResult::Cancel => true,
                TextInputResult::Continue => false,
                TextInputResult::Confirm => {
                    let name = d.fields[0].value.trim().to_string();
                    let login = d.fields[1].value.trim().to_string();
                    let password = d.fields[2].value.clone();
                    let hint = d.fields[3].value.trim().to_string();
                    let notes = d.fields[4].value.trim().to_string();
                    if !name.is_empty() {
                        let mut ch = Character::new(&name, &server_id);
                        // Only persist login if it differs from the display name.
                        if !login.is_empty() && login != name {
                            ch.login = Some(login);
                        }
                        if !hint.is_empty() {
                            ch.password_hint = Some(hint);
                        }
                        if !notes.is_empty() {
                            ch.notes = Some(notes);
                        }
                        if !password.is_empty() {
                            let effective = ch.effective_login().to_string();
                            if let Err(e) =
                                config::store_password(&server_id, &effective, &password)
                            {
                                warn!("Could not store password in keyring: {e}");
                            }
                        }
                        let cid = ch.id.clone();
                        // Ensure server is expanded so the new char is visible.
                        state.collapsed.remove(&server_id);
                        config.characters.push(ch);
                        save_config(config, config_path);
                        state.move_to_char(config, &cid);
                    }
                    true
                }
            }
        }

        Dialog::EditCharacter { char_id, inner: d } => {
            let char_id = char_id.clone();
            match text_input(d, key) {
                TextInputResult::Cancel => true,
                TextInputResult::Continue => false,
                TextInputResult::Confirm => {
                    let new_name = d.fields[0].value.trim().to_string();
                    let login = d.fields[1].value.trim().to_string();
                    let password = d.fields[2].value.clone();
                    let hint = d.fields[3].value.trim().to_string();
                    let notes = d.fields[4].value.trim().to_string();
                    if let Some(ch) = config.characters.iter_mut().find(|c| c.id == char_id) {
                        let server_id = ch.server_id.clone();
                        let old_login = ch.effective_login().to_string();
                        if !new_name.is_empty() {
                            ch.name = new_name;
                        }
                        // Only store a separate login if it actually differs from the name.
                        ch.login = if !login.is_empty() && login != ch.name {
                            Some(login)
                        } else {
                            None
                        };
                        ch.password_hint = if hint.is_empty() { None } else { Some(hint) };
                        ch.notes = if notes.is_empty() { None } else { Some(notes) };
                        if !password.is_empty() {
                            let new_login = ch.effective_login().to_string();
                            if let Err(e) =
                                config::store_password(&server_id, &new_login, &password)
                            {
                                warn!("Could not update password in keyring: {e}");
                            }
                            // Remove the old keyring entry if the login name changed.
                            if old_login != new_login {
                                let _ = config::delete_password(&server_id, &old_login);
                            }
                        }
                    }
                    save_config(config, config_path);
                    true
                }
            }
        }
    }
}

/// Handle a key event on the selection screen.
/// Returns `true` if the application should quit.
pub fn handle_key(
    state: &mut SelectState,
    config: &mut Config,
    config_path: &Path,
    key: KeyEvent,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }

    if let Some(mut dialog) = state.dialog.take() {
        let close = handle_dialog_key(&mut dialog, state, config, config_path, key);
        if !close {
            state.dialog = Some(dialog);
        }
        return false;
    }

    let tree = build_tree(config, &state.collapsed);
    state.clamp(tree.len());
    let current = tree.get(state.cursor);

    match key.code {
        KeyCode::Char('q') => return true,

        KeyCode::Up => {
            state.cursor = state.cursor.saturating_sub(1);
        }

        KeyCode::Down => {
            if state.cursor + 1 < tree.len() {
                state.cursor += 1;
            }
        }

        // Space / Left / Right → toggle collapse on a server row.
        KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
            if let Some(TreeRow::Server {
                server_id,
                collapsed,
                ..
            }) = current
            {
                let id = server_id.clone();
                if *collapsed {
                    state.collapsed.remove(&id);
                } else {
                    state.collapsed.insert(id);
                }
            }
        }

        KeyCode::Enter => {
            match current {
                Some(TreeRow::Server {
                    server_id,
                    collapsed,
                    char_count,
                    ..
                }) => {
                    if *char_count == 0 {
                        // No characters yet — connect directly without a saved character.
                        state.pending_connect = Some((server_id.clone(), None));
                    } else {
                        // Toggle expand/collapse.
                        let id = server_id.clone();
                        if *collapsed {
                            state.collapsed.remove(&id);
                        } else {
                            state.collapsed.insert(id);
                        }
                    }
                }
                Some(TreeRow::Character {
                    server_id, char_id, ..
                }) => {
                    state.pending_connect = Some((server_id.clone(), Some(char_id.clone())));
                }
                None => {}
            }
        }

        // n — add character to the selected (or nearest) server.
        // N (shift) — always add a new server.
        KeyCode::Char('n') => {
            let server_id = match current {
                Some(TreeRow::Server { server_id, .. }) => Some(server_id.clone()),
                Some(TreeRow::Character { server_id, .. }) => Some(server_id.clone()),
                None => None,
            };
            state.dialog = Some(match server_id {
                Some(sid) => Dialog::add_character(&sid),
                None => Dialog::add_server(),
            });
        }

        KeyCode::Char('N') => {
            state.dialog = Some(Dialog::add_server());
        }

        KeyCode::Char('e') => {
            state.dialog = match current {
                Some(TreeRow::Server { server_id, .. }) => {
                    let sid = server_id.clone();
                    config
                        .servers
                        .iter()
                        .find(|s| s.id == sid)
                        .map(Dialog::edit_server)
                }
                Some(TreeRow::Character { char_id, .. }) => {
                    let cid = char_id.clone();
                    config
                        .characters
                        .iter()
                        .find(|c| c.id == cid)
                        .map(Dialog::edit_character)
                }
                None => None,
            };
        }

        KeyCode::Char('d') => {
            let target = match current {
                Some(TreeRow::Server { server_id, .. }) => {
                    Some(DeleteTarget::Server(server_id.clone()))
                }
                Some(TreeRow::Character { char_id, .. }) => {
                    Some(DeleteTarget::Character(char_id.clone()))
                }
                None => None,
            };
            if let Some(t) = target {
                state.dialog = Some(Dialog::ConfirmDelete { target: t });
            }
        }

        _ => {}
    }

    false
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub fn draw(frame: &mut Frame, state: &SelectState, config: &Config) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Fill(1),   // tree
        Constraint::Length(1), // status bar
    ])
    .split(area);

    draw_tree(frame, chunks[0], state, config);
    draw_status_bar(frame, chunks[1], &state.dialog);

    if let Some(dialog) = &state.dialog {
        match dialog {
            Dialog::ConfirmDelete { target } => draw_confirm_dialog(frame, area, target, config),
            _ => {
                if let Some(d) = dialog.as_text_dialog() {
                    draw_text_dialog(frame, area, d);
                }
            }
        }
    }
}

fn draw_tree(frame: &mut Frame, area: Rect, state: &SelectState, config: &Config) {
    let tree = build_tree(config, &state.collapsed);
    let selected_idx = if tree.is_empty() {
        None
    } else {
        Some(state.cursor.min(tree.len() - 1))
    };

    let highlight_style = Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let items: Vec<ListItem> = if tree.is_empty() {
        vec![ListItem::new(Span::styled(
            "  (no servers — press N to add one)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        tree.iter()
            .map(|row| match row {
                TreeRow::Server {
                    name,
                    collapsed,
                    char_count,
                    server_id,
                    ..
                } => {
                    let icon = if *collapsed { "▶ " } else { "▼ " };
                    let tls_badge = if config
                        .servers
                        .iter()
                        .find(|s| &s.id == server_id)
                        .map(|s| s.tls)
                        .unwrap_or(false)
                    {
                        " [TLS]"
                    } else {
                        ""
                    };
                    let suffix = if *collapsed {
                        format!("  [{char_count}]")
                    } else {
                        String::new()
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(icon, Style::default().fg(Color::Yellow)),
                        Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::styled(tls_badge, Style::default().fg(Color::Green)),
                        Span::styled(suffix, Style::default().fg(Color::DarkGray)),
                    ]))
                }
                TreeRow::Character {
                    name,
                    login,
                    hint,
                    notes,
                    is_last,
                    ..
                } => {
                    let connector = if *is_last { "  └ " } else { "  ├ " };
                    let mut spans = vec![
                        Span::styled(connector, Style::default().fg(Color::DarkGray)),
                        Span::raw(name.clone()),
                    ];
                    // Show login in cyan brackets when it differs from the display name.
                    if let Some(l) = login.as_deref() {
                        spans.push(Span::styled(
                            format!(" [{l}]"),
                            Style::default().fg(Color::Cyan),
                        ));
                    }
                    // Show notes in yellow.
                    if let Some(n) = notes.as_deref() {
                        spans.push(Span::styled(
                            format!("  ⟨{n}⟩"),
                            Style::default().fg(Color::Yellow),
                        ));
                    }
                    // Show password hint dimmed at the end.
                    if let Some(h) = hint.as_deref() {
                        spans.push(Span::styled(
                            format!("  (hint: {h})"),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
            })
            .collect()
    };

    let title = format!(
        " Servers: {}   Characters: {} ",
        config.servers.len(),
        config.characters.len()
    );
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(highlight_style);

    let mut list_state = ListState::default();
    list_state.select(selected_idx);
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_status_bar(frame: &mut Frame, area: Rect, dialog: &Option<Dialog>) {
    let hints = match dialog {
        None => {
            " ↑↓ move   Space/←/► expand/collapse   \
             n add char   N add server   e edit   d delete   Enter connect/toggle   q quit"
        }
        Some(Dialog::ConfirmDelete { .. }) => " y yes   n / Esc no",
        Some(_) => " Tab/↑↓ next field   Enter confirm   Esc cancel",
    };
    let p = Paragraph::new(hints).style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(p, area);
}

fn draw_text_dialog(frame: &mut Frame, area: Rect, d: &TextDialog) {
    let dialog_w = 58u16;
    let dialog_h = d.fields.len() as u16 + 4;
    let dialog_area = centered_rect(dialog_w, dialog_h, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (i, f) in d.fields.iter().enumerate() {
        let prefix = if i == d.focused { "▶ " } else { "  " };
        let display_value = if f.masked {
            "•".repeat(f.value.len())
        } else {
            f.value.clone()
        };
        let cursor = if i == d.focused { "█" } else { "" };
        let content = format!("{prefix}{}: {display_value}{cursor}", f.label);
        if i == d.focused {
            lines.push(Line::styled(
                content,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            lines.push(Line::from(content));
        }
    }

    let paragraph = Paragraph::new(lines).block(
        Block::bordered()
            .title(format!(" {} ", d.title))
            .title_bottom(" Tab/↑↓ next   Enter OK   Esc cancel "),
    );
    frame.render_widget(paragraph, dialog_area);
}

fn draw_confirm_dialog(frame: &mut Frame, area: Rect, target: &DeleteTarget, config: &Config) {
    let (what, extra) = match target {
        DeleteTarget::Server(id) => {
            let name = config
                .servers
                .iter()
                .find(|s| s.id == *id)
                .map(|s| s.name.as_str())
                .unwrap_or("?");
            (
                format!("Delete server \"{name}\"?"),
                Some("All associated characters will also be deleted."),
            )
        }
        DeleteTarget::Character(id) => {
            let name = config
                .characters
                .iter()
                .find(|c| c.id == *id)
                .map(|c| c.name.as_str())
                .unwrap_or("?");
            (format!("Delete character \"{name}\"?"), None)
        }
    };

    let dialog_h = if extra.is_some() { 7u16 } else { 5u16 };
    let dialog_w = 54u16;
    let dialog_area = centered_rect(dialog_w, dialog_h, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines = vec![
        Line::from(""),
        Line::styled(what, Style::default().add_modifier(Modifier::BOLD)),
    ];
    if let Some(text) = extra {
        lines.push(Line::from(""));
        lines.push(Line::styled(text, Style::default().fg(Color::Yellow)));
    }

    let paragraph = Paragraph::new(lines).block(
        Block::bordered()
            .title(" Confirm Delete ")
            .title_bottom(" y yes   n / Esc no "),
    );
    frame.render_widget(paragraph, dialog_area);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}
