# 🏰 Durthang

> *"Durthang (Sauron's castle) loomed dark and tall before them."*  
> — J.R.R. Tolkien, *The Return of the King*

**Durthang** is a modern, terminal-based MUD client written in Rust 🦀.
It runs entirely in your terminal, requires no graphical environment, and ships as a single
statically-linked binary with no extra runtime dependencies.

---

## ✨ Features

| Category | Details |
|---|---|
| 🔌 **Connection** | Plain TCP and TLS (system root certs); clean disconnect and reconnect |
| 📡 **Telnet** | IAC negotiation — ECHO, NAWS, GMCP; all unknown options refused |
| 🗺️ **GMCP** | Room.Info parsing for automap; extensible for other modules |
| 🎨 **ANSI / VT100** | Full 16- and 256-colour rendering via ratatui |
| 📜 **Scrollback buffer** | 5 000 lines; scroll with PgUp/PgDn or mouse wheel |
| ⌨️ **Input** | Shell-style history (↑/↓), Home/End/Left/Right cursor movement |
| ⚡ **Aliases** | Per-character short-command expansions, stored in config |
| 🔍 **Triggers** | Regex → highlight colour and/or auto-send, stored per character |
| 📋 **Copy mode** | Scroll and copy output text to clipboard via OSC 52 |
| 🗺️ **Automap** | Real-time ASCII map built from GMCP `Room.Info` or heuristic `Exits:` parsing; persisted to disk per server |
| 🪟 **Sidebar** | Right-side panel column with an **Automap** minimap and a **Notes** panel; toggleable, resizable, configurable per character |
| 📝 **Notes panel** | Create, edit, delete, and reorder personal free-text notes without leaving the game |
| 🔐 **Secure credentials** | Passwords stored in the OS keyring (Secret Service / macOS Keychain / Windows Credential Manager); never written to the config file |
| 💾 **Config persistence** | TOML config at `~/.config/durthang/config.toml` (XDG respected); sidebar layout and notes are saved automatically |
| 📶 **Latency display** | Rolling-average latency shown in the status bar |
| 🖱️ **Mouse support** | Scroll wheel for scrollback |
| 🪵 **Logging** | Structured logs to `~/.local/share/durthang/durthang.log` via `tracing` |

---

## 📦 Installation

### From crates.io

```bash
cargo install durthang
```

### From source

```bash
git clone https://github.com/Pommersche92/durthang.git
cd durthang
cargo build --release
# The binary is at target/release/durthang
```

**Minimum Rust version:** 1.85 (edition 2024)

### 🐧 Linux: password storage dependency

Durthang uses the OS keyring to store passwords securely.
On most Linux desktops this works out of the box via the Secret Service D-Bus API
(GNOME Keyring, KWallet, etc.).

On a **headless** or minimal system you need a running Secret Service daemon:

```bash
# Debian/Ubuntu — GNOME Keyring
sudo apt install gnome-keyring
eval $(gnome-keyring-daemon --start --components=secrets)

# or use the kwallet-based stack on KDE
```

Durthang will still start without a keyring, but password saving will fail with a
runtime error. You can work around this by leaving the password field blank and
using the server's own login prompt.

---

## 🚀 Quick Start

Launch Durthang:

```bash
durthang
```

An optional `--config` flag points to a custom config file:

```bash
durthang --config ~/my-muds.toml
```

You will land on the **server/character selection screen**.

### 🗂️ Selection screen keys

| Key | Action |
|---|---|
| `↑` / `↓` | Move cursor |
| `Space` / `←` / `→` | Expand or collapse a server |
| `Enter` | Connect with the selected character |
| `N` | Add a new server |
| `n` | Add a new character to the selected server |
| `e` | Edit the selected server or character |
| `d` | Delete the selected server or character (with confirmation) |
| `q` / `Ctrl+Q` | Quit |

---

## 🎮 Game Screen

Once connected, the game screen shows:

```
┌─ MUME ─────────────────────────────────────┬──────────────┐
│                                            │  Automap     │
│  [scrollable MUD output]                   │  . . .@. .   │
│                                            │  @ Rivendell │
│                                            ├──────────────┤
│                                            │  Notes       │
│                                            │  - buy food  │
│                                            │  - repair eq │
│                                            │              │
│                                            │              │
├────────────────────────────────────────────│              │
│ ▶ input line                              │              │
├─────────────────────────────── ↑42 ────────┴──────────────┤
│ MUME / Berejorn   lat 12ms   Ctrl+Q disconnect            │
└───────────────────────────────────────────────────────────┘
```

### ⌨️ Game screen keys

| Key | Action |
|---|---|
| `Enter` | Send the current input line |
| `↑` / `↓` | Input history |
| `PgUp` / `PgDn` | Scroll the output buffer |
| `Ctrl+End` | Jump back to the live view |
| `F3` | Toggle the right sidebar |
| `F4` | Cycle focus to the next sidebar panel |
| `F1` / `Esc` (in sidebar) | Return focus to the game input |
| `Ctrl+C` | Enter copy mode (scroll + copy a line) |
| `Ctrl+Q` | Disconnect and return to the selection screen |

### 💬 Meta-commands (type in the input line)

| Command | Effect |
|---|---|
| `/connect` | Reconnect to the current server |
| `/disconnect` | Disconnect and return to the selection screen |
| `/quit` | Exit Durthang |
| `/alias <name> <expansion>` | Add or update an alias |
| `/unalias <name>` | Remove an alias |
| `/trigger <regex> [color=<c>] [send=<cmd>]` | Add a trigger |
| `/untrigger <id-prefix>` | Remove a trigger |
| `/sidebar right` | Toggle the right sidebar |

---

## 🪟 Sidebar panels

The sidebar is toggled with **F3**. Focus cycles through panels with **F4**.
Press **`o`** while a panel is focused to open the **Sidebar Options** overlay,
where you can toggle panel visibility, reorder panels, and adjust the sidebar width.

### 🗺️ Automap panel

The automap builds a live ASCII grid as you move through the world:

- `@` marks your current room.
- `.` marks known adjacent rooms.
- The legend line shows the current room name and Z-level.

Map data is loaded from GMCP `Room.Info` when available, with a fallback heuristic
that reads `Exits:` lines from the server output.
Maps are saved automatically to `~/.local/share/durthang/<server-id>.map.json`.

### 📝 Notes panel

A personal scratchpad attached to each character.

| Key | Action |
|---|---|
| `n` / `a` | Add a new note |
| `e` / `Enter` | Edit the selected note inline |
| `d` / `Delete` | Delete the selected note |
| `K` | Move the selected note up |
| `J` | Move the selected note down |
| `Esc` | Cancel editing |

Notes are persisted in the character's sidebar config automatically on every change.

---

## ⚡ Aliases & Triggers

Aliases and triggers are stored per character in the config file.

**Alias example** — type `k orc` in game and it expands to `kill orc`:

```
/alias k kill
```

**Trigger example** — highlight lines containing "You are hungry" in yellow:

```
/trigger You are hungry color=yellow
```

**Trigger with auto-response** — auto-buy food when a shopkeeper says it's available:

```
/trigger "You see food on sale" send="buy bread"
```

---

## ⚙️ Configuration

Config file: `~/.config/durthang/config.toml`

```toml
[[servers]]
id = "…"            # auto-generated UUID
name = "MUME"
host = "mume.org"
port = 4242
tls  = true

[[characters]]
id        = "…"
name      = "Eomer"
server_id = "…"    # references servers[].id
login     = "xX_H0R538055_Xx"
password_hint = "mother's maiden name"   # reminder only, not the actual password
notes     = "Rohirrim, warrior"

[characters.sidebar]
right_visible = true
right_width   = 26

[[characters.sidebar.panels]]
kind     = "automap"
side     = "right"
height_pct = 35

[[characters.sidebar.panels]]
kind     = "notes"
side     = "right"
height_pct = 65

[[characters.aliases]]
name      = "k"
expansion = "kill"

[[characters.triggers]]
id      = "…"
pattern = "You are hungry"
color   = "yellow"
```

Passwords are **never** stored here 🔒; they live in the OS keyring under the key
`durthang/<server-id>/<character-name>`.

---

## 🗂️ Project Structure

```
src/
  main.rs        Entry point, CLI parsing, terminal setup, main event loop
  app.rs         Top-level state machine (ServerSelect ↔ Game)
  config/        Serde data model, TOML persistence, keyring helpers
  net/           Async TCP/TLS/Telnet connection task (tokio)
  ui/
    selection.rs Server/character selection tree-view
    game.rs      Game screen, key handling, alias/trigger evaluation
    sidebar.rs   Sidebar panel system (automap + notes)
  map/           Room data model, GMCP/heuristic parsing, JSON persistence
```

---

## 🤝 Contributing

Issues and pull requests are welcome!

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

---

## � Releasing

All release automation lives in `scripts/`.

### Prerequisites

| Tool | Purpose | Install |
|---|---|---|
| `gh` | GitHub CLI — creates releases and uploads assets | [cli.github.com](https://cli.github.com/) → `gh auth login` |
| `cross` | Docker-based cross-compiler (preferred) | `cargo install cross` |
| `rustup` targets | Needed if **not** using `cross` | `rustup target add x86_64-unknown-linux-musl x86_64-pc-windows-gnu` |
| `zip` | Windows zip packaging | `sudo pacman -S zip` / `sudo apt install zip` |
| `appimagetool` | Builds the Linux AppImage | [AppImageKit releases](https://github.com/AppImage/AppImageKit/releases) |
| `makepkg` | Generates `.SRCINFO` for AUR | Arch Linux or an Arch-based Docker container |
| AUR SSH key | Authenticates pushes to AUR | Add public key at [aur.archlinux.org/account](https://aur.archlinux.org/account/) |

**Recommended SSH config** for AUR (`~/.ssh/config`):

```
Host aur.archlinux.org
    IdentityFile ~/.ssh/id_ed25519_aur
    User aur
```

**Optional — AppImage icon:**  
Place a `docs/durthang.png` (512 × 512 px) in the repo root. If absent, a
placeholder is generated via ImageMagick (`convert`).

### Running a release

```bash
# Full release: GitHub + AppImage + Windows zip + AUR
bash scripts/release.sh

# Skip individual steps
bash scripts/release.sh --skip-appimage   # no AppImage
bash scripts/release.sh --skip-windows    # no Windows build
bash scripts/release.sh --skip-aur        # no AUR push
```

The script automatically reads the version from `Cargo.toml`, so bump it there
(and in the `CHANGELOG` / `git tag`) before running.

All build artefacts land in `dist/` (git-ignored):

```
dist/
  durthang-<ver>-x86_64-linux.tar.gz      ← used by AUR and GitHub release
  durthang-<ver>-x86_64-linux.AppImage
  durthang-<ver>-x86_64-windows.zip
  durthang-<ver>-sha256sums.txt
```

### AUR-only update

If you only need to bump the AUR package without creating a full GitHub release:

```bash
bash scripts/aur/update-aur.sh <VERSION> dist/durthang-<VERSION>-x86_64-linux.tar.gz
```

The script clones `ssh://aur@aur.archlinux.org/durthang-bin.git`, writes a
`PKGBUILD` with the correct `pkgver` and `sha256sums`, regenerates `.SRCINFO`,
and pushes. **First-time setup:** create the `durthang-bin` package via the
[AUR web interface](https://aur.archlinux.org/packages/) before the first push.

---

## �📜 License

GNU General Public License v3.0 — see [LICENSE](LICENSE) for the full text.

---

*"Even in Mordor, there were those who endured." 🔥*

