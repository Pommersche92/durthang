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
//!   - TLS: when `tls = true` the plain TCP stream is wrapped with rustls after
//!     the TCP handshake completes. System root certificates are loaded via
//!     `rustls-native-certs`.
//!   - Auto-login: when `auto_login = Some((login, password))` is given the
//!     task sends the login on the first server prompt and the password on the
//!     second prompt.
//!   - The task gracefully shuts down when either the TCP stream closes or the
//!     UI drops its end of the channel.

use std::{sync::Arc, time::{Duration, Instant}};

use bytes::BytesMut;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::{interval, timeout, MissedTickBehavior},
};
use tokio_rustls::{
    rustls::{self, ClientConfig, RootCertStore},
    TlsConnector,
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
const AYT: u8 = 0xF6;

// Telnet option codes
const OPT_ECHO: u8 = 0x01;
const OPT_NAWS: u8 = 0x1F;
const OPT_GMCP: u8 = 0xC9;

/// Connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Periodic best-effort latency probe interval.
const LATENCY_PROBE_INTERVAL: Duration = Duration::from_secs(30);
/// Maximum age for a user-command latency sample.
const USER_LATENCY_MAX_AGE: Duration = Duration::from_secs(10);
/// Maximum age for a probe latency sample.
const PROBE_LATENCY_MAX_AGE: Duration = Duration::from_secs(3);

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum LatencySource {
    UserCommand,
    Probe,
}

#[derive(Copy, Clone, Debug)]
struct PendingLatency {
    started: Instant,
    source: LatencySource,
}

impl PendingLatency {
    fn new(source: LatencySource) -> Self {
        Self {
            started: Instant::now(),
            source,
        }
    }

    fn max_age(self) -> Duration {
        match self.source {
            LatencySource::UserCommand => USER_LATENCY_MAX_AGE,
            LatencySource::Probe => PROBE_LATENCY_MAX_AGE,
        }
    }

    fn is_stale(self) -> bool {
        self.started.elapsed() > self.max_age()
    }
}

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
    /// A raw GMCP message payload (for example: `Room.Info {...}`).
    Gmcp(String),
    /// A latency sample in milliseconds.
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
    /// - `tls`: wrap the TCP stream with TLS (rustls + system root certs).
    /// - `auto_login`: when `Some((login, opt_password))`, the task automatically
    ///   sends `login` on the first server output and `password` (if `Some`) on
    ///   the first prompt that follows.
    pub fn spawn(
        host: String,
        port: u16,
        tls: bool,
        auto_login: Option<(String, Option<String>)>,
        initial_size: (u16, u16),
    ) -> Self {
        let (net_tx, net_rx) = mpsc::channel::<NetEvent>(256);
        let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>(64);

        tokio::spawn(async move {
            run_connection(host, port, tls, auto_login, initial_size, net_tx, ui_rx).await;
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

struct TelnetParseResult {
    text: String,
    gmcp: Vec<String>,
}

/// Parse `buf` in-place:
/// - Strips IAC sequences and negotiates options by appending responses to
///   `responses`.
/// - Returns printable text and extracted GMCP payloads.
fn parse_telnet(buf: &[u8], responses: &mut Vec<u8>) -> TelnetParseResult {
    let mut out = Vec::with_capacity(buf.len());
    let mut gmcp = Vec::new();
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
                // Sub-negotiation: read until IAC SE.
                i += 1;
                if i >= buf.len() {
                    break;
                }
                let opt = buf[i];
                i += 1;
                let mut payload = Vec::new();
                while i < buf.len() {
                    if i + 1 < buf.len() && buf[i] == IAC && buf[i + 1] == IAC {
                        payload.push(IAC);
                        i += 2;
                        continue;
                    }
                    if i + 1 < buf.len() && buf[i] == IAC && buf[i + 1] == SE {
                        i += 2;
                        break;
                    }
                    payload.push(buf[i]);
                    i += 1;
                }
                if opt == OPT_GMCP {
                    let msg = String::from_utf8_lossy(&payload).trim().to_string();
                    if !msg.is_empty() {
                        gmcp.push(msg);
                    }
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
    TelnetParseResult {
        text: String::from_utf8_lossy(&out).into_owned(),
        gmcp,
    }
}

// ---------------------------------------------------------------------------
// TLS helper
// ---------------------------------------------------------------------------

/// Perform a TLS handshake over an existing TCP stream.
async fn connect_tls(
    stream: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Box<dyn std::error::Error + Send + Sync>> {
    let mut root_store = RootCertStore::empty();
    let certs = rustls_native_certs::load_native_certs();
    for cert in certs.certs {
        // Ignore errors from individual untrusted/malformed system certs.
        let _ = root_store.add(cert);
    }
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let domain = rustls::pki_types::ServerName::try_from(host.to_string())?;
    Ok(connector.connect(domain, stream).await?)
}

// ---------------------------------------------------------------------------
// Connection task
// ---------------------------------------------------------------------------

async fn run_connection(
    host: String,
    port: u16,
    tls: bool,
    auto_login: Option<(String, Option<String>)>,
    initial_size: (u16, u16),
    tx: mpsc::Sender<NetEvent>,
    ui_rx: mpsc::Receiver<UiEvent>,
) {
    let addr = format!("{host}:{port}");
    info!("Connecting to {addr} (tls={tls})");

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

    info!("TCP connected to {addr}");

    if tls {
        match connect_tls(stream, &host).await {
            Ok(tls_stream) => {
                info!("TLS handshake successful for {host}");
                let (r, w) = tokio::io::split(tls_stream);
                connection_loop(
                    Box::new(r) as _,
                    Box::new(w) as _,
                    initial_size,
                    tx,
                    ui_rx,
                    auto_login,
                )
                .await;
            }
            Err(e) => {
                error!("TLS handshake failed: {e}");
                let _ = tx.send(NetEvent::Disconnected(format!("TLS error: {e}"))).await;
            }
        }
    } else {
        let (r, w) = stream.into_split();
        connection_loop(
            Box::new(r) as _,
            Box::new(w) as _,
            initial_size,
            tx,
            ui_rx,
            auto_login,
        )
        .await;
    }
}

/// Check whether the escape sequence starting at `seq[0]` (which must be ESC)
/// is fully terminated within `seq`.
fn is_complete_escape(seq: &[u8]) -> bool {
    if seq.len() < 2 || seq[0] != 0x1b {
        return false;
    }
    match seq[1] {
        b'[' => {
            // CSI: terminated by a byte in 0x40..=0x7E
            seq[2..].iter().any(|&b| (0x40..=0x7E).contains(&b))
        }
        b']' | b'P' | b'^' | b'_' | b'X' => {
            // OSC / DCS / PM / APC / SOS: terminated by BEL or ST (ESC \)
            seq[2..].iter().any(|&b| b == 0x07)
                || seq[2..].windows(2).any(|w| w == [0x1b, b'\\'])
        }
        b'(' | b')' | b'*' | b'+' => {
            // Charset designation: ESC + designator + one charset byte = 3 bytes
            seq.len() >= 3
        }
        _ => true, // Fe / single-byte: ESC + one byte is always complete
    }
}

/// Find the byte offset up to which `buf` can safely be sent as prompt text
/// without splitting an in-progress ANSI escape sequence.
/// Any trailing incomplete sequence is excluded from the returned range.
fn safe_prompt_end(buf: &[u8]) -> usize {
    let len = buf.len();
    if len == 0 {
        return 0;
    }
    // Scan backwards (up to 32 bytes) for the last ESC byte.
    let mut j = len;
    while j > 0 {
        j -= 1;
        if buf[j] == 0x1b {
            if is_complete_escape(&buf[j..]) {
                return len; // Last ESC starts a complete sequence → all safe.
            } else {
                return j; // Incomplete → cut before this ESC.
            }
        }
        if len - j > 32 {
            break;
        }
    }
    len // No ESC found in trailing region → all safe.
}

/// Core read/write loop — shared between plain-TCP and TLS connections.
///
/// Auto-login:
///   Step 0 → fires on the FIRST server output (any line or prompt) → sends login.
///   Step 1 → fires on the next PROMPT (partial line, no \n) → sends password if stored.
/// Using "first output" for login covers both MUDs that send a prompt without \n
/// and those that send the login line with \n.
async fn connection_loop(
    mut reader: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    mut writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    initial_size: (u16, u16),
    tx: mpsc::Sender<NetEvent>,
    mut ui_rx: mpsc::Receiver<UiEvent>,
    auto_login: Option<(String, Option<String>)>,
) {
    let _ = tx.send(NetEvent::Connected).await;

    // Send initial NAWS.
    let naws = naws_packet(initial_size.0, initial_size.1);
    if let Err(e) = writer.write_all(&naws).await {
        warn!("Failed to send initial NAWS: {e}");
    }

    let mut read_buf = BytesMut::with_capacity(4096);
    let mut line_buf = String::new();
    // 0 = send login on first server output (line or prompt)
    // 1 = send password on next PROMPT
    // 2 = done
    let mut auto_login_step: u8 = if auto_login.is_some() { 0 } else { 2 };
    // Approximate round-trip latency: timestamp an outstanding user command
    // (or periodic probe) and sample when the next server output arrives.
    // Stale timestamps are dropped so unrelated output does not create spikes.
    let mut pending_latency: Option<PendingLatency> = None;
    let mut latency_probe = interval(LATENCY_PROBE_INTERVAL);
    latency_probe.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
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
                        let parsed = parse_telnet(&raw, &mut responses);

                        for gmcp in parsed.gmcp {
                            let _ = tx.send(NetEvent::Gmcp(gmcp)).await;
                        }

                        // Send negotiation responses immediately.
                        if !responses.is_empty() {
                            if let Err(e) = writer.write_all(&responses).await {
                                warn!("Failed to write telnet responses: {e}");
                            }
                        }

                        // Split into lines; partial trailing content → Prompt.
                        line_buf.push_str(&parsed.text);
                        let mut had_complete_lines = false;
                        while let Some(pos) = line_buf.find('\n') {
                            had_complete_lines = true;
                            let line: String = line_buf.drain(..=pos).collect();
                            let line = line.trim_end_matches('\n').trim_end_matches('\r').to_string();
                            let _ = tx.send(NetEvent::Line(line)).await;
                        }
                        let had_prompt = !line_buf.is_empty();

                        // Auto-login state machine.
                        // Step 0: fire on ANY first server output (line or prompt).
                        if auto_login_step == 0 && (had_complete_lines || had_prompt) {
                            if let Some((ref login, _)) = auto_login {
                                info!("Auto-login: sending login name");
                                auto_login_step = 1;
                                let mut data = login.as_bytes().to_vec();
                                data.extend_from_slice(b"\r\n");
                                if let Err(e) = writer.write_all(&data).await {
                                    error!("Auto-login write error: {e}");
                                    let _ = tx.send(NetEvent::Disconnected(e.to_string())).await;
                                    break;
                                }
                            }
                        // Step 1: fire only on a PROMPT – wait for the actual password prompt.
                        } else if auto_login_step == 1 && had_prompt {
                            if let Some((_, Some(ref password))) = auto_login {
                                info!("Auto-login: sending password");
                                auto_login_step = 2;
                                let mut data = password.as_bytes().to_vec();
                                data.extend_from_slice(b"\r\n");
                                if let Err(e) = writer.write_all(&data).await {
                                    error!("Auto-login password write error: {e}");
                                    let _ = tx.send(NetEvent::Disconnected(e.to_string())).await;
                                    break;
                                }
                            } else {
                                // No stored password – user will type it manually.
                                auto_login_step = 2;
                            }
                        }

                        if had_prompt {
                            // Don't send a prompt if `line_buf` ends with an
                            // incomplete ANSI escape sequence — keep the fragment
                            // for the next read so it can be reassembled.
                            let safe = safe_prompt_end(line_buf.as_bytes());
                            if safe > 0 {
                                let prompt_text: String = line_buf[..safe].chars()
                                    .filter(|&c| c != '\r')
                                    .collect();
                                let _ = tx.send(NetEvent::Prompt(prompt_text)).await;
                            }
                            // Keep the incomplete tail (if any) for the next read.
                            let tail = line_buf[safe..].to_string();
                            line_buf.clear();
                            line_buf.push_str(&tail);
                        }

                        if had_complete_lines || had_prompt {
                            if let Some(pending) = pending_latency.take() {
                                if !pending.is_stale() {
                                    let elapsed = pending.started.elapsed().as_millis();
                                    let latency_ms = u64::try_from(elapsed).unwrap_or(u64::MAX);
                                    let _ = tx.send(NetEvent::Latency(latency_ms)).await;
                                }
                            }
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
                        if !matches!(pending_latency, Some(PendingLatency { source: LatencySource::UserCommand, .. })) {
                            // A real user command takes precedence over any probe sample.
                            pending_latency = Some(PendingLatency::new(LatencySource::UserCommand));
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

            _ = latency_probe.tick() => {
                if pending_latency.map(|p| p.is_stale()).unwrap_or(false) {
                    pending_latency = None;
                }
                if pending_latency.is_some() {
                    continue;
                }
                let probe = [IAC, AYT];
                if let Err(e) = writer.write_all(&probe).await {
                    warn!("Failed to send latency probe: {e}");
                } else {
                    pending_latency = Some(PendingLatency::new(LatencySource::Probe));
                }
            }
        }
    }
}
