# Durthang — MUD Client Roadmap

## Phase 1: Project Foundation
- [x] Define module structure (`ui/`, `net/`, `config/`, `map/`)
- [x] Choose and integrate a persistent config format (TOML via `serde` + `toml`)
- [x] Decide on secure credential storage strategy (`keyring` crate — secrets stored in OS keyring, never in TOML)
- [x] Set up basic application state machine (`AppState` enum: `ServerSelect`, `Game`, …)
- [x] Add logging (`tracing` + `tracing-subscriber` writing to `~/.local/share/durthang/durthang.log`)

## Phase 2: Configuration & Data Model
- [x] Define data model for servers (`Server`: name, host, port, notes)
- [x] Define data model for characters (`Character`: name, server_id, optional password hint)
- [x] Implement save/load of config file (`~/.config/durthang/config.toml` or XDG)
- [x] Implement secure password storage (OS keyring via `keyring` crate — `store_password` / `get_password` / `delete_password`)
- [x] CLI argument parsing for `--config` path override (`clap`)

## Phase 3: Server/Character Selection UI
- [x] Implement a two-panel tree-view widget
  - [x] Top panel: list of servers (expandable/collapsible via Space)
  - [x] Bottom panel: characters belonging to the selected server
- [x] Keyboard navigation (arrow keys, Enter to connect, `n` to add new, `d` to delete, `e` to edit)
- [x] Add/edit server dialog (name, host, port)
- [x] Add/edit character dialog (name, password input with masking, password hint)
- [x] Confirmation dialog for delete actions
- [x] Status bar showing key hints

## Phase 4: Network Layer
- [x] Async runtime integration (`tokio` via `Runtime::block_on`)
- [x] Raw TCP connection to MUD server
- [x] Telnet protocol handling (IAC negotiation — ECHO, NAWS, GMCP; all others refused)
- [x] NAWS: send terminal size on connect and on resize (`Event::Resize`)
- [x] Non-blocking read/write using channels between network task and UI (`mpsc`)
- [x] Graceful disconnect and reconnect logic (press `q` in game view returns to selection; server-close detected)
- [x] Connection timeout and error reporting (15 s timeout, `NetEvent::Disconnected` surfaced)

## Phase 5: Game View UI
- [x] Split-pane layout: scrollable output area + input line
- [x] ANSI/VT100 colour code rendering via ratatui `Line`/`Span` with styles
- [x] Scrollback buffer (configurable size, e.g. 5 000 lines)
- [x] Input history (up/down arrow recall, like a shell)
- [x] Word-wrap of output lines respecting terminal width
- [x] Status bar: server name, character, connection state, latency
- [x] Resize handling (redraw on `SIGWINCH`)

## Phase 6: Quality-of-Life Features
- [ ] Alias system (map short commands to full strings, stored per character)
- [ ] Trigger system (regex → action, e.g. highlight keywords or auto-respond)
- [ ] `/connect`, `/disconnect`, `/quit` meta-commands in input line
- [ ] Copy mode (scroll through output and copy text to clipboard)
- [ ] Mouse support (optional, scroll wheel for scrollback)

## Phase 7: Sidebars
- [ ] Implement sidebar panel system with visibility toggle (show/hide per panel)
  - [ ] Character sheet panel (stats, vitals populated via GMCP or triggers)
  - [ ] Paperdoll panel (equipped items, equip/unequip keybinds when panel is focused)
  - [ ] Inventory panel (item list, equip/unequip/drop keybinds when panel is focused)
  - [ ] Automap panel (live map widget, see Phase 8)
- [ ] F-Key focus system: each sidebar panel is bound to an F-Key (F2–F5 --> F1 is Game input/output); pressing the key activates the panel and switches the input context to its local keybind set
- [ ] Per-character layout configuration (which panels are visible, their order and width) stored persistently in config
- [ ] Options screen (per character) for configuring the sidebar layout

## Phase 8: Automap
- [ ] Define internal map data model (`Room`: id, name, exits `HashMap<Direction, RoomId>`, coordinates)
- [ ] GMCP `Room.Info` parser to auto-create rooms on arrival
- [ ] Fallback: heuristic room detection from output text (regex on "Exits:" lines)
- [ ] Map rendering widget (ASCII/Unicode grid, rendered in a sidebar or overlay)
- [ ] Manual room linking / position override
- [ ] Save/load map per server to a file (`~/.local/share/durthang/<server>.map.json`)
- [ ] Map export to image or plain-text grid (stretch goal)

## Phase 9: Polish & Distribution
- [ ] Full keyboard shortcut help screen (`?`)
- [ ] Theming / colour scheme config
- [ ] Man page / `--help` output
- [ ] Unit tests for net layer, config serialization, map logic
- [ ] Integration test with a local mock MUD server
- [ ] CI pipeline (GitHub Actions: `cargo test`, `cargo clippy`, `cargo fmt --check`)
- [ ] Release builds + AppImage / `.deb` packaging

---

## Dependency Shortlist

| Crate | Purpose |
|---|---|
| `ratatui` | TUI framework |
| `crossterm` | Terminal backend |
| `tokio` | Async runtime |
| `serde` + `toml` | Config serialization |
| `keyring` | OS secure credential storage |
| `clap` | CLI argument parsing |
| `tracing` + `tracing-subscriber` | Structured logging |
| `regex` | Triggers / room detection |
| `unicode-width` | Correct display-width of characters |
