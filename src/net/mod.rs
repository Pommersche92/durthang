//! Network layer — async TCP/Telnet connection to a MUD server.
//!
//! Architecture:
//!   - `Connection::spawn()` launches a Tokio task that owns the TCP socket.
//!   - The task sends decoded text lines to the UI via an `mpsc` channel
//!     (`net_tx` / `net_rx`).
//!   - The UI sends raw user input back to the task via a second `mpsc` channel
//!     (`input_tx` / `input_rx`).
//!   - Telnet IAC bytes are stripped / negotiated before forwarding to the UI.
//!   - NAWS: terminal size is sent on connect and whenever the UI notifies a
//!     resize via `Connection::send_naws()`.
//!   - The task gracefully shuts down when either the TCP stream closes or the
//!     UI drops its end of the channel.

use std::time::Duration;

use bytes::BytesMut;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::timeout,
};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Telnet constants
// ---------------------------------------------------------------------------

const IAC: u8 = 0xFF;
const WILL: u8 = 0xFB;
const WONT: u8 = 0xFC;
const DO: u8 = 0xFD;
const DONT: u8 = 0xFE;
const SB: u8 = 0xFA;
const SE: u8 = 0xF0;

// Telnet option codes
const OPT_ECHO: u8 = 0x01;
const OPT_NAWS: u8 = 0x1F;
const OPT_GMCP: u8 = 0xC9;

/// Connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// Messages from the network task to the UI.
#[derive(Debug)]
pub enum NetEvent {
    /// A decoded line of text from the server (may contain ANSI escape codes).
    Line(String),
    /// A partial line that ended without `\n` (e.g. a prompt).
    Prompt(String),
    /// The connection was established.
    Connected,
    /// The connection was lost or refused.
    Disconnected(String),
    /// A latency sample in milliseconds (placeholder, filled in Phase 5).
    Latency(u64),
}

/// Messages from the UI to the network task.
#[derive(Debug)]
pub enum UiEvent {
    /// Send a line to the server (newline will be appended).
    SendLine(String),
    /// Update NAWS with the new terminal size.
    Resize { cols: u16, rows: u16 },
    /// Close the connection.
    Disconnect,
}

// ---------------------------------------------------------------------------
// Connection handle
// ---------------------------------------------------------------------------

/// Handle returned to the UI after spawning the network task.
pub struct Connection {
    /// Receive net events (lines, connect/disconnect notification, …).
    pub rx: mpsc::Receiver<NetEvent>,
    /// Send user input and control messages to the network task.
    pub tx: mpsc::Sender<UiEvent>,
}

impl Connection {
    /// Spawn the network task and return a `Connection` handle.
    ///
    /// The task connects to `host:port`, negotiates Telnet, and bridges
    /// reads/writes via the two channels.
    pub fn spawn(host: String, port: u16, initial_size: (u16, u16)) -> Self {
        let (net_tx, net_rx) = mpsc::channel::<NetEvent>(256);
        let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(64);

        tokio::spawn(async move {
            run_connection(host, port, initial_size, net_tx, ui_rx).await;
        });

        Connection { rx: net_rx, tx: ui_tx }
    }

    /// Convenience: send a `Resize` event.
    pub async fn send_naws(&self, cols: u16, rows: u16) {
        let _ = self.tx.send(UiEvent::Resize { cols, rows }).await;
    }

    /// Convenience: send a line of user input.
    pub async fn send_line(&self, line: String) {
        let _ = self.tx.send(UiEvent::SendLine(line)).await;
    }

    /// Convenience: request a graceful disconnect.
    pub async fn disconnect(&self) {
        let _ = self.tx.send(UiEvent::Disconnect).await;
    }
}

// ---------------------------------------------------------------------------
// Telnet option negotiation
// ---------------------------------------------------------------------------

/// Build a WILL / WONT / DO / DONT response for a single option byte.
fn refuse(verb: u8) -> u8 {
    match verb {
        DO => WONT,
        WILL => DONT,
        _ => WONT,
    }
}

/// Build a 3-byte IAC negotiation response.
fn iac_response(verb: u8, opt: u8) -> [u8; 3] {
    [IAC, verb, opt]
}

/// Build a NAWS sub-negotiation packet for the given terminal size.
fn naws_packet(cols: u16, rows: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9);
    buf.extend_from_slice(&[IAC, SB, OPT_NAWS]);
    // NAWS values must have 0xFF doubled if they appear in the data.
    for byte in cols.to_be_bytes().iter().chain(rows.to_be_bytes().iter()) {
        if *byte == IAC {
            buf.push(IAC);
        }
        buf.push(*byte);
    }
    buf.extend_from_slice(&[IAC, SE]);
    buf
}

// ---------------------------------------------------------------------------
// Telnet stream parser
// ---------------------------------------------------------------------------

/// Parse `buf` in-place:
/// - Strips IAC sequences and negotiates options by appending responses to
///   `responses`.
/// - Returns the printable bytes as a `String` (may be shorter than `buf`).
fn parse_telnet(buf: &[u8], responses: &mut Vec<u8>) -> String {
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != IAC {
            out.push(buf[i]);
            i += 1;
            continue;
        }
        // IAC
        i += 1;
        if i >= buf.len() {
            break;
        }
        match buf[i] {
            IAC => {
                // Escaped 0xFF literal
                out.push(IAC);
                i += 1;
            }
            SB => {
                // Sub-negotiation: skip until IAC SE
                i += 1;
                while i + 1 < buf.len() {
                    if buf[i] == IAC && buf[i + 1] == SE {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            WILL | WONT | DO | DONT => {
                let verb = buf[i];
                i += 1;
                if i >= buf.len() {
                    break;
                }
                let opt = buf[i];
                i += 1;
                match (verb, opt) {
                    // Accept DO NAWS — we WILL send NAWS.
                    (DO, OPT_NAWS) => {
                        responses.extend_from_slice(&iac_response(WILL, OPT_NAWS));
                        debug!("Telnet: accepted DO NAWS");
                    }
                    // Accept WILL ECHO — server echoes back what we send.
                    (WILL, OPT_ECHO) => {
                        responses.extend_from_slice(&iac_response(DO, OPT_ECHO));
                        debug!("Telnet: accepted WILL ECHO");
                    }
                    // Accept WILL GMCP — acknowledge with DO GMCP.
                    (WILL, OPT_GMCP) => {
                        responses.extend_from_slice(&iac_response(DO, OPT_GMCP));
                        debug!("Telnet: accepted WILL GMCP");
                    }
                    // Refuse everything else.
                    _ => {
                        responses.extend_from_slice(&iac_response(refuse(verb), opt));
                        debug!("Telnet: refused verb={verb:#x} opt={opt:#x}");
                    }
                }
            }
            _ => {
                // Unknown command byte — skip.
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Connection task
// ---------------------------------------------------------------------------

async fn run_connection(
    host: String,
    port: u16,
    initial_size: (u16, u16),
    tx: mpsc::Sender<NetEvent>,
    mut ui_rx: mpsc::Receiver<UiEvent>,
) {
    let addr = format!("{host}:{port}");
    info!("Connecting to {addr}");

    let stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            error!("TCP connect failed: {e}");
            let _ = tx.send(NetEvent::Disconnected(e.to_string())).await;
            return;
        }
        Err(_) => {
            error!("TCP connect timed out");
            let _ = tx.send(NetEvent::Disconnected("Connection timed out".into())).await;
            return;
        }
    };

    info!("Connected to {addr}");
    let _ = tx.send(NetEvent::Connected).await;

    let (mut reader, mut writer) = stream.into_split();

    // Send initial NAWS.
    let naws = naws_packet(initial_size.0, initial_size.1);
    if let Err(e) = writer.write_all(&naws).await {
        warn!("Failed to send initial NAWS: {e}");
    }

    let mut read_buf = BytesMut::with_capacity(4096);
    let mut line_buf = String::new();

    loop {
        // We need to either receive data from the server or a command from the UI.
        tokio::select! {
            // Server → UI
            result = reader.read_buf(&mut read_buf) => {
                match result {
                    Ok(0) => {
                        info!("Server closed connection");
                        let _ = tx.send(NetEvent::Disconnected("Server closed the connection".into())).await;
                        break;
                    }
                    Ok(_) => {
                        let mut responses = Vec::new();
                        let raw = read_buf.split().freeze();
                        let text = parse_telnet(&raw, &mut responses);

                        // Send negotiation responses immediately.
                        if !responses.is_empty() {
                            if let Err(e) = writer.write_all(&responses).await {
                                warn!("Failed to write telnet responses: {e}");
                            }
                        }

                        // Split into lines; partial trailing content → Prompt.
                        line_buf.push_str(&text);
                        while let Some(pos) = line_buf.find('\n') {
                            let line: String = line_buf.drain(..=pos).collect();
                            let line = line.trim_end_matches('\n').trim_end_matches('\r').to_string();
                            let _ = tx.send(NetEvent::Line(line)).await;
                        }
                        // Whatever remains is a prompt (no newline yet).
                        if !line_buf.is_empty() {
                            let _ = tx.send(NetEvent::Prompt(line_buf.clone())).await;
                            line_buf.clear();
                        }
                    }
                    Err(e) => {
                        error!("Read error: {e}");
                        let _ = tx.send(NetEvent::Disconnected(e.to_string())).await;
                        break;
                    }
                }
            }

            // UI → Server
            msg = ui_rx.recv() => {
                match msg {
                    None | Some(UiEvent::Disconnect) => {
                        info!("Disconnecting on UI request");
                        let _ = tx.send(NetEvent::Disconnected("Disconnected".into())).await;
                        break;
                    }
                    Some(UiEvent::SendLine(line)) => {
                        let mut data = line.into_bytes();
                        data.extend_from_slice(b"\r\n");
                        if let Err(e) = writer.write_all(&data).await {
                            error!("Write error: {e}");
                            let _ = tx.send(NetEvent::Disconnected(e.to_string())).await;
                            break;
                        }
                    }
                    Some(UiEvent::Resize { cols, rows }) => {
                        let naws = naws_packet(cols, rows);
                        if let Err(e) = writer.write_all(&naws).await {
                            warn!("Failed to send NAWS on resize: {e}");
                        }
                    }
                }
            }
        }
    }
}
