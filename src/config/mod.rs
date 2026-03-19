// Config module — Phase 2 will provide full save/load of
// ~/.config/durthang/config.toml (XDG).
//
// Credential-storage decision: OS keyring via the `keyring` crate.
// Secrets are never written to the TOML file; the keyring entry key
// is "<service>/<username>" where service = "durthang/<server-id>".
