# Durthang

> *"Durthang (Sauron's castle) loomed dark and tall before them."*
> — J.R.R. Tolkien, *The Return of the King*

**Durthang** is a modern, terminal-based MUD client written in Rust, named after thes fortress in Mordor. Like its namesake, it is built to endure — fast, lean, and uncompromising.

## Features (planned)

- **Server & character management** — tree-view selection screen on startup; add, edit, and delete servers and characters without leaving the terminal
- **Secure credential storage** — passwords stored in the OS keyring or an encrypted vault; never in plain text
- **Full ANSI/VT100 colour support** — renders the rich text output of classic and modern MUD servers faithfully
- **Telnet protocol** — proper IAC negotiation including ECHO, NAWS, and GMCP stubs
- **Scrollback buffer** — configurable history so you never miss a line
- **Input history** — recall previous commands with the arrow keys, just like a shell
- **Alias & trigger system** — automate repetitive input and highlight important events
- **Automap** — optional real-time ASCII/Unicode map built from GMCP `Room.Info` data or heuristic output parsing
- **Lightweight** — a single static binary with no runtime dependencies

## Status

Early development. See [TODO.md](TODO.md) for the full roadmap.

## Getting Started

### Prerequisites

- Rust toolchain (stable, 1.80+): <https://rustup.rs>

### Build & run

```bash
git clone https://github.com/yourname/durthang.git
cd durthang
cargo run
```

### Usage

On first launch you will be greeted by the server/character selection screen. Use the arrow keys to navigate, **Enter** to connect, **n** to add a new entry, **e** to edit, and **d** to delete. Press **q** or **Ctrl-C** to quit.

Once connected, you play the game in the main view. Press **?** for a full list of key bindings.

## Configuration

Configuration is stored at `~/.config/durthang/config.toml` (XDG base dir respected). A commented example file will be generated on first run.

## Project Structure (planned)

```
src/
  main.rs          entry point, app loop
  app.rs           top-level state machine
  ui/              all ratatui widgets and screens
  net/             async telnet / TCP connection layer
  config/          serde config and credential management
  map/             automap data model and renderer
```

## Contributing

The project is in its infancy. Issues and pull requests are welcome once the initial structure is in place.

## License

MIT — see `LICENSE` for details.

---

*"Even in Mordor, there were those who endured."*
