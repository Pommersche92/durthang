//! Server / character selection screen.
//!
//! Layout:
//!   ┌─ Servers (N) ──────────────────────────────┐
//!   │ ▼ My MUD                                   │
//!   │ ▶ Another MUD                              │
//!   └────────────────────────────────────────────┘
//!   ┌─ Characters — My MUD (N) ──────────────────┐
//!   │   Thorien  (hint: main account)            │
//!   └────────────────────────────────────────────┘
//!   ↑↓ move  Tab switch  Space expand  n add  e edit  d delete  Enter connect  q quit

use std::collections::HashSet;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use tracing::warn;

use crate::config::{self, Character, Config, Server};

// ---------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Servers,
    Characters,
}

// ---------------------------------------------------------------------------
// Dialog types
// ---------------------------------------------------------------------------

pub struct DialogField {
    pub label: &'static str,
    pub value: String,
    pub masked: bool,
}

impl DialogField {
    fn plain(label: &'static str) -> Self {
        Self { label, value: String::new(), masked: false }
    }
    fn prefilled(label: &'static str, value: impl Into<String>) -> Self {
        Self { label, value: value.into(), masked: false }
    }
    fn secret(label: &'static str) -> Self {
        Self { label, value: String::new(), masked: true }
    }
}

pub struct TextDialog {
    pub title: &'static str,
    pub fields: Vec<DialogField>,
    pub focused: usize,
}

pub enum DeleteTarget {
    Server(String),
    Character(String),
}

pub enum Dialog {
    AddServer(TextDialog),
    EditServer { server_id: String, inner: TextDialog },
    AddCharacter { server_id: String, inner: TextDialog },
    EditCharacter { char_id: String, inner: TextDialog },
    ConfirmDelete { target: DeleteTarget },
}

impl Dialog {
    fn add_server() -> Self {
        Dialog::AddServer(TextDialog {
            title: "Add Server",
            fields: vec![
                DialogField::plain("Name"),
                DialogField::plain("Host"),
                DialogField::prefilled("Port", "23"),
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
                    DialogField::secret("Password"),
                    DialogField::plain("Password hint (optional)"),
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
                    DialogField::secret("Password (empty = keep current)"),
                    DialogField::prefilled(
                        "Password hint",
                        c.password_hint.as_deref().unwrap_or(""),
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
    pub focus: Focus,
    pub server_idx: usize,
    pub char_idx: usize,
    /// Server IDs displayed with a collapsed (▶) indicator.
    pub collapsed: HashSet<String>,
    pub dialog: Option<Dialog>,
    /// Set when the user presses Enter on a character — triggers connect.
    pub pending_connect: Option<(String, String)>, // (server_id, char_id)
}

impl SelectState {
    pub fn new() -> Self {
        Self {
            focus: Focus::default(),
            server_idx: 0,
            char_idx: 0,
            collapsed: HashSet::new(),
            dialog: None,
            pending_connect: None,
        }
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

/// Handle a keystroke inside a `TextDialog`. Mutates `d` in place.
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

/// Handle a key event for the active dialog.
/// Returns `true` if the dialog should be closed afterwards.
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
                        state.server_idx = state.server_idx.saturating_sub(1);
                        state.char_idx = 0;
                    }
                    DeleteTarget::Character(id) => {
                        let id = id.clone();
                        config.characters.retain(|c| c.id != id);
                        state.char_idx = state.char_idx.saturating_sub(1);
                    }
                }
                save_config(config, config_path);
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
                if !name.is_empty() && !host.is_empty() {
                    config.servers.push(Server::new(name, host, port));
                    state.server_idx = config.servers.len().saturating_sub(1);
                    state.char_idx = 0;
                    save_config(config, config_path);
                }
                true
            }
        },

        Dialog::EditServer { server_id, inner: d } => {
            let server_id = server_id.clone();
            match text_input(d, key) {
                TextInputResult::Cancel => true,
                TextInputResult::Continue => false,
                TextInputResult::Confirm => {
                    let name = d.fields[0].value.trim().to_string();
                    let host = d.fields[1].value.trim().to_string();
                    let port = d.fields[2].value.trim().parse::<u16>().unwrap_or(23);
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
                    }
                    save_config(config, config_path);
                    true
                }
            }
        }

        Dialog::AddCharacter { server_id, inner: d } => {
            let server_id = server_id.clone();
            match text_input(d, key) {
                TextInputResult::Cancel => true,
                TextInputResult::Continue => false,
                TextInputResult::Confirm => {
                    let name = d.fields[0].value.trim().to_string();
                    let password = d.fields[1].value.clone();
                    let hint = d.fields[2].value.trim().to_string();
                    if !name.is_empty() {
                        let mut ch = Character::new(&name, &server_id);
                        if !hint.is_empty() {
                            ch.password_hint = Some(hint);
                        }
                        if !password.is_empty() {
                            if let Err(e) = config::store_password(&server_id, &name, &password) {
                                warn!("Could not store password in keyring: {e}");
                            }
                        }
                        config.characters.push(ch);
                        save_config(config, config_path);
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
                    let password = d.fields[1].value.clone();
                    let hint = d.fields[2].value.trim().to_string();
                    if let Some(ch) = config.characters.iter_mut().find(|c| c.id == char_id) {
                        let server_id = ch.server_id.clone();
                        let old_name = ch.name.clone();
                        if !new_name.is_empty() {
                            ch.name = new_name;
                        }
                        ch.password_hint = if hint.is_empty() { None } else { Some(hint) };
                        if !password.is_empty() {
                            if let Err(e) =
                                config::store_password(&server_id, &ch.name, &password)
                            {
                                warn!("Could not update password in keyring: {e}");
                            }
                            if old_name != ch.name {
                                let _ = config::delete_password(&server_id, &old_name);
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

/// Handle a key event on the server/character selection screen.
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

    // Delegate to dialog if one is open.
    if let Some(mut dialog) = state.dialog.take() {
        let close = handle_dialog_key(&mut dialog, state, config, config_path, key);
        if !close {
            state.dialog = Some(dialog);
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') => return true,

        KeyCode::Tab => {
            state.focus = Focus::Characters;
            state.char_idx = 0;
        }
        KeyCode::BackTab => {
            state.focus = Focus::Servers;
        }

        KeyCode::Up => match state.focus {
            Focus::Servers => {
                state.server_idx = state.server_idx.saturating_sub(1);
                state.char_idx = 0;
            }
            Focus::Characters => {
                state.char_idx = state.char_idx.saturating_sub(1);
            }
        },

        KeyCode::Down => match state.focus {
            Focus::Servers => {
                if state.server_idx + 1 < config.servers.len() {
                    state.server_idx += 1;
                    state.char_idx = 0;
                }
            }
            Focus::Characters => {
                let max = config
                    .servers
                    .get(state.server_idx)
                    .map(|s| config.characters_for_server(&s.id).len())
                    .unwrap_or(0)
                    .saturating_sub(1);
                if state.char_idx < max {
                    state.char_idx += 1;
                }
            }
        },

        KeyCode::Char(' ') if state.focus == Focus::Servers => {
            if let Some(server) = config.servers.get(state.server_idx) {
                let id = server.id.clone();
                if !state.collapsed.remove(&id) {
                    state.collapsed.insert(id);
                }
            }
        }

        KeyCode::Enter => match state.focus {
            Focus::Servers => {
                state.focus = Focus::Characters;
                state.char_idx = 0;
            }
            Focus::Characters => {
                if let Some(server) = config.servers.get(state.server_idx) {
                    let server_id = server.id.clone();
                    let chars = config.characters_for_server(&server_id);
                    if let Some(ch) = chars.get(state.char_idx) {
                        state.pending_connect = Some((server_id, ch.id.clone()));
                    }
                }
            }
        },

        KeyCode::Char('n') => {
            state.dialog = Some(match state.focus {
                Focus::Servers => Dialog::add_server(),
                Focus::Characters => {
                    if let Some(s) = config.servers.get(state.server_idx) {
                        Dialog::add_character(&s.id.clone())
                    } else {
                        Dialog::add_server()
                    }
                }
            });
        }

        KeyCode::Char('e') => {
            state.dialog = match state.focus {
                Focus::Servers => config.servers.get(state.server_idx).map(Dialog::edit_server),
                Focus::Characters => {
                    let server_id = config.servers.get(state.server_idx).map(|s| s.id.clone());
                    server_id.and_then(|sid| {
                        let chars = config.characters_for_server(&sid);
                        chars.get(state.char_idx).map(|c| Dialog::edit_character(c))
                    })
                }
            };
        }

        KeyCode::Char('d') => {
            let target = match state.focus {
                Focus::Servers => config
                    .servers
                    .get(state.server_idx)
                    .map(|s| DeleteTarget::Server(s.id.clone())),
                Focus::Characters => {
                    let server_id = config.servers.get(state.server_idx).map(|s| s.id.clone());
                    server_id.and_then(|sid| {
                        let chars = config.characters_for_server(&sid);
                        chars.get(state.char_idx).map(|c| DeleteTarget::Character(c.id.clone()))
                    })
                }
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

/// Render the full selection screen, including any active dialog overlay.
pub fn draw(frame: &mut Frame, state: &SelectState, config: &Config) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Fill(1),    // server list
        Constraint::Fill(1),    // character list
        Constraint::Length(1),  // status bar
    ])
    .split(area);

    draw_servers(frame, chunks[0], state, config);
    draw_characters(frame, chunks[1], state, config);
    draw_status_bar(frame, chunks[2], &state.dialog);

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

fn draw_servers(frame: &mut Frame, area: Rect, state: &SelectState, config: &Config) {
    let focused = state.focus == Focus::Servers;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let highlight_style = if focused {
        Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };

    let items: Vec<ListItem> = if config.servers.is_empty() {
        vec![ListItem::new("  (no servers — press n to add)")
            .style(Style::default().fg(Color::DarkGray))]
    } else {
        config
            .servers
            .iter()
            .map(|s| {
                let icon = if state.collapsed.contains(&s.id) { "▶ " } else { "▼ " };
                ListItem::new(format!("{icon}{}", s.name))
            })
            .collect()
    };

    let title = format!(" Servers ({}) ", config.servers.len());
    let list = List::new(items)
        .block(Block::bordered().title(title).border_style(border_style))
        .highlight_style(highlight_style);

    let mut list_state = ListState::default();
    if !config.servers.is_empty() {
        list_state.select(Some(state.server_idx.min(config.servers.len().saturating_sub(1))));
    }
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_characters(frame: &mut Frame, area: Rect, state: &SelectState, config: &Config) {
    let focused = state.focus == Focus::Characters;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let highlight_style = if focused {
        Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };

    let (server_name, chars) = match config.servers.get(state.server_idx) {
        Some(s) => (s.name.as_str(), config.characters_for_server(&s.id)),
        None => ("—", vec![]),
    };

    let items: Vec<ListItem> = if chars.is_empty() {
        vec![ListItem::new("  (no characters — press n to add)")
            .style(Style::default().fg(Color::DarkGray))]
    } else {
        chars
            .iter()
            .map(|ch| {
                let hint = ch
                    .password_hint
                    .as_deref()
                    .map(|h| format!("  (hint: {h})"))
                    .unwrap_or_default();
                ListItem::new(format!("  {}{hint}", ch.name))
            })
            .collect()
    };

    let title = format!(" Characters — {server_name} ({}) ", chars.len());
    let list = List::new(items)
        .block(Block::bordered().title(title).border_style(border_style))
        .highlight_style(highlight_style);

    let mut list_state = ListState::default();
    if !chars.is_empty() {
        list_state.select(Some(state.char_idx.min(chars.len().saturating_sub(1))));
    }
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_status_bar(frame: &mut Frame, area: Rect, dialog: &Option<Dialog>) {
    let hints = match dialog {
        None => {
            " ↑↓ move   Tab switch panel   Space expand/collapse   \
             n add   e edit   d delete   Enter connect   q quit"
        }
        Some(Dialog::ConfirmDelete { .. }) => " y yes   n / Esc no",
        Some(_) => " Tab/↑↓ next field   Enter confirm   Esc cancel",
    };
    let p = Paragraph::new(hints).style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(p, area);
}

fn draw_text_dialog(frame: &mut Frame, area: Rect, d: &TextDialog) {
    let dialog_w = 58u16;
    // borders(2) + 1 blank padding + one row per field + 1 blank padding
    let dialog_h = d.fields.len() as u16 + 4;
    let dialog_area = centered_rect(dialog_w, dialog_h, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines: Vec<Line> = vec![Line::from("")]; // top padding
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
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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
