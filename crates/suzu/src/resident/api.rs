//! The Resident's loopback HTTP API.
//!
//! A minimal HTTP/1.1 responder on 127.0.0.1:7899 (S-U-Z-U + 1).
//! Every `/api/events` connection starts with one complete snapshot,
//! then streams incremental events. Device and registry collections are
//! replaced as complete values. A heartbeat detects half-open
//! connections. Commands use bounded actor request/reply channels.
//!
//! CORS: `*` — the Tauri webview is a foreign origin to this socket,
//! and it is the only client that matters. The bind is loopback, so
//! the trust boundary is the machine itself (ADR-0002: local-first,
//! same machine as the devices).

use super::device::DeviceAction;
use super::devices::{DevicesCmd, DevicesSnapshot};
use super::events::{ResidentEvent, ResidentSnapshot, JournalLine, ServiceFacts};
use super::jobs::Job;
use super::jobs::Jobs;
use super::notifications::NotificationCmd;
use super::registry::DeviceRegistry;
use anyhow::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

pub const API_PORT: u16 = 7899;

/// Maximum actor command response time.
const DOOR_TIMEOUT: Duration = Duration::from_secs(5);
/// A comment frame detects half-open connections,
/// so a killed resident is *down* within seconds, not minutes.
const PING_PERIOD: Duration = Duration::from_secs(10);

/// The event journal — the Log page's memory. Bounded, in-memory,
/// process-local and is cleared on restart.
pub struct Journal {
    lines: Mutex<VecDeque<JournalLine>>,
    tx: tokio::sync::broadcast::Sender<JournalLine>,
}

impl Journal {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(256);
        Self {
            lines: Mutex::new(VecDeque::new()),
            tx,
        }
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
            const JOURNAL_CAP: usize = 600;
            while lines.len() > JOURNAL_CAP {
                lines.pop_front();
            }
        }
        let _ = self.tx.send(line);
    }

    pub fn tail(&self, limit: usize) -> Vec<JournalLine> {
        self.lines
            .lock()
            .expect("journal lock")
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }
}

/// The destinations the About page may reach. The closed vocabulary: a
/// URL is only ever opened if it points at the product's own surfaces
/// — resolved here, never hardened into markup.
const DESTINATIONS: &[(&str, &str, &str, &str, &str)] = &[
    ("Ghostlight's sibling", "home", "Project page",
        "What suzu is, who it is for, and how it behaves.",
        "https://github.com/sylin-org/suzu"),
    ("Ghostlight's sibling", "contract", "The face contract",
        "What every face does, regardless of dialect.",
        "https://github.com/sylin-org/suzu/blob/dev/docs/the-face-contract.md"),
    ("Design documentation", "adr_lake", "RP2040 matrix rendering",
        "Event and host-metric rendering rules for the matrix display.",
        "https://github.com/sylin-org/suzu/blob/dev/docs/adr/0001-the-lake.md"),
];

static UI: include_dir::Dir<'_> = ::include_dir::include_dir!("$CARGO_MANIFEST_DIR/ui");

/// The content type for one embedded UI file. The UI ships five kinds
/// of file; anything else is served as data (browsers sniff nothing
/// here — the CSP-minded set is explicit).
fn ui_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Serve one embedded workbench file, or the index for `/`.
fn ui_file(path: &str) -> Option<(u16, &'static str, Vec<u8>)> {
    let rel = path.trim_start_matches('/');
    let (rel, file) = if rel.is_empty() {
        ("index.html", UI.get_file("index.html")?)
    } else {
        (rel, UI.get_file(rel)?)
    };
    Some((200, ui_content_type(rel), file.contents().to_vec()))
}

pub struct Ctx {
    pub catalog: Arc<crate::Catalog>,
    pub jobs: Arc<Jobs>,
    pub events: tokio::sync::broadcast::Sender<ResidentEvent>,
    pub devices: mpsc::Sender<DevicesCmd>,
    pub notifications: mpsc::Sender<NotificationCmd>,
    pub registry: Arc<std::sync::RwLock<DeviceRegistry>>,
    pub journal: Arc<Journal>,
    /// Number of live `/api/events` connections. Automatic capture runs
    /// only while this is non-zero.
    pub streams: std::sync::atomic::AtomicUsize,
}

/// Bind before opening serial ports so a second Resident fails early.
pub async fn bind() -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", API_PORT)).await
}

pub async fn listen(ctx: Arc<Ctx>, listener: TcpListener) -> Result<()> {
    println!(
        "[api] listening on http://127.0.0.1:{API_PORT} — snapshot and event stream enabled"
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
    let mut authorization = String::new();
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = lower.strip_prefix("authorization:") {
            authorization = v.trim().to_string();
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
    let body = String::from_utf8_lossy(&body[..body.len().min(content_length)]).to_string();

    if path == "/api/events" && method == "GET" {
        return events_stream(ctx, stream).await;
    }

    // The bell (POST /api/say) is the one endpoint wired for outside
    // callers; when SUZU_API_TOKEN is set it must arrive as a bearer.
    if method == "POST" && path == "/api/say"
        && let Some(token) = std::env::var("SUZU_API_TOKEN").ok().filter(|t| !t.is_empty())
        && authorization != format!("bearer {token}")
    {
        write_response(&mut stream, 401, "application/json",
            br#"{"error":"the bell needs its token"}"#.to_vec()).await?;
        return Ok(());
    }
    let started = Instant::now();
    let (status, content_type, payload) = route(&ctx, &method, &path, &body).await;
    if method == "POST" {
        // Log one access line per command.
        ctx.journal.record(
            "api",
            &format!(
                "{method} {path} → {status} ({} ms)",
                started.elapsed().as_millis()
            ),
        );
    }
    write_response(&mut stream, status, content_type, payload).await?;
    if path == "/api/shutdown" && method == "POST" {
        println!("[api] shutdown requested");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::process::exit(0);
    }
    Ok(())
}

/// Send one initial snapshot followed by incremental events and journal entries.
async fn events_stream(ctx: Arc<Ctx>, mut stream: TcpStream) -> Result<()> {
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\naccess-control-allow-origin: *\r\nconnection: keep-alive\r\n\r\n";
    stream.write_all(head.as_bytes()).await?;
    // Subscribe before building the snapshot so concurrent events are
    // replayed as idempotent collection replacements.
    let mut fact_rx = ctx.events.subscribe();
    let mut journal_rx = ctx.journal.subscribe();
    ctx.streams.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let snap = snapshot_fact(&ctx).await;
    let snap_json =
        serde_json::to_string(&ResidentEvent::Snapshot { snapshot: snap }).unwrap_or_default();
    stream
        .write_all(format!("event: snapshot\ndata: {snap_json}\n\n").as_bytes())
        .await?;
    stream.flush().await?;

    let mut ping = tokio::time::interval(PING_PERIOD);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if stream.write_all(b": ping\n\n").await.is_err() {
                    break;
                }
                stream.flush().await?;
            }
            ev = fact_rx.recv() => match ev {
                Ok(ev) => {
                    if !is_delta(&ev) {
                        continue;
                    }
                    let json = serde_json::to_string(&ev).unwrap_or_default();
                    if stream
                        .write_all(format!("event: fact\ndata: {json}\n\n").as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                    stream.flush().await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            line = journal_rx.recv() => match line {
                Ok(line) => {
                    let payload = serde_json::json!({ "type": "journal", "line": line });
                    let json = serde_json::to_string(&payload).unwrap_or_default();
                    if stream
                        .write_all(format!("event: journal\ndata: {json}\n\n").as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                    stream.flush().await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
        }
    }
    // Disable automatic capture when the final event client disconnects.
    let remaining = ctx.streams.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
    if remaining == 0 {
        let _ = ctx
            .devices
            .send(DevicesCmd::WatchMedia { on: false, reply: None })
            .await;
    }
    Ok(())
}

/// The delta vocabulary: the facts a client's store is built from.
/// Other events are represented by formatted journal entries.
fn is_delta(ev: &ResidentEvent) -> bool {
    matches!(
        ev,
        ResidentEvent::Devices { .. }
            | ResidentEvent::DeviceRegistry { .. }
            | ResidentEvent::Job { .. }
            | ResidentEvent::Frame { .. }
            | ResidentEvent::Paused { .. }
            | ResidentEvent::MediaWatched { .. }
    )
}

/// Build the initial connection snapshot.
async fn snapshot_fact(ctx: &Ctx) -> ResidentSnapshot {
    let devs = request_devices(&ctx.devices, |reply| DevicesCmd::Snapshot { reply })
        .await
        .unwrap_or_else(|_| DevicesSnapshot {
            devices: Vec::new(),
            paused: false,
            media_watched: false,
            frames: Vec::new(),
        });
    let registry = ctx.registry.read().map(|r| r.snapshot()).unwrap_or_default();
    ResidentSnapshot {
        service: ServiceFacts {
            name: "suzu".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            paused: devs.paused,
        },
        devices: devs.devices,
        registry,
        jobs: ctx.jobs.all(),
        journal: ctx.journal.tail(200),
        frames: devs.frames,
        media_watched: devs.media_watched,
    }
}

/// Send a devices-actor command and wait for its bounded reply.
async fn request_devices<T, F>(tx: &mpsc::Sender<DevicesCmd>, build: F) -> Result<T, String>
where
    F: FnOnce(mpsc::Sender<T>) -> DevicesCmd,
{
    let (reply_tx, mut reply_rx) = mpsc::channel(1);
    tx.send(build(reply_tx))
        .await
        .map_err(|_| "the devices domain is not running".to_string())?;
    match tokio::time::timeout(DOOR_TIMEOUT, reply_rx.recv()).await {
        Ok(Some(reply)) => Ok(reply),
        Ok(None) => Err("the devices domain dropped the reply".into()),
        Err(_) => Err(format!(
            "the devices actor did not answer within {}s; try again",
            DOOR_TIMEOUT.as_secs()
        )),
    }
}

/// Resolve a device port from either its port name or device ID.
fn resolve_target(ctx: &Ctx, target: &str) -> Option<String> {
    let registry = ctx.registry.read().ok()?;
    if registry.by_port(target).is_some() {
        return Some(target.to_string());
    }
    registry.registered_device(target).and_then(|i| i.last_port.clone())
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Standard API response envelope from docs/the-door-contract.md.
fn envelope(confirmed: bool, message: impl serde::Serialize) -> (u16, &'static str, Vec<u8>) {
    (
        if confirmed { 200 } else { 409 },
        "application/json",
        serde_json::to_vec(&serde_json::json!({
            "confirmed": confirmed,
            "message": message,
        }))
        .unwrap_or_default(),
    )
}

/// Return the standard response for an unknown resource.
fn no_such(msg: &'static str) -> (u16, &'static str, Vec<u8>) {
    (
        404,
        "application/json",
        serde_json::to_vec(&serde_json::json!({ "confirmed": false, "message": msg }))
            .unwrap_or_default(),
    )
}

async fn route(ctx: &Ctx, method: &str, path: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let json = |v: serde_json::Value| (200u16, "application/json", serde_json::to_vec(&v).unwrap_or_default());
    match (method, path) {
        // The workbench UI, served by the Resident itself: any browser
        // on the host is the family window (the desktop shell wraps the
        // same files).
        ("GET", "/") | ("GET", "/index.html") => {
            ui_file("/").unwrap_or_else(|| no_such("no workbench UI in this build"))
        }
        ("GET", p) if !p.starts_with("/api/") => ui_file(p)
            .unwrap_or_else(|| no_such("no such page")),

        // Debug endpoint for the in-memory journal.
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
        ("GET", p) if p.starts_with("/api/faceplate-preview/") => {
            // <class>/<id>.gif — whitelisted against the declared
            // bundles; a missing capture is a 404 the chooser
            // answers with pictogram and words (ADR-0005).
            let rest = p.trim_start_matches("/api/faceplate-preview/");
            let rest = rest.trim_end_matches(".gif").trim_end_matches(".png");
            let (class, id) = match rest.split_once('/') {
                Some(pair) => pair,
                None => return no_such("no such faceplate"),
            };
            match ctx.catalog.faceplate_preview(class, id) {
                Some(path) => match std::fs::read(&path) {
                    Ok(bytes) => {
                        // Derive the content type from the actual file.
                        let kind = if path.extension().and_then(|e| e.to_str()) == Some("png") {
                            "image/png"
                        } else {
                            "image/gif"
                        };
                        (200, kind, bytes)
                    }
                    Err(_) => no_such("the preview capture is missing"),
                },
                None => no_such("no faceplate preview is available"),
            }
        }
        ("GET", p) if p.starts_with("/api/faceplates/") => {
            let class = p.trim_start_matches("/api/faceplates/");
            let list: Vec<serde_json::Value> = ctx
                .catalog
                .faceplates_for_class(class)
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "id": f.id,
                        "name": f.display_name,
                        "blurb": f.blurb,
                        "mount": f.mount,
                        "version": f.version,
                        "preview": f.has_preview.then(|| {
                            format!("/api/faceplate-preview/{class}/{}.gif", f.id)
                        }),
                    })
                })
                .collect();
            (200, "application/json", serde_json::to_vec(&list).unwrap_or_default())
        }
        ("POST", p) if p.starts_with("/api/capture/") && p.ends_with("/save") => {
            let target = p.trim_start_matches("/api/capture/").trim_end_matches("/save");
            match resolve_target(ctx, target) {
                Some(port) => capture_save(ctx, &port).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/record/") => {
            let target = p.trim_start_matches("/api/record/");
            match resolve_target(ctx, target) {
                Some(port) => record_start(ctx, &port, body).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/admission/") => {
            let target = p.trim_start_matches("/api/admission/");
            match resolve_target(ctx, target) {
                Some(port) => admission(ctx, &port).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("GET", p) if p.starts_with("/api/device/identify/") => {
            // The utterance reads as itself: identify device COM24.
            let token = p.trim_start_matches("/api/device/identify/");
            let port = resolve_target(ctx, token).or_else(|| {
                let ports: Vec<String> =
                    crate::enumerate().into_iter().map(|e| e.name).collect();
                match super::devices::resolve_target_token(token, &ports) {
                    super::devices::SayTarget::Port(known) => Some(known),
                    _ => None,
                }
            });
            match port {
                Some(port) => identify(ctx, &port).await,
                None => no_such("no such port on this machine"),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/identify") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/identify");
            match resolve_target(ctx, target) {
                Some(port) => device_action(ctx, &port, DeviceAction::Identify, None).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/pause") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/pause");
            match resolve_target(ctx, target) {
                Some(port) => device_stream_toggle(ctx, &port, false).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/resume") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/resume");
            match resolve_target(ctx, target) {
                Some(port) => device_stream_toggle(ctx, &port, true).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/install") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/install");
            match resolve_target(ctx, target) {
                Some(port) => device_action(ctx, &port, DeviceAction::Install, faceplate_from(body)).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/update") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/update");
            match resolve_target(ctx, target) {
                Some(port) => device_action(ctx, &port, DeviceAction::Update, faceplate_from(body)).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/device/") && p.ends_with("/factory-reset") => {
            let target = p.trim_start_matches("/api/device/").trim_end_matches("/factory-reset");
            match resolve_target(ctx, target) {
                Some(port) => device_action(ctx, &port, DeviceAction::FactoryReset, None).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", p) if p.starts_with("/api/maintenance/") => {
            let target = p.trim_start_matches("/api/maintenance/");
            match resolve_target(ctx, target) {
                Some(port) => maintenance(ctx, &port, body).await,
                None => no_such("no such device on the registry"),
            }
        }
        ("POST", "/api/shutdown") => (200, "application/json", serde_json::to_vec(&serde_json::json!({
        "confirmed": true,
        "stopping": true,
        "message": "resident shutdown requested",
    })).unwrap_or_default()),
        ("POST", "/api/ui") => ui_action(ctx, body).await,
        ("POST", "/api/control") => control(ctx, body).await,
        ("POST", "/api/say") => say(ctx, body).await,
        _ => no_such("no such endpoint"),
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

/// Return the latest frame when it satisfies the freshness limit.
async fn shot(ctx: &Ctx, path: &str) -> (u16, &'static str, Vec<u8>) {
    let Some(raw) = path.trim_start_matches("/api/shot/").strip_suffix(".png") else {
        return (404, "application/json", br#"{"error":"shots are /api/shot/PORT.png"}"#.to_vec());
    };
    let Some(port) = resolve_target(ctx, raw) else {
        return no_such("no such device on the registry");
    };
    match request_devices(&ctx.devices, |reply| DevicesCmd::LatestFrame { port, reply }).await {
        Ok(Ok(png)) => (200, "image/png", png),
        Ok(Err(e)) => envelope(false, format!("{e:#}")),
        Err(e) => envelope(false, e),
    }
}

async fn capture_save(ctx: &Ctx, port: &str) -> (u16, &'static str, Vec<u8>) {
    match request_devices(&ctx.devices, |reply| DevicesCmd::CaptureSave { port: port.to_string(), reply }).await {
        Ok(Ok(path)) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "confirmed": true,
                "saved": path,
                "message": format!("the newest frame of {port}, saved"),
            }))
            .unwrap_or_default(),
        ),
        Ok(Err(e)) => envelope(false, format!("{e:#}")),
        Err(e) => envelope(false, e),
    }
}

async fn record_start(ctx: &Ctx, port: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let secs = parsed.get("secs").and_then(|v| v.as_u64()).unwrap_or(4) as u32;
    let fps = parsed.get("fps").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
    let (secs, fps) = (secs.clamp(1, 60), fps.clamp(1, 5));
    let job_id = format!("record-{}-{}", port, chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    ctx.jobs.create(Job {
        id: job_id.clone(),
        kind: "record".into(),
        target: port.to_string(),
        device_id: None,
        state: "recording".into(),
        index: 0,
        total: secs * fps,
        label: format!("{secs}s at {fps} fps"),
        gif: None,
        started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    match request_devices(&ctx.devices, |reply| DevicesCmd::RecordStart {
        port: port.to_string(),
        job_id,
        secs,
        fps,
        reply,
    })
    .await
    {
        Ok(Ok(())) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "confirmed": true,
                "record": { "secs": secs, "fps": fps },
                "message": format!("recording {port} — {secs}s at {fps} fps; the GIF's verdict arrives as a Job fact"),
            }))
            .unwrap_or_default(),
        ),
        Ok(Err(e)) => envelope(false, format!("{e:#}")),
        Err(e) => envelope(false, e),
    }
}

async fn admission(ctx: &Ctx, port: &str) -> (u16, &'static str, Vec<u8>) {
    match request_devices(&ctx.devices, |reply| DevicesCmd::AdmissionRetry { port: port.to_string(), reply }).await {
        Ok(Ok(())) => (
            200,
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "confirmed": true,
                "admission": "retry",
                "message": format!("admission tests restarted on {port}; results appear in the log"),
            }))
            .unwrap_or_default(),
        ),
        Ok(Err(e)) => envelope(false, format!("{e:#}")),
        Err(e) => envelope(false, e),
    }
}

/// Send the identify indication to one device.
async fn identify(ctx: &Ctx, port: &str) -> (u16, &'static str, Vec<u8>) {
    device_action(ctx, port, DeviceAction::Identify, None).await
}

async fn device_stream_toggle(ctx: &Ctx, port: &str, resume: bool) -> (u16, &'static str, Vec<u8>) {
    device_action(
        ctx,
        port,
        if resume { DeviceAction::Resume } else { DeviceAction::Pause },
        None,
    )
    .await
}

async fn maintenance(ctx: &Ctx, port: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let faceplate = parsed
        .get("faceplate")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let action = match kind.as_str() {
        "install" | "provision" | "adopt" => DeviceAction::Install,
        "soft" => DeviceAction::Update,
        "factory" => DeviceAction::FactoryReset,
        _ => return envelope(false, format!("unknown maintenance kind {kind:?}")),
    };
    device_action(ctx, port, action, faceplate).await
}

fn faceplate_from(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("faceplate").and_then(|v| v.as_str()).map(str::to_string))
}

async fn device_action(
    ctx: &Ctx,
    port: &str,
    action: DeviceAction,
    faceplate: Option<String>,
) -> (u16, &'static str, Vec<u8>) {
    match request_devices(&ctx.devices, |reply| DevicesCmd::Act {
        port: port.to_string(),
        action,
        faceplate: faceplate.clone(),
        reply,
    })
    .await {
        Ok(Ok(())) => {
            let mut response = serde_json::json!({
                "confirmed": true,
                "message": action_message(port, action),
            });
            response[action.as_str()] = action_echo(port, action, faceplate.as_deref());
            (200, "application/json", serde_json::to_vec(&response).unwrap_or_default())
        }
        Ok(Err(e)) => envelope(false, format!("{e:#}")),
        Err(e) => envelope(false, e),
    }
}

fn action_echo(port: &str, action: DeviceAction, faceplate: Option<&str>) -> serde_json::Value {
    match action {
        DeviceAction::Pause => serde_json::json!("off"),
        DeviceAction::Resume => serde_json::json!("on"),
        DeviceAction::Identify => serde_json::json!(port),
        DeviceAction::Install | DeviceAction::Update => {
            serde_json::json!({ "faceplate": faceplate })
        }
        DeviceAction::FactoryReset => serde_json::json!(true),
    }
}

fn action_message(port: &str, action: DeviceAction) -> String {
    match action {
        DeviceAction::Pause => format!("streaming paused for {port}"),
        DeviceAction::Resume => format!("streaming resumed for {port}; no admission test required"),
        DeviceAction::Identify => {
            format!("identify indication sent to {port}")
        }
        DeviceAction::Install => {
            format!("installation started for {port}; progress appears in the log")
        }
        DeviceAction::Update => {
            format!("update started for {port}; progress appears in the log")
        }
        DeviceAction::FactoryReset => {
            format!("factory reset started for {port}; progress appears in the log")
        }
    }
}

/// Parse and dispatch Workbench actions. `confirmed` indicates whether
/// state changed; the echo contains the normalized request.
#[derive(Debug, serde::Deserialize)]
struct UiAction {
    watch_media: Option<Watch>,
}

#[derive(Debug, PartialEq, Eq)]
enum Watch {
    On,
    Off,
}

impl<'de> serde::Deserialize<'de> for Watch {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "on" => Ok(Watch::On),
            "off" => Ok(Watch::Off),
            other => Err(serde::de::Error::custom(format!(
                "watch_media is \"on\" or \"off\", not {other:?}"
            ))),
        }
    }
}

impl Watch {
    fn as_str(&self) -> &'static str {
        match self {
            Watch::On => "on",
            Watch::Off => "off",
        }
    }
}

async fn ui_action(ctx: &Ctx, body: &str) -> (u16, &'static str, Vec<u8>) {
    let json = |v: serde_json::Value| (200u16, "application/json", serde_json::to_vec(&v).unwrap_or_default());
    let Ok(action) = serde_json::from_str::<UiAction>(body) else {
        return (
            400,
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "confirmed": false,
                "message": "expected {\"watch_media\":\"on\"|\"off\"}",
            }))
            .unwrap_or_default(),
        );
    };
    let Some(watch) = action.watch_media else {
        return json(serde_json::json!({
            "confirmed": false,
            "message": "request did not include watch_media",
        }));
    };
    let on = watch == Watch::On;
    match request_devices(&ctx.devices, |reply| DevicesCmd::WatchMedia { on, reply: Some(reply) }).await {
        Ok(report) => {
            let message = match (on, report.changed) {
                (true, true) => format!("Streaming captures on {} devices", report.active_captures),
                (true, false) => {
                    format!("Streaming already enabled ({} devices)", report.active_captures)
                }
                (false, true) => "Streaming captures disabled".to_string(),
                (false, false) => "Streaming captures already disabled".to_string(),
            };
            json(serde_json::json!({
                "confirmed": report.changed,
                "watch_media": watch.as_str(),
                "message": message,
            }))
        }
        Err(e) => (
            504,
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "confirmed": false, "message": e }))
                .unwrap_or_default(),
        ),
    }
}

async fn control(ctx: &Ctx, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    match parsed.get("verb").and_then(|v| v.as_str()) {
        Some(verb @ ("pause" | "resume")) => {
            let resume = verb == "resume";
            let asked = move |reply| {
                if resume {
                    DevicesCmd::Resume { reply }
                } else {
                    DevicesCmd::Pause { reply }
                }
            };
            match request_devices(&ctx.devices, asked).await {
                Ok(report) => {
                    let message = match (resume, report.changed) {
                        (true, true) => format!(
                            "stream resumed — {} device session(s) opened",
                            report.ports
                        ),
                        (true, false) => "the stream was already active".to_string(),
                        (false, true) => format!(
                            "stream paused — {} device port(s) released",
                            report.ports
                        ),
                        (false, false) => "the stream was already paused".to_string(),
                    };
                    (
                        200,
                        "application/json",
                        serde_json::to_vec(&serde_json::json!({
                            "confirmed": report.changed,
                            "verb": verb,
                            "message": message,
                        }))
                        .unwrap_or_default(),
                    )
                }
                Err(e) => envelope(false, e),
            }
        }
        _ => (400, "application/json", br#"{"error":"verb is pause | resume"}"#.to_vec()),
    }
}

/// The bell rope (ADR-0006 grammar): the workbench posts `kind`, CI
/// runners and hooks post `signal` — same rope, either hand. The
/// display-event budget governs the ring; SUZU_API_TOKEN gates callers
/// from beyond the loopback when the host sets one.
async fn say(ctx: &Ctx, body: &str) -> (u16, &'static str, Vec<u8>) {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let kind = parsed
        .get("kind")
        .or_else(|| parsed.get("signal"))
        .and_then(|v| v.as_str())
        .unwrap_or("transition")
        .to_string();
    let label = parsed
        .get("label")
        .or_else(|| parsed.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("from the workbench")
        .to_string();
    let urgency = parsed.get("urgency").and_then(|v| v.as_u64()).unwrap_or(2) as u8;
    let _ = ctx
        .notifications
        .send(NotificationCmd::submit("workbench", &kind, Some(label.clone()), urgency.min(5)))
        .await;
    (
        200,
        "application/json",
        serde_json::to_vec(&serde_json::json!({
            "confirmed": true,
            "say": kind,
            "message": format!("event submitted — {}", label),
        }))
        .unwrap_or_default(),
    )
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, payload: Vec<u8>) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        504 => "Gateway Timeout",
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

#[cfg(test)]
mod ui_tests {
    use super::*;

    #[test]
    fn the_workbench_ships_in_the_binary() {
        let (status, ctype, body) = ui_file("/").expect("index");
        assert_eq!(status, 200);
        assert_eq!(ctype, "text/html; charset=utf-8");
        assert!(String::from_utf8_lossy(&body).contains("<title>Suzu</title>"));
        let (_, ctype, _) = ui_file("/app.js").expect("app.js");
        assert_eq!(ctype, "text/javascript; charset=utf-8");
        let (_, ctype, _) = ui_file("/styles.css").expect("styles");
        assert_eq!(ctype, "text/css; charset=utf-8");
        assert!(ui_file("/no-such-thing").is_none());
    }

    #[test]
    fn content_types_cover_the_ui_vocabulary() {
        assert_eq!(ui_content_type("x.html"), "text/html; charset=utf-8");
        assert_eq!(ui_content_type("x.js"), "text/javascript; charset=utf-8");
        assert_eq!(ui_content_type("x.css"), "text/css; charset=utf-8");
        assert_eq!(ui_content_type("x.png"), "image/png");
        assert_eq!(ui_content_type("x.weird"), "application/octet-stream");
    }
}
