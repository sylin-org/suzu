//! Suzu workbench — the keeper's window. Everything it shows comes
//! from the Resident's loopback API; it invents nothing.

(async () => {
  "use strict";

  const API = "http://127.0.0.1:7899";
  const el = {};
  for (const id of [
    "lamp", "state-word", "state-facts", "wheel", "wheel-label", "wheel-icon",
    "status-count", "device-list", "log-count", "log-stream",
    "media-grid", "media-note", "about-facts", "about-links", "card-version", "service",
    "toast", "confirm-dialog", "confirm-title", "confirm-detail",
  ]) {
    el[id.replaceAll("-", "_")] = document.getElementById(id);
  }

  const installing = new Set(); // ports with a saga in flight
  let paused = false;
  let activeView = "status";
  let mediaTimer = null;
  let confirmResolve = null;

  const escapeHtml = (s) => String(s).replace(/[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  // The Rust shell speaks to the Resident; the webview never makes a
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
  }

  function toast(text) {
    el.toast.textContent = text;
    el.toast.hidden = false;
    clearTimeout(toast.timer);
    toast.timer = setTimeout(() => { el.toast.hidden = true; }, 4000);
  }

  function confirmChange(title, detail) {
    return new Promise((resolve) => {
      el.confirm_title.textContent = title;
      el.confirm_detail.textContent = detail;
      el.confirm_dialog.hidden = false;
      confirmResolve = resolve;
    });
  }

  // ── navigation ──────────────────────────────────────────────────
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".tab").forEach((t) => {
        t.classList.toggle("active", t === tab);
        t.removeAttribute("aria-current");
        if (t === tab) t.setAttribute("aria-current", "page");
      });
      activeView = tab.dataset.view;
      document.querySelectorAll(".view").forEach((v) =>
        v.classList.toggle("active", v.dataset.page === activeView));
      if (activeView === "media") startMedia();
      else stopMedia();
    });
  });

  // ── the wheel: pause / resume, one datagram's worth of respect ──
  el.wheel.addEventListener("click", async () => {
    const verb = paused ? "resume" : "pause";
    try {
      const r = await call("POST", "/api/control", { verb });
      if (!r.ok) throw new Error(r.json().error ?? r.status);
    } catch (e) {
      toast(`the house did not answer: ${e.message}`);
    }
  });

  function renderWheel() {
    el.wheel.disabled = false;
    el.wheel.dataset.intent = paused ? "resume" : "pause";
    el.wheel_label.textContent = paused ? "Resume" : "Pause";
    el.wheel_icon.innerHTML = paused
      ? '<path d="M8 5v14l11-7z"/>'
      : '<rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/>';
  }

  // ── status ──────────────────────────────────────────────────────
  function deviceCard(row, rosterEntry) {
    // The keeper's formula: a device is LIVE, NEW, or PAUSED - and the
    // buttons are exactly the ones its state offers, nothing else.
    const saga = rosterEntry?.maintenance;
    const sagaRunning = saga?.state === "running";
    const isInstalling = installing.has(row.port) || sagaRunning;
    const lc = isInstalling ? "installing" : (row.lifecycle ?? "new");
    const pill = isInstalling
      ? "INSTALLING"
      : { live: "LIVE", new: "NEW", paused: "PAUSED" }[lc] ?? escapeHtml(lc.toUpperCase());
    const pillTone = isInstalling ? "warn" : ({ live: "good", new: "warn", paused: "info" }[lc] ?? "info");
    const lock = isInstalling ? "disabled" : "";
    const currentStep = [...(saga?.steps ?? [])].pop();
    const sagaLine = sagaRunning
      ? `<div class="device-saga">installing \u2014 step ${currentStep ? `${currentStep.index} of ${currentStep.total}: ${escapeHtml(currentStep.name)}` : "starting\u2026"}</div>`
      : saga?.state === "failed"
        ? `<div class="device-saga">the last ${escapeHtml(saga?.kind ?? "saga")} failed \u2014 see the log, or try again</div>`
        : "";

    let line = "";
    if (lc === "live") {
      line = row.last_data_s != null
        ? `on the stream \u00b7 last data ${row.last_data_s}s ago`
        : "on the stream";
    } else if (lc === "paused") {
      line = "off the stream - the face rests";
    } else if (!row.proto) {
      line = `pre-suzu firmware (${escapeHtml(row.version ?? "?")}) - not on the stream`;
    } else {
      line = "installed - joining the stream\u2026";
    }

    const streamButton = lc === "live"
      ? `<button class="ghost-button" data-action="pause" ${lock}>Pause</button>`
      : lc === "paused"
        ? `<button class="ghost-button" data-action="resume" ${lock}>Resume</button>`
        : `<button class="ghost-button" data-action="install" ${lock}>Install Firmware</button>`;
    const reinstall = `<button class="ghost-button" data-action="install" ${lock}>Reinstall Firmware</button>`;
    const factory = `<button class="danger-button" data-action="factory" ${lock}>Factory Reset</button>`;
    const tools = lc === "new" ? streamButton + factory : streamButton + reinstall + factory;

    const photo = row.class
      ? `<img class="device-photo" alt="" src="${API}/api/device-image/${encodeURIComponent(row.class)}" onerror="this.remove()">`
      : "";

    return `
      <article class="device-card" data-port="${escapeHtml(row.port)}">
        ${photo}
        <div class="device-body">
        <div class="device-head">
          <span class="chip ${row.streaming ? "on" : ""}"><span class="dot"></span>${escapeHtml(row.port)}</span>
          <span class="device-class">${escapeHtml(row.class ?? "unknown device")}</span>
          <span class="pill ${pillTone}">${pill}</span>
        </div>
        <div class="device-facts mono">${escapeHtml(row.family ?? "?")}/${escapeHtml(row.variant ?? "?")} v${escapeHtml(row.version ?? "?")}</div>
        <div class="device-admission">${line}</div>
        ${sagaLine}
        <div class="device-actions">${tools}</div>
        </div>
      </article>`;
  }

  async function pollStatus() {
    try {
      const d = await getJSON("/api/status");
      online = true;
      const devices = d.devices ?? [];
      const roster = new Map((d.roster ?? []).map((i) => [i.device_id, i]));
      el.state_word.textContent = paused ? "Paused" : "Running";
      el.state_facts.innerHTML =
        `<b>${devices.length}</b> face${devices.length === 1 ? "" : "s"} on the roster`;
      el.status_count.textContent = `${devices.length} device${devices.length === 1 ? "" : "s"}`;
      el.device_list.innerHTML = devices.map((row) =>
        deviceCard(row, roster.get(row.device_id ?? ""))).join("")
        || '<div class="empty">No faces on the bench — plug one in (data cable, not charge-only).</div>';
      renderWheel();
    } catch (e) {
      online = false;
      el.state_word.textContent = "Stopped";
      el.state_facts.textContent = "the Resident is not running";
      el.wheel.disabled = true;
    }
    renderService();
  }

  function renderService() {
    el.service.disabled = false;
    el.service.textContent = online ? "Stop service" : "Start service";
  }

  el.service?.addEventListener("click", async () => {
    el.service.disabled = true;
    try {
      if (online) {
        await call("POST", "/api/shutdown", {});
      } else {
        await window.__TAURI__.core.invoke("start_resident");
      }
    } catch (e) {
      toast(`the house did not answer: ${e.message ?? e}`);
    }
  });

  el.device_list?.addEventListener("click", async (event) => {
    const button = event.target.closest("button[data-action]");
    if (!button || button.disabled) return;
    const card = button.closest(".device-card");
    const port = card?.dataset.port;
    if (!port) return;
    const action = button.dataset.action;

    if (action === "pause" || action === "resume") {
      button.disabled = true;
      await postOrToast(`/api/device/${port}/${action}`, {});
    } else if (action === "install") {
      const ok = await confirmChange(
        `Install firmware on ${port}?`,
        "The face files return to ship state (its identity is kept), the face restarts, and it is tested before it goes live again.",
      );
      if (ok) {
        installing.add(port);
        pollStatus();
        await postOrToast(`/api/maintenance/${port}`, { kind: "install" });
      }
    } else if (action === "factory") {
      const ok = await confirmChange(
        `Factory reset ${port}?`,
        "Every flash cell is erased and the firmware is rebuilt from the vendored artifacts. The individual's identity is backed up first and restored after, and the face is tested before it goes live again.",
      );
      if (ok) {
        button.disabled = true;
        await postOrToast(`/api/maintenance/${port}`, { kind: "factory" });
        toast(`${port}: factory reset began - follow the steps here`);
      }
    }
  });

  async function postOrToast(path, body) {
    try {
      const r = await call("POST", path, body);
      const d = r.json();
      if (!r.ok) toast(d.error ?? `${path} failed`);
      return d;
    } catch (e) {
      toast(`the house did not answer: ${e.message}`);
      return {};
    }
  }

  // ── log ─────────────────────────────────────────────────────────
  function appendLogLine(domain, text, at) {
    if (activeView !== "log") return;
    const row = document.createElement("div");
    row.className = "stream-row";
    row.innerHTML = `<span class="row-when">${escapeHtml((at ?? "").slice(11))}</span>`
      + `<span class="row-domain">${escapeHtml(domain)}</span>`
      + `<span class="row-text">${escapeHtml(text)}</span>`;
    el.log_stream.prepend(row);
  }

  async function pollLog() {
    try {
      const lines = await getJSON("/api/log");
      el.log_count.textContent = `${lines.length} line${lines.length === 1 ? "" : "s"}`;
      el.log_stream.innerHTML = lines.map((l) =>
        `<div class="stream-row"><span class="row-when">${escapeHtml(l.ts.slice(11))}</span>`
        + `<span class="row-domain">${escapeHtml(l.domain)}</span>`
        + `<span class="row-text">${escapeHtml(l.text)}</span></div>`).join("")
        || '<div class="empty">Nothing has happened yet. Say something.</div>';
    } catch { /* offline: status already said so */ }
  }

  // ── media ───────────────────────────────────────────────────────
  async function startMedia() {
    stopMedia();
    let d;
    try { d = await getJSON("/api/status"); } catch { return; }
    const ports = (d.devices ?? []).map((row) => row.port);
    el.media_grid.innerHTML = ports.map((port) => `
      <article class="media-pane" data-port="${escapeHtml(port)}">
        <div class="device-head">
          <span class="chip on"><span class="dot"></span>${escapeHtml(port)}</span>
          <span class="media-age" id="age-${escapeHtml(port)}"></span>
        </div>
        <img class="media-frame" id="frame-${escapeHtml(port)}" alt="live frame from ${escapeHtml(port)}">
        <div class="device-actions">
          <button class="ghost-button" data-maction="shot">Screenshot</button>
          <button class="ghost-button" data-maction="record">Record 4s</button>
        </div>
      </article>`).join("")
      || '<div class="empty">No faces to watch.</div>';

    const tick = async () => {
      for (const port of ports) {
        const img = document.getElementById(`frame-${port}`);
        if (!img) continue;
        // <img> display is not CORS-gated: the shot loads straight from
      // the resident, and onload/onerror say which happened.
      const age = document.getElementById(`age-${port}`);
      img.onload = () => { if (age) age.textContent = `frame ${new Date().toLocaleTimeString()}`; };
      img.onerror = () => { if (age) age.textContent = "no shot — unreachable"; };
      img.src = `${API}/api/shot/${port}.png?scale=1&t=${Date.now()}`;
      }
      const rec = document.getElementById("media-note");
      try {
        const statuses = await Promise.all(ports.map((p) => getJSON(`/api/record/${p}`)));
        const running = statuses.filter((s) => s.phase === "recording");
        rec.hidden = running.length === 0;
        rec.textContent = `Recording ${running.map((s) => s.frames + " frames").join(", ")} — the panes show the frames the GIF is taking.`;
      } catch { rec.hidden = true; }
    };
    await tick();
    mediaTimer = setInterval(tick, 1200);
  }

  function stopMedia() {
    if (mediaTimer) { clearInterval(mediaTimer); mediaTimer = null; }
  }

  el.media_grid?.addEventListener("click", async (event) => {
    const button = event.target.closest("button[data-maction]");
    if (!button) return;
    const port = button.closest(".media-pane")?.dataset.port;
    if (!port) return;
    if (button.dataset.maction === "shot") {
      const d = await postOrToast(`/api/capture/${port}/save`, {});
      if (d.saved) toast(`saved ${d.saved}`);
    } else {
      await postOrToast(`/api/record/${port}`, { secs: 4, fps: 3 });
      toast(`${port}: recording — the pane shows what the GIF takes`);
    }
  });

  // ── about ───────────────────────────────────────────────────────
  async function renderAbout() {
    try {
      const d = await getJSON("/api/status");
      el.card_version.textContent = (d.resident?.version ?? "0.1").split(".").slice(0, 2).join(".");
      const faces = (d.devices ?? []).length;
      const facts = [
        ["Version", d.resident?.version ?? "unknown"],
        ["Faces", `${faces} connected`],
        ["Resident", paused ? "paused" : "running"],
        ["License", "MIT"],
      ];
      el.about_facts.innerHTML = facts
        .map(([t, v]) => `<dt>${escapeHtml(t)}</dt><dd>${escapeHtml(v)}</dd>`).join("");
    } catch {
      el.about_facts.innerHTML = "<dt>Resident</dt><dd>offline</dd>";
    }
    try {
      const groups = await getJSON("/api/destinations");
      const byGroup = new Map();
      for (const g of groups) {
        if (!byGroup.has(g.group)) byGroup.set(g.group, []);
        byGroup.get(g.group).push(g);
      }
      el.about_links.innerHTML = [...byGroup.entries()].map(([group, items]) =>
        `<section class="about-group"><h2>${escapeHtml(group)}</h2><div>${
          items.map((g) =>
            `<button class="about-link" type="button" data-url="${escapeHtml(g.url)}">`
            + `<span class="about-link-title">${escapeHtml(g.title)}</span>`
            + `<span class="about-link-blurb">${escapeHtml(g.blurb)}</span></button>`).join("")
        }</div></section>`).join("");
    } catch { /* the card alone is enough when the house sleeps */ }
  }

  el.about_links?.addEventListener("click", (event) => {
    const button = event.target.closest("button.about-link");
    if (!button) return;
    window.__TAURI__?.core?.invoke("open_destination", { url: button.dataset.url })
      ?.catch((e) => toast(`refused: ${e}`));
  });

  // ── confirm dialog ──────────────────────────────────────────────
  el.confirm_dialog?.addEventListener("click", (event) => {
    const answer = event.target.dataset?.confirm;
    if (!answer) return;
    el.confirm_dialog.hidden = true;
    confirmResolve?.(answer === "proceed");
    confirmResolve = null;
  });

  // ── the pulse ───────────────────────────────────────────────────
  // The live wire: the house's facts arrive as they happen, and the
  // roster re-renders within a beat of each one.
  let pollQueued = false;
  // The announcement pipeline: one stream, routed by type. Roster
  // facts refresh the cards; rings and saga steps flow to the Log.
  const ROSTER_EVENTS = new Set([
    "device_minded", "device_released", "device_homecoming",
    "individual_held", "admission_report", "stream_attached",
    "stream_detached", "maintenance_started", "maintenance_step",
    "maintenance_completed",
  ]);
  if (window.__TAURI__?.event?.listen) {
    window.__TAURI__.event.listen("house", (e) => {
      let msg;
      try { msg = JSON.parse(e.payload); } catch { return; }
      if (ROSTER_EVENTS.has(msg.type) && !pollQueued) {
        pollQueued = true;
        setTimeout(() => { pollQueued = false; pollStatus(); }, 150);
      }
      if (activeView === "log" && msg.text) {
        appendLogLine(msg.domain, msg.text, msg.at);
      }
    });
  }

  await pollStatus();
  await renderAbout();
  renderWheel();
  setInterval(pollStatus, 10000); // the safety net; announcements do the work
  setInterval(() => { if (activeView === "log") pollLog(); }, 10000);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) stopMedia();
    else if (activeView === "media") startMedia();
  });

  // the surface is up: let the shell show it
  window.__TAURI__?.core?.invoke("ready")?.catch?.(() => {});
})();
