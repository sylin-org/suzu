//! The resident's loopback read API — the third door into the house
//! (the CLI and the control chirps were the first two).
//!
//! A minimal HTTP/1.1 responder on 127.0.0.1:7899 (S-U-Z-U + 1). The
//! workbench renders what these endpoints answer and invents nothing:
//! the fleet table, the moment journal, in-band shots, and the
//! maintenance sagas all come from the domains that own them. No
//! serial ever leaves the resident; a workbench that wants a face's
//! pixels asks for a PNG and gets one.
//!
//! CORS: `*` — the Tauri webview is a foreign origin to this socket,
//! and it is the only client that matters. The bind is loopback, so
//! the trust boundary is the machine itself (ADR-0002: local-first,
//! same machine as the faces).

use super::devices::{DevicesCmd, RecordState};
use super::moments::MomentsCmd;
use super::roster::Roster;
use anyhow::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

pub const API_PORT: u16 = 7899;

/// The moment journal — the Log page's memory. Bounded, in-memory,
/// honest: it dies with the process, like the pause flag.
pub struct Journal {
    lines: Mutex<VecDeque<JournalLine>>,
    tx: tokio::sync::broadcast::Sender<JournalLine>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JournalLine {
    pub ts: String,
    pub domain: String,
    pub text: String,
}

const JOURNAL_CAP: usize = 600;

impl Journal {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(256);
        Self { lines: Mutex::new(VecDeque::new()), tx }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<JournalLine> {
        self.tx.subscribe()
    }

    pub fn record(&self, domain: &str, text: &str) {
        let line = JournalLine {
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            domain: domain.to_string(),
            text: text.to_string(),
        };
        {
            let mut lines = self.lines.lock().expect("journal lock");
            lines.push_back(line.clone());
            while lines.len() > JOURNAL_CAP {
                lines.pop_front();
            }
        }
        let _ = self.tx.send(line);
    }

    pub fn tail(&self, limit: usize) -> Vec<JournalLine> {
        let lines = self.lines.lock().expect("journal lock");
        lines.iter().rev().take(limit).cloned().collect()
    }
}

/// The destinations the About page may reach — the closed vocabulary,
/// resolved here so no URL ever hardens into the workbench markup.
const DESTINATIONS: &[(&str, &str, &str, &str, &str)] = &[
    ("Ghostlight's sibling", "home", "Project page",
        "What suzu is, who it is for, and how it behaves.",
        "https://github.com/sylin-org/suzu"),
    ("Ghostlight's sibling", "contract", "The face contract",
        "What every face does, regardless of dialect.",
        "https://github.com/sylin-org/suzu/blob/dev/docs/the-face-contract.md"),
    ("Ghostlight's sibling", "adr_lake", "Why the matrix is a lake",
        "Raindrops, atom fireflies, and the rendering grammar.",
        "https://github.com/sylin-org/suzu/blob/dev/docs/adr/0001-the-lake.md"),
];

pub struct Ctx {
    pub catalog: Arc<crate::Catalog>,
    pub devices: mpsc::Sender<DevicesCmd>,
    pub moments: mpsc::Sender<MomentsCmd>,
    pub roster: Arc<std::sync::RwLock<Roster>>,
    pub journal: Arc<Journal>,
}

pub async fn listen(ctx: Arc<Ctx>) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", API_PORT)).await?;
    println!(
        "[api] listening on http://127.0.0.1:{API_PORT} — the workbench's door (status · log · shots · sagas)"
    );
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            let _ = serve_one(stream, ctx).await;
        });
    }
}

async fn serve_one(mut stream: TcpStream, ctx: Arc<Ctx>) -> Result<()> {
    let mut buf = Vec::new();
    // Read until end of headers, then content-length bytes of body.
    let header_end;
    loop {
        let mut chunk = [0u8; 1024];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_headers_end(&buf) {
            header_end = pos;
            break;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    // query strings are transport noise (?t= cache busters and friends)
    let path = parts
        .next()
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    let mut content_length = 0usize;
    for line in lines {
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0u8; 1024];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&body[..body.len().min(content_length.max(0))]).to_string();

    if path == "/api/events" && method == "GET" {
        return events_stream(ctx, stream).await;
    }

    let started = Instant::now();
    let (status, content_type, payload) = route(&ctx, &method, &path, &body).await;
    if path != "/api/shot" {
        // One honest access line per request — shots poll too fast to matter.
        ctx.journal.record("api", &format!("{method} {path} → {status} ({} ms)", started.elapsed().as_millis()));
    }
    write_response(&mut stream, status, &content_type, payload).await
}

/// The live wire: a replay of the recent past, then every new fact as
/// it lands. text/event-stream, one JSON JournalLine per event.
async fn events_stream(ctx: Arc<Ctx>, mut stream: TcpStream) -> Result<()> {
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n                cache-control: no-cache\r\naccess-control-allow-origin: *\r\n                connection: keep-alive\r\n\r\n";
    stream.write_all(head.as_bytes()).await?;
    for line in ctx.journal.tail(30) {
        let payload = serde_json::to_string(&line).unwrap_or_default();
        stream.write_all(format!("data: {payload}\n\n").as_bytes()).await?;
    }
    stream.flush().await?;
    let mut rx = ctx.journal.subscribe();
    loop {
        match rx.recv().await {
            Ok(line) => {
                let payload = serde_json::to_string(&line).unwrap_or_default();
                stream.write_all(format!("data: {payload}\n\n").as_bytes()).await?;
                stream.flush().await?;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    Ok(())
}

/// The keeper may name a device by port or by identity — the roster
/// knows both. Returns the port the transport can act on.
fn resolve_target(ctx: &Ctx, target: &str) -> Option<String> {
    let roster = ctx.roster.read().ok()?;
    if roster.by_port(target).is_some() {
        return Some(target.to_string());
    }
    roster.individual(target).and_then(|i| i.last_port.clone())
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

async fn route(ctx: &Ctx, method: &str, path: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let json = |v: serde_json::Value| (200u16, "application/json", serde_json::to_vec(&v).unwrap_or_default());
    match (method, path) {
        ("GET", "/api/status") => status(ctx).await,
        ("GET", "/api/log") => json(serde_json::json!(ctx.journal.tail(300))),
        ("GET", "/api/destinations") => json(serde_json::json!(
            DESTINATIONS.iter().map(|(group, key, title, blurb, url)| serde_json::json!({
                "group": group, "key": key, "title": title, "blurb": blurb, "url": url,
            })).collect::<Vec<_>>()
        )),
        ("GET", p) if p.starts_with("/api/shot/") => shot(ctx, p).await,
        ("GET", p) if p.starts_with("/api/device-image/") => {
            let class = p.trim_start_matches("/api/device-image/");
            device_image(ctx, class)
        }
        ("POST", p) if p.starts_with("/api/capture/") && p.ends_with("/save") => {
            let target = p.trim_start_matches("/api/capture/").trim_end_matches("/save");
            match resolve_target(ctx, target) {
                Some(port) => capture_save(ctx, &port).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("POST", p) if p.starts_with("/api/record/") => {
            let target = p.trim_start_matches("/api/record/");
            match resolve_target(ctx, target) {
                Some(port) => record_start(ctx, &port, body).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("GET", p) if p.starts_with("/api/record/") => {
            let target = p.trim_start_matches("/api/record/");
            match resolve_target(ctx, target) {
                Some(port) => record_status(ctx, &port).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("POST", p) if p.starts_with("/api/admission/") => {
            let target = p.trim_start_matches("/api/admission/");
            match resolve_target(ctx, target) {
                Some(port) => admission(ctx, &port).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/pause") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/pause");
            match resolve_target(ctx, target) {
                Some(port) => device_stream_toggle(ctx, &port, false).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/resume") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/resume");
            match resolve_target(ctx, target) {
                Some(port) => device_stream_toggle(ctx, &port, true).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("POST", p) if p.starts_with("/api/maintenance/") => {
            let target = p.trim_start_matches("/api/maintenance/");
            match resolve_target(ctx, target) {
                Some(port) => maintenance(ctx, &port, body).await,
                None => (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec()),
            }
        }
        ("POST", "/api/control") => control(ctx, body).await,
        ("POST", "/api/say") => say(ctx, body).await,
        _ => (404, "application/json", br#"{"error":"no such door"}"#.to_vec()),
    }
}

/// The class's product photo, straight from its manifest folder. The
/// class id is whitelisted against the catalog's own manifest map, so
/// the path can never wander outside hardware/classes/.
fn device_image(ctx: &Ctx, class: &str) -> (u16, &'static str, Vec<u8>) {
    let Some(file) = ctx.catalog.device_image(class) else {
        return (404, "application/json", br#"{"error":"no image declared for this class"}"#.to_vec());
    };
    match std::fs::read(&file) {
        Ok(bytes) => (200, "image/jpeg", bytes),
        Err(_) => (404, "application/json", br#"{"error":"declared image is missing"}"#.to_vec()),
    }
}

async fn status(ctx: &Ctx) -> (u16, &'static str, Vec<u8>) {
    let (tx, mut rx) = mpsc::channel(1);
    let _ = ctx.devices.send(DevicesCmd::Snapshot { reply: tx }).await;
    let rows = rx.recv().await.unwrap_or_default();
    let individuals = ctx
        .roster
        .read()
        .map(|r| r.snapshot())
        .unwrap_or_default();
    let json = serde_json::json!({
        "resident": { "name": "suzu", "version": env!("CARGO_PKG_VERSION") },
        "devices": rows,
        "roster": individuals,
    });
    (200, "application/json", serde_json::to_vec(&json).unwrap_or_default())
}

async fn shot(ctx: &Ctx, path: &str) -> (u16, &'static str, Vec<u8>) {
    let Some(raw) = path.trim_start_matches("/api/shot/").strip_suffix(".png") else {
        return (404, "application/json", br#"{"error":"shots are /api/shot/PORT.png"}"#.to_vec());
    };
    let Some(port) = resolve_target(ctx, raw) else {
        return (404, "application/json", br#"{"error":"no such device on the roster"}"#.to_vec());
    };
    if false {
        return (404, "application/json", br#"{"error":"shots are /api/shot/PORT.png"}"#.to_vec());
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let _ = ctx
        .devices
        .send(DevicesCmd::Capture { port: port.to_string(), reply: tx })
        .await;
    // The session answers within 10 s or the shot never happened.
    let png = tokio::task::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(11)).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    if png.is_empty() {
        return (409, "application/json", br#"{"error":"no shot - face unreachable or frame law missing"}"#.to_vec());
    }
    (200, "image/png", png)
}

async fn capture_save(ctx: &Ctx, port: &str) -> (u16, &'static str, Vec<u8>) {
    let (tx, mut rx) = mpsc::channel(1);
    let _ = ctx
        .devices
        .send(DevicesCmd::CaptureSave { port: port.to_string(), reply: tx })
        .await;
    match rx.recv().await {
        Some(Ok(path)) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "saved": path })).unwrap_or_default(),
        ),
        Some(Err(e)) => (409, "application/json", serde_json::to_vec(&serde_json::json!({ "error": format!("{e:#}") })).unwrap_or_default()),
        None => (500, "application/json", br#"{"error":"devices domain is gone"}"#.to_vec()),
    }
}

async fn record_start(ctx: &Ctx, port: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let secs = parsed.get("secs").and_then(|v| v.as_u64()).unwrap_or(4) as u32;
    let fps = parsed.get("fps").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
    let (tx, mut rx) = mpsc::channel(1);
    let _ = ctx
        .devices
        .send(DevicesCmd::RecordStart { port: port.to_string(), secs, fps, reply: tx })
        .await;
    match rx.recv().await {
        Some(Ok(())) => (200, "application/json", br#"{"started":true}"#.to_vec()),
        Some(Err(e)) => (409, "application/json", serde_json::to_vec(&serde_json::json!({ "error": format!("{e:#}") })).unwrap_or_default()),
        None => (500, "application/json", br#"{"error":"devices domain is gone"}"#.to_vec()),
    }
}

async fn record_status(ctx: &Ctx, port: &str) -> (u16, &'static str, Vec<u8>) {
    let (tx, mut rx) = mpsc::channel(1);
    let _ = ctx
        .devices
        .send(DevicesCmd::RecordStatus { port: port.to_string(), reply: tx })
        .await;
    let state = rx.recv().await.flatten();
    let json = match state {
        Some(RecordState { phase, frames, gif_path, .. }) => serde_json::json!({
            "phase": phase, "frames": frames, "gif": gif_path,
        }),
        None => serde_json::json!({ "phase": "idle" }),
    };
    (200, "application/json", serde_json::to_vec(&json).unwrap_or_default())
}

async fn admission(ctx: &Ctx, port: &str) -> (u16, &'static str, Vec<u8>) {
    let (tx, mut rx) = mpsc::channel(1);
    let _ = ctx
        .devices
        .send(DevicesCmd::AdmissionRetry { port: port.to_string(), reply: tx })
        .await;
    match rx.recv().await {
        Some(Ok(())) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "started": true, "note": "the verdict arrives on the log" })).unwrap_or_default(),
        ),
        Some(Err(e)) => (409, "application/json", serde_json::to_vec(&serde_json::json!({ "error": format!("{e:#}") })).unwrap_or_default()),
        None => (500, "application/json", br#"{"error":"devices domain is gone"}"#.to_vec()),
    }
}

async fn device_stream_toggle(ctx: &Ctx, port: &str, resume: bool) -> (u16, &'static str, Vec<u8>) {
    let (tx, mut rx) = mpsc::channel(1);
    let cmd = if resume {
        DevicesCmd::ResumeDevice { port: port.to_string(), reply: tx }
    } else {
        DevicesCmd::PauseDevice { port: port.to_string(), reply: tx }
    };
    let _ = ctx.devices.send(cmd).await;
    match rx.recv().await {
        Some(Ok(())) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "stream": if resume { "on" } else { "off" } })).unwrap_or_default(),
        ),
        Some(Err(e)) => (409, "application/json", serde_json::to_vec(&serde_json::json!({ "error": format!("{e:#}") })).unwrap_or_default()),
        None => (500, "application/json", br#"{"error":"devices domain is gone"}"#.to_vec()),
    }
}

async fn maintenance(ctx: &Ctx, port: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (tx, mut rx) = mpsc::channel(1);
    let _ = ctx
        .devices
        .send(DevicesCmd::MaintenanceStart { port: port.to_string(), kind, reply: tx })
        .await;
    match rx.recv().await {
        Some(Ok(())) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "started": true, "note": "the saga's steps arrive on the log" })).unwrap_or_default(),
        ),
        Some(Err(e)) => (409, "application/json", serde_json::to_vec(&serde_json::json!({ "error": format!("{e:#}") })).unwrap_or_default()),
        None => (500, "application/json", br#"{"error":"devices domain is gone"}"#.to_vec()),
    }
}

async fn control(ctx: &Ctx, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    match parsed.get("verb").and_then(|v| v.as_str()) {
        Some("pause") => {
            let _ = ctx.devices.send(DevicesCmd::Pause).await;
            (200, "application/json", br#"{"paused":true}"#.to_vec())
        }
        Some("resume") => {
            let _ = ctx.devices.send(DevicesCmd::Resume).await;
            (200, "application/json", br#"{"paused":false}"#.to_vec())
        }
        _ => (400, "application/json", br#"{"error":"verb is pause | resume"}"#.to_vec()),
    }
}

async fn say(ctx: &Ctx, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("transition").to_string();
    let label = parsed
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("from the workbench")
        .to_string();
    let urgency = parsed.get("urgency").and_then(|v| v.as_u64()).unwrap_or(2) as u8;
    let _ = ctx
        .moments
        .send(MomentsCmd::tell("workbench", &kind, Some(label), urgency.min(5)))
        .await;
    (200, "application/json", br#"{"rung":true}"#.to_vec())
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, payload: Vec<u8>) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\naccess-control-allow-origin: *\r\nconnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}
