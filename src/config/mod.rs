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
// Data model
// ---------------------------------------------------------------------------

/// A MUD server entry stored in the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
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
            notes: None,
        }
    }
}

/// A character belonging to a server.
/// The actual password is stored in the OS keyring, never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub server_id: String,
    /// Optional human-readable reminder — never the actual password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hint: Option<String>,
}

impl Character {
    pub fn new(name: impl Into<String>, server_id: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            server_id: server_id.into(),
            password_hint: None,
        }
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

