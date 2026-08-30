from pathlib import Path

# ── api.rs: answer CORS preflights, expose failed sagas ──
p = Path("crates/suzu/src/resident/api.rs")
s = p.read_text(encoding="utf-8")

s = s.replace("""    if path == "/api/events" && method == "GET" {
        return events_stream(ctx, stream).await;
    }
""", """    if path == "/api/events" && method == "GET" {
        return events_stream(ctx, stream).await;
    }

    // The webview is a foreign origin: every POST is prefetched with an
    // OPTIONS preflight, and until it is answered the workbench cannot
    // so much as press a button (the silent-click lesson, 2026-08-30).
    if method == "OPTIONS" {
        let head = "HTTP/1.1 204 No Content\\r\\naccess-control-allow-origin: *\\r\\n\\
                    access-control-allow-methods: GET, POST, OPTIONS\\r\\n\\
                    access-control-allow-headers: content-type\\r\\n\\
                    access-control-max-age: 86400\\r\\n\\r\\n";
        stream.write_all(head.as_bytes()).await?;
        return Ok(());
    }
""")

s = s.replace("""async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, payload: Vec<u8>) -> Result<()> {""",
"""async fn write_response_preflight(stream: &mut TcpStream) -> Result<()> {
    let head = "HTTP/1.1 204 No Content\\r\\naccess-control-allow-origin: *\\r\\n\\
                access-control-allow-methods: GET, POST, OPTIONS\\r\\n\\
                access-control-allow-headers: content-type\\r\\n\\
                access-control-max-age: 86400\\r\\nconnection: close\\r\\n\\r\\n";
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, payload: Vec<u8>) -> Result<()> {""")

# route the OPTIONS method through the dedicated preflight writer
s = s.replace("""    if method == "OPTIONS" {
        let head = "HTTP/1.1 204 No Content\\r\\naccess-control-allow-origin: *\\r\\n\\
                    access-control-allow-methods: GET, POST, OPTIONS\\r\\n\\
                    access-control-allow-headers: content-type\\r\\n\\
                    access-control-max-age: 86400\\r\\n\\r\\n";
        stream.write_all(head.as_bytes()).await?;
        return Ok(());
    }
""", """    if method == "OPTIONS" {
        return write_response_preflight(&mut stream).await;
    }
""")
p.write_text(s, encoding="utf-8")
print("api preflight done")

# ── workbench: failed sagas stay visible on the card ──
p = Path("crates/workbench/ui/app.js")
s = p.read_text(encoding="utf-8")
old = """    const sagaRunning = saga?.state === "running";
    const sagaLine = sagaRunning
      ? `<div class="device-saga">${escapeHtml(saga.kind)}: ${
          saga.steps.length
            ? saga.steps.map((x) => (x.ok ? "\\u2713 " : "\\u2717 ") + escapeHtml(x.name)).join(" \\u2192 ")
            : "starting\\u2026"
        }</div>`
      : "";"""
new = """    const sagaRunning = saga?.state === "running";
    const sagaLine = sagaRunning
      ? `<div class="device-saga">${escapeHtml(saga.kind)}: ${
          saga.steps.length
            ? saga.steps.map((x) => (x.ok ? "\\u2713 " : "\\u2717 ") + escapeHtml(x.name)).join(" \\u2192 ")
            : "starting\\u2026"
        }</div>`
      : saga?.state === "failed"
        ? `<div class="device-saga">the last ${escapeHtml(saga.kind)} failed - see the log, or try again</div>`
        : "";"""
assert old in s
s = s.replace(old, new)
p.write_text(s, encoding="utf-8")
print("workbench failed-saga line done")
