import re
from pathlib import Path

# ── app.js: transport via Tauri IPC ──
p = Path("crates/workbench/ui/app.js")
s = p.read_text(encoding="utf-8")

if "invoke(\"api\"" not in s:
    old = """  async function getJSON(path) {
    const r = await fetch(API + path);
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json();
  }"""
    new = """  // The Rust shell speaks to the Resident; the webview never makes a
  // cross-origin request, so CORS does not exist in this product.
  async function call(method, path, body) {
    const r = await window.__TAURI__.core.invoke("api", { method, path, body: body ?? null });
    return {
      status: r.status,
      ok: r.status > 0 && r.status < 400,
      json() { try { return JSON.parse(r.body || "{}"); } catch { return {}; } },
    };
  }
  async function getJSON(path) {
    const r = await call("GET", path);
    if (r.status !== 200) throw new Error(`${path}: ${r.status}`);
    return r.json();
  }"""
    assert old in s, "getJSON"
    s = s.replace(old, new)

    old = """      const r = await fetch(`${API}/api/control`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ verb }),
      });
      if (!r.ok) throw new Error(await r.text());"""
    new = """      const r = await call("POST", "/api/control", { verb });
      if (!r.ok) throw new Error(r.json().error ?? r.status);"""
    assert old in s, "wheel"
    s = s.replace(old, new)

    old = """      const r = await fetch(API + path, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const d = await r.json().catch(() => ({}));
      if (!r.ok) toast(d.error ?? `${path} failed`);
      return d;"""
    new = """      const r = await call("POST", path, body);
      const d = r.json();
      if (!r.ok) toast(d.error ?? `${path} failed`);
      return d;"""
    assert old in s, "postOrToast"
    s = s.replace(old, new)

    old = """        const r = await fetch(`${API}/api/capture/${port}/save`, { method: "POST" });
        const d = await r.json();
        if (d.saved) toast(`saved ${d.saved}`);
        else toast(`no shot: ${d.error ?? "unknown"}`);"""
    new = """        const r = await call("POST", `/api/capture/${port}/save`, {});
        const d = r.json();
        if (d.saved) toast(`saved ${d.saved}`);
        else toast(`no shot: ${d.error ?? "unknown"}`);"""
    assert old in s, "capture-save"
    s = s.replace(old, new)

# ── EventSource -> Tauri house events ──
pat = re.compile(
    r"  // The live wire: the house's facts arrive as they happen, and the\n"
    r"  // roster re-renders within a beat of each one\.\n"
    r"  const es = new EventSource\(`\$\{API\}/api/events`\);\n"
    r"  let pollQueued = false;\n"
    r"  es\.onmessage = \(\) => \{\n"
    r"    if \(!pollQueued\) \{\n"
    r"      pollQueued = true;\n"
    r"      setTimeout\(\(\) => \{ pollQueued = false; pollStatus\(\); \}, 150\);\n"
    r"    \}\n"
    r"  \};\n"
    r"  es\.onerror = \(\) => \{ /\* the house is away; the status poll says so \*/ \};",
)
new_events = """  // The live wire: the house's facts arrive as they happen (the Rust
  // shell holds the SSE connection and republishes), and the roster
  // re-renders within a beat of each one.
  let pollQueued = false;
  if (window.__TAURI__?.event?.listen) {
    window.__TAURI__.event.listen("house", () => {
      if (!pollQueued) {
        pollQueued = true;
        setTimeout(() => { pollQueued = false; pollStatus(); }, 150);
      }
    });
  }"""
s, n = pat.subn(new_events, s, count=1)
assert n == 1, "EventSource block not found"

# the media tick still fetches shots directly — <img> display needs no
# CORS, but route it through the proxy for one-transport consistency
s = s.replace('const r = await fetch(`${API}/api/shot/${port}.png`, { cache: "no-store" });',
              'const r = await call("GET", `/api/shot/${port}.png`);')
s = s.replace("""          if (r.ok) {
            const url = URL.createObjectURL(await r.blob());
            img.src = url;""", """          if (r.ok) {
            const url = "data:image/png;base64," + btoa(r.body);
            img.src = url;""")
s = s.replace("""          if (r.ok) {
            const url = URL.createObjectURL(await r.blob());
            img.src = url;
            const previous = mediaUrls.get(port);
            if (previous) URL.revokeObjectURL(previous);
            mediaUrls.set(port, url);""", """          if (r.ok) {
            img.src = "data:image/png;base64," + btoa(unescape(encodeURIComponent(r.body)));
            const previous = mediaUrls.get(port);
            if (previous) URL.revokeObjectURL(previous);
            mediaUrls.set(port, url);""")
p.write_text(s, encoding="utf-8")
print("app.js done")

# ── resident: strip the CORS headers ──
p = Path("crates/suzu/src/resident/api.rs")
s = p.read_text(encoding="utf-8")
s = s.replace(r'\r\naccess-control-allow-origin: *', "")
p.write_text(s, encoding="utf-8")
print("resident CORS stripped:", "access-control-allow-origin" not in s)
