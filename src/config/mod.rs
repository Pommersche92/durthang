// Config module — data model, TOML persistence and OS-keyring credential helpers.
//
// Credential-storage decision: OS keyring via the `keyring` crate.
// Secrets are never written to the TOML file; the keyring entry key is
// "<service>/<server_id>/<character_name>" where service = "durthang".

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Aliases & Triggers
// ---------------------------------------------------------------------------

/// A command alias: replaces `name` (or a line starting with `name`) with
/// `expansion` before the line is sent to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alias {
    pub name: String,
    pub expansion: String,
}

/// A trigger: when an incoming line matches `pattern`, optionally re-colour
/// it and/or auto-send a command back to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub id: String,
    /// A regular expression matched against the raw (ANSI-stripped) line.
    pub pattern: String,
    /// Named colour applied as foreground when the line matches
    /// (e.g. `"red"`, `"yellow"`, `"cyan"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// If set, this command is automatically sent to the server when a match
    /// is found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<String>,
}

impl Trigger {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            pattern: pattern.into(),
            color: None,
            send: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Sidebar layout
// ---------------------------------------------------------------------------

/// Identifies one of the sidebar panels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    CharSheet,
    Paperdoll,
    Inventory,
    Automap,
}

impl PanelKind {
    pub fn label(&self) -> &'static str {
        match self {
            PanelKind::CharSheet => "Character Sheet",
            PanelKind::Paperdoll => "Paperdoll",
            PanelKind::Inventory => "Inventory",
            PanelKind::Automap   => "Automap",
        }
    }

    /// Short label used in the sidebar tab bar.
    pub fn short_label(&self) -> &'static str {
        match self {
            PanelKind::CharSheet => "Stats",
            PanelKind::Paperdoll => "Wear",
            PanelKind::Inventory => "Inv",
            PanelKind::Automap   => "Map",
        }
    }
}

/// Which sidebar column a panel is assigned to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSide {
    Left,
    Right,
}

/// Per-panel configuration: sidebar assignment and relative height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelConfig {
    pub kind: PanelKind,
    /// Which sidebar column.  `None` = not displayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<SidebarSide>,
    /// Relative height expressed as a percentage (1–100).
    /// Panels that share a sidebar column have their values normalised to fill it.
    #[serde(default = "default_panel_height_pct")]
    pub height_pct: u8,
}

fn default_panel_height_pct() -> u8  { 50 }
fn default_left_visible()    -> bool { true }
fn default_right_visible()   -> bool { true }
fn default_left_width()      -> u16  { 26 }
fn default_right_width()     -> u16  { 26 }

fn default_panels() -> Vec<PanelConfig> {
    vec![
        PanelConfig { kind: PanelKind::CharSheet, side: Some(SidebarSide::Left),  height_pct: 40  },
        PanelConfig { kind: PanelKind::Paperdoll, side: Some(SidebarSide::Left),  height_pct: 60  },
        PanelConfig { kind: PanelKind::Automap,   side: Some(SidebarSide::Right), height_pct: 35  },
        PanelConfig { kind: PanelKind::Inventory, side: Some(SidebarSide::Right), height_pct: 65  },
    ]
}

/// Per-character sidebar layout persisted in the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidebarLayout {
    /// Whether the left sidebar column is shown.
    #[serde(default = "default_left_visible")]
    pub left_visible: bool,
    /// Width of the left sidebar column in terminal characters.
    #[serde(default = "default_left_width")]
    pub left_width: u16,
    /// Whether the right sidebar column is shown.
    #[serde(default = "default_right_visible")]
    pub right_visible: bool,
    /// Width of the right sidebar column in terminal characters.
    #[serde(default = "default_right_width")]
    pub right_width: u16,
    /// Panel configurations (kind, side assignment, relative height).
    #[serde(default = "default_panels")]
    pub panels: Vec<PanelConfig>,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        Self {
            left_visible:  true,
            left_width:    26,
            right_visible: true,
            right_width:   26,
            panels: default_panels(),
        }
    }
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A MUD server entry stored in the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Whether to use TLS for this server connection.
    #[serde(default)]
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Server {
    pub fn new(name: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            host: host.into(),
            port,
            tls: false,
            notes: None,
        }
    }
}

/// A character belonging to a server.
/// The actual password is stored in the OS keyring, never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    /// Display name shown in the UI.
    pub name: String,
    pub server_id: String,
    /// The username typed at the MUD's login prompt.
    /// When absent the character's `name` is used instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    /// Optional human-readable reminder — never the actual password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hint: Option<String>,
    /// Free-form notes (e.g. race, class, level).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Command aliases stored per character.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Alias>,
    /// Trigger rules stored per character.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    /// Sidebar layout for this character.
    #[serde(default)]
    pub sidebar: SidebarLayout,
}

impl Character {
    pub fn new(name: impl Into<String>, server_id: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            server_id: server_id.into(),
            login: None,
            password_hint: None,
            notes: None,
            aliases: Vec::new(),
            triggers: Vec::new(),
            sidebar: SidebarLayout::default(),
        }
    }

    /// Returns the login name to use at the MUD prompt (falls back to `name`).
    pub fn effective_login(&self) -> &str {
        self.login.as_deref().unwrap_or(&self.name)
    }
}

// ---------------------------------------------------------------------------
// Root config object
// ---------------------------------------------------------------------------

/// Root configuration — serialised to / deserialised from TOML.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub servers: Vec<Server>,
    #[serde(default)]
    pub characters: Vec<Character>,
}

impl Config {
    /// Resolve the default config file path using XDG or
    /// fall back to `~/.config/durthang/config.toml`.
    pub fn default_path() -> PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").expect("HOME environment variable not set");
                PathBuf::from(home).join(".config")
            });
        base.join("durthang").join("config.toml")
    }

    /// Load config from `path`.
    /// Returns a default `Config` if the file does not exist yet.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
    }

    /// Persist the config to `path`, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    /// Look up a server by id.
    pub fn server_by_id(&self, id: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == id)
    }

    /// Return all characters that belong to a given server.
    pub fn characters_for_server(&self, server_id: &str) -> Vec<&Character> {
        self.characters
            .iter()
            .filter(|c| c.server_id == server_id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Keyring helpers
// ---------------------------------------------------------------------------

const KEYRING_SERVICE: &str = "durthang";

fn keyring_entry(server_id: &str, character_name: &str) -> keyring::Result<keyring::Entry> {
    let account = format!("{server_id}/{character_name}");
    keyring::Entry::new(KEYRING_SERVICE, &account)
}

/// Store a password for the given character in the OS keyring.
pub fn store_password(
    server_id: &str,
    character_name: &str,
    password: &str,
) -> keyring::Result<()> {
    keyring_entry(server_id, character_name)?.set_password(password)
}

/// Retrieve the stored password for a character from the OS keyring.
/// Returns `None` if no entry exists yet.
pub fn get_password(
    server_id: &str,
    character_name: &str,
) -> keyring::Result<Option<String>> {
    match keyring_entry(server_id, character_name)?.get_password() {
        Ok(pw) => Ok(Some(pw)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Remove the stored password for a character from the OS keyring.
pub fn delete_password(server_id: &str, character_name: &str) -> keyring::Result<()> {
    keyring_entry(server_id, character_name)?.delete_credential()
}

