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

## 🚢 Releasing

All release automation lives in `scripts/`.

### Scripts Overview

| Script | Purpose |
|---|---|
| `scripts/release.sh` | Full pipeline: tests → crates.io → GitHub → AUR |
| `scripts/release-github.sh` | Build assets and create/update a GitHub release |
| `scripts/build-appimage.sh` | Build a Linux AppImage (auto-downloads `linuxdeploy`) |
| `scripts/deploy-aur.sh` | Update and push `durthang` + `durthang-bin` PKGBUILDs to AUR |

### Prerequisites

| Tool | Purpose | Install |
|---|---|---|
| `gh` | GitHub CLI — creates releases and uploads assets | [cli.github.com](https://cli.github.com/) → `gh auth login` |
| `gcc-mingw-w64-x86-64` | Windows cross-compile linker | `sudo apt install gcc-mingw-w64-x86-64` / `sudo pacman -S mingw-w64-gcc` |
| `zip` | Windows zip packaging | `sudo apt install zip` / `sudo pacman -S zip` |
| `makepkg` | Generates `.SRCINFO` for AUR (optional) | Arch Linux or an Arch-based container |
| AUR SSH key | Authenticates pushes to AUR | Add public key at [aur.archlinux.org/account](https://aur.archlinux.org/account/) |

> `linuxdeploy` (AppImage builder) is downloaded automatically into
> `target/appimage-build/` on first use — no manual install needed.

**Recommended SSH config** for AUR (`~/.ssh/config`):

```
Host aur.archlinux.org
    IdentityFile ~/.ssh/id_ed25519_aur
    User aur
```

**Optional — AppImage icon:**
Place a `docs/durthang.png` (256 × 256 px or larger) in the repo. If absent,
a placeholder is generated via ImageMagick (`convert`), or a 1×1 fallback is
used so the build does not abort.

### AUR one-time setup

Before the first `--push` to AUR, clone both package repos:

```bash
mkdir -p aur/durthang
git clone ssh://aur@aur.archlinux.org/durthang.git aur/durthang/aur-repo

mkdir -p aur/durthang-bin
git clone ssh://aur@aur.archlinux.org/durthang-bin.git aur/durthang-bin/aur-repo
```

### Running a release

```bash
# Full release: crates.io + GitHub + AppImage + Windows zip + AUR
./scripts/release.sh

# Draft GitHub release (safe to iterate on)
./scripts/release.sh --draft

# Skip individual steps
./scripts/release.sh --skip-crates        # already published to crates.io
./scripts/release.sh --skip-github        # skip GitHub release
./scripts/release.sh --skip-aur           # skip AUR push
```

All build artefacts land in `target/dist/` (git-ignored):

```
target/dist/
  durthang-<ver>-x86_64.tar.gz           ← Linux tarball (GitHub + AUR source)
  durthang-<ver>-x86_64.AppImage         ← Linux AppImage
  durthang-<ver>-x86_64-windows.zip      ← Windows binary zip
```

### AUR-only update

To push only a version bump to AUR without a full release:

```bash
# Dry-run (no push) — inspect the changes first
./scripts/deploy-aur.sh

# Push both packages
./scripts/deploy-aur.sh --push

# Push only one package
./scripts/deploy-aur.sh --push --package durthang-bin
```

The script reads the version from `Cargo.toml`, downloads the relevant source
or binary to compute `sha256sums`, updates both PKGBUILDs, regenerates
`.SRCINFO` (via `makepkg --printsrcinfo`), and pushes to the AUR.

---

## �📜 License

GNU General Public License v3.0 — see [LICENSE](LICENSE) for the full text.

---

*"Even in Mordor, there were those who endured." 🔥*

