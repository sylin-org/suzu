from pathlib import Path
p = Path("crates/workbench/ui/app.js")
s = p.read_text(encoding="utf-8")

old = """      try {
        const r = await call("GET", `/api/shot/${port}.png`);
        const age = document.getElementById(`age-${port}`);
        if (r.ok) {
          const url = "data:image/png;base64," + btoa(unescape(encodeURIComponent(r.body)));
          img.src = url;
          const previous = mediaUrls.get(port);
          if (previous) URL.revokeObjectURL(previous);
          mediaUrls.set(port, url);
          if (age) age.textContent = `frame ${new Date().toLocaleTimeString()}`;
        } else if (age) {
          age.textContent = "no shot — unreachable";
        }
      } catch { /* one miss is not a story */ }"""
new = """      // <img> display is not CORS-gated: the shot loads straight from
      // the resident, and onload/onerror say which happened.
      const age = document.getElementById(`age-${port}`);
      img.onload = () => { if (age) age.textContent = `frame ${new Date().toLocaleTimeString()}`; };
      img.onerror = () => { if (age) age.textContent = "no shot — unreachable"; };
      img.src = `${API}/api/shot/${port}.png?t=${Date.now()}`;"""
assert old in s, "media tick block"
s = s.replace(old, new)
s = s.replace("  const mediaUrls = new Map(); // port → the blob URL in the <img>; the old one is revoked\n", "")
p.write_text(s, encoding="utf-8")
print("media tick fixed")
