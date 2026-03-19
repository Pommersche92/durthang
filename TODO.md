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
- [ ] Implement a two-panel tree-view widget
  - [ ] Top panel: list of servers (expandable/collapsible)
  - [ ] Bottom panel: characters belonging to the selected server
- [ ] Keyboard navigation (arrow keys, Enter to connect, `n` to add new, `d` to delete, `e` to edit)
- [ ] Add/edit server dialog (name, host, port)
- [ ] Add/edit character dialog (name, password input with masking)
- [ ] Confirmation dialog for delete actions
- [ ] Status bar showing key hints

## Phase 4: Network Layer
- [ ] Async runtime integration (`tokio`)
- [ ] Raw TCP connection to MUD server
- [ ] Telnet protocol handling (IAC negotiation — at minimum ECHO, NAWS, GMCP stubs)
- [ ] NAWS: send terminal size on connect and on resize
- [ ] Non-blocking read/write using channels between network task and UI
- [ ] Graceful disconnect and reconnect logic
- [ ] Connection timeout and error reporting

## Phase 5: Game View UI
- [ ] Split-pane layout: scrollable output area + input line
- [ ] ANSI/VT100 colour code rendering via ratatui `Line`/`Span` with styles
- [ ] Scrollback buffer (configurable size, e.g. 5 000 lines)
- [ ] Input history (up/down arrow recall, like a shell)
- [ ] Word-wrap of output lines respecting terminal width
- [ ] Status bar: server name, character, connection state, latency
- [ ] Resize handling (redraw on `SIGWINCH`)

## Phase 6: Quality-of-Life Features
- [ ] Alias system (map short commands to full strings, stored per character)
- [ ] Trigger system (regex → action, e.g. highlight keywords or auto-respond)
- [ ] `/connect`, `/disconnect`, `/quit` meta-commands in input line
- [ ] Copy mode (scroll through output and copy text to clipboard)
- [ ] Mouse support (optional, scroll wheel for scrollback)

## Phase 7: Automap
- [ ] Define internal map data model (`Room`: id, name, exits `HashMap<Direction, RoomId>`, coordinates)
- [ ] GMCP `Room.Info` parser to auto-create rooms on arrival
- [ ] Fallback: heuristic room detection from output text (regex on "Exits:" lines)
- [ ] Map rendering widget (ASCII/Unicode grid, rendered in a sidebar or overlay)
- [ ] Manual room linking / position override
- [ ] Save/load map per server to a file (`~/.local/share/durthang/<server>.map.json`)
- [ ] Map export to image or plain-text grid (stretch goal)

## Phase 8: Polish & Distribution
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
