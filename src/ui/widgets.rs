// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only
//
// Durthang is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free
// Software Foundation, version 3.  See <https://www.gnu.org/licenses/gpl-3.0.html>.

//! Widget system for sidebar panels — gauge, kv_list, and grid/table displays.
//!
//! Widgets are configured in the character config and display GMCP data in the
//! sidebar. Each widget type has its own rendering logic and data extraction.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::config::{WidgetConfig, WidgetKind};

// ---------------------------------------------------------------------------
// Widget data storage
// ---------------------------------------------------------------------------

/// Parsed GMCP data stored for widget display.
///
/// This is a simple key-value store where keys are GMCP paths like "char.vitals.hp"
/// and values are the extracted string values.
#[derive(Debug, Clone, Default)]
pub struct WidgetDataStore {
    /// Map of GMCP path -> string value
    data: std::collections::HashMap<String, String>,
}

impl WidgetDataStore {
    /// Create a new empty data store.
    pub fn new() -> Self {
        Self {
            data: std::collections::HashMap::new(),
        }
    }

    /// Update or insert a value for a GMCP path.
    #[allow(dead_code)]
    pub fn set(&mut self, path: &str, value: &str) {
        self.data.insert(path.to_string(), value.to_string());
    }

    /// Get a value for a GMCP path, returning None if not found.
    pub fn get(&self, path: &str) -> Option<&str> {
        self.data.get(path).map(|s| s.as_str())
    }

    /// Parse a GMCP message and extract values for configured widget paths.
    ///
    /// GMCP messages are in the format: `Package.Subpackage {...json...}`
    /// This method extracts values and stores them under their full path.
    pub fn apply_gmcp(&mut self, msg: &str) {
        // Parse GMCP message format: "Package.Subpackage {...}"
        let Some((package, json)) = msg.split_once(' ') else {
            return;
        };

        // Try to parse the JSON payload
        let json: serde_json::Value = match serde_json::from_str(json.trim()) {
            Ok(v) => v,
            Err(_) => return,
        };

        // Extract all leaf values and store them with their full path
        extract_json_values(&json, package, &mut self.data);
    }
}

/// Recursively extract all leaf values from a JSON object and store with their paths.
fn extract_json_values(
    value: &serde_json::Value,
    prefix: &str,
    data: &mut std::collections::HashMap<String, String>,
) {
    match value {
        serde_json::Value::Object(obj) => {
            for (key, val) in obj {
                let new_path = format!("{}.{}", prefix, key);
                extract_json_values(val, &new_path, data);
            }
        }
        serde_json::Value::Array(arr) => {
            for (idx, val) in arr.iter().enumerate() {
                let new_path = format!("{}.{idx}", prefix);
                extract_json_values(val, &new_path, data);
            }
        }
        serde_json::Value::String(s) => {
            data.insert(prefix.to_string(), s.clone());
        }
        serde_json::Value::Number(n) => {
            data.insert(prefix.to_string(), n.to_string());
        }
        serde_json::Value::Bool(b) => {
            data.insert(prefix.to_string(), b.to_string());
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Widget rendering
// ---------------------------------------------------------------------------

/// Render a widget inside the given area.
pub fn draw_widget(
    frame: &mut Frame,
    config: &WidgetConfig,
    data: &WidgetDataStore,
    area: Rect,
    focused: bool,
) {
    match config.kind {
        WidgetKind::Gauge => draw_gauge(frame, config, data, area, focused),
        WidgetKind::KvList => draw_kv_list(frame, config, data, area, focused),
        WidgetKind::Grid => draw_grid(frame, config, data, area, focused),
    }
}

/// Draw a gauge widget (progress bar with label).
fn draw_gauge(
    frame: &mut Frame,
    config: &WidgetConfig,
    data: &WidgetDataStore,
    area: Rect,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", config.label),
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Get value and max from GMCP data
    let value: f64 = config
        .value_gmcp
        .as_deref()
        .and_then(|p| data.get(p))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);

    let max: f64 = config
        .max_gmcp
        .as_deref()
        .and_then(|p| data.get(p))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);

    let max = if max > 0.0 { max } else { 1.0 };
    let ratio = (value / max).clamp(0.0, 1.0);

    // Get color
    let color = config
        .color
        .as_deref()
        .and_then(parse_color_name)
        .unwrap_or(Color::Green);

    // Draw the gauge bar
    if inner.width > 0 && inner.height > 0 {
        let bar_width = inner.width as u16;
        let filled = (bar_width as f64 * ratio) as u16;
        let empty = bar_width.saturating_sub(filled);

        let bar_line = Line::from(vec![
            Span::styled(
                "█".repeat(filled as usize),
                Style::default().fg(color).bg(Color::DarkGray),
            ),
            Span::styled(
                "░".repeat(empty as usize),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        // Add value text below the bar if there's room
        if inner.height >= 2 {
            let value_text = format!("{value:.0} / {max:.0}");
            let text_line = Line::from(Span::styled(
                value_text,
                Style::default().fg(Color::White),
            ));
            frame.render_widget(
                ratatui::widgets::List::new(vec![bar_line, text_line]),
                inner,
            );
        } else {
            frame.render_widget(Paragraph::new(bar_line), inner);
        }
    }
}

/// Draw a key-value list widget.
fn draw_kv_list(
    frame: &mut Frame,
    config: &WidgetConfig,
    data: &WidgetDataStore,
    area: Rect,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", config.label),
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let items: Vec<ListItem> = config
        .keys
        .iter()
        .filter_map(|key| {
            let value = data.get(key)?;
            // Extract the last part of the key as the display label
            let label = key.split('.').last().unwrap_or(key);
            Some(ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<6}", label),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(format!(" {}", value)),
            ])))
        })
        .collect();

    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  (no data)",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
    } else {
        let list = ratatui::widgets::List::new(items);
        frame.render_widget(list, inner);
    }
}

/// Draw a grid/table widget.
fn draw_grid(
    frame: &mut Frame,
    config: &WidgetConfig,
    data: &WidgetDataStore,
    area: Rect,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", config.label),
            border_style.add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 3 || inner.height < 1 {
        return;
    }

    // For grid, we expect keys to be in format "row.col" or similar
    // Group values by row prefix
    let mut rows: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();

    for key in &config.keys {
        let parts: Vec<&str> = key.split('.').collect();
        if parts.len() >= 2 {
            let row = parts[0].to_string();
            let col = parts[1..].join(".");
            if let Some(value) = data.get(key) {
                rows.entry(row).or_default().push((col, value.to_string()));
            }
        }
    }

    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  (no data)",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    // Build table lines
    let mut lines: Vec<Line> = Vec::new();
    for (_row_name, cols) in rows {
        let mut spans: Vec<Span> = Vec::new();
        for (col_name, value) in cols {
            spans.push(Span::styled(
                format!("{:<8}", col_name),
                Style::default().fg(Color::Cyan),
            ));
            spans.push(Span::raw(format!("{}  ", value)));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Color parsing
// ---------------------------------------------------------------------------

/// Parse a color name to a ratatui Color.
fn parse_color_name(name: &str) -> Option<Color> {
    match name.to_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark_gray" | "darkgray" => Some(Color::DarkGray),
        "light_red" => Some(Color::LightRed),
        "light_green" => Some(Color::LightGreen),
        "light_yellow" => Some(Color::LightYellow),
        "light_blue" => Some(Color::LightBlue),
        "light_magenta" => Some(Color::LightMagenta),
        "light_cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

// Import needed for ListItem
use ratatui::widgets::ListItem;