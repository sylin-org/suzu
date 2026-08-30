//! Suzu workbench — the keeper's window.
//!
//! ADR-0004 is the law here: one store, views as pure functions from
//! store slices to DOM. No event handler writes state into the page;
//! a handler only sends a command and lets the wire's own facts
//! re-render the truth. Nothing is polled, nothing is patched in,
//! nothing races a timer.

(async () => {
  "use strict";

  const Store = window.SuzuStore;
  const el = {};
  for (const id of [
    "state-word", "state-facts", "wheel", "wheel-label", "wheel-icon",
    "status-count", "device-list", "log-count", "log-stream",
    "media-grid", "media-note", "about-facts", "about-links", "card-version", "service",
    "toast", "confirm-dialog", "confirm-title", "confirm-detail",
  ]) {
    el[id.replaceAll("-", "_")] = document.getElementById(id);
  }

  const API = "http://127.0.0.1:7899";
  const tauri = window.__TAURI__;

  let activeView = "status";
  let serviceBusy = false; // declared UI state: a start/stop is in flight
  let confirmResolve = null;
  const installing = new Set(); // ports with a saga this window began, before its roster fact lands

  const escapeHtml = (s) => String(s).replace(/[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

  // The Rust shell speaks to the Resident; the webview never makes a
  // cross-origin request, so CORS does not exist in this product.
  // The shell's door takes the body as a string: JSON is written at
  // this boundary, once, so every command may speak plain objects.
  async function call(method, path, body) {
    const payload = body == null ? null
      : typeof body === "string" ? body : JSON.stringify(body);
    const r = await tauri.core.invoke("api", { method, path, body: payload });
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
  async function postOrToast(path, body) {
    try {
      const r = await call("POST", path, body);
      const d = r.json();
      if (!r.ok) toast(d.error ?? `${path} failed`);
      return d;
    } catch (e) {
      toast(`the house did not answer: ${e.message ?? e}`);
      return {};
    }
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

  // ── navigation — client UI state, not house state ────────────────
  function setView(name) {
    activeView = name;
    document.querySelectorAll(".tab").forEach((t) => {
      const on = t.dataset.view === name;
      t.classList.toggle("active", on);
      if (on) t.setAttribute("aria-current", "page");
      else t.removeAttribute("aria-current");
    });
    document.querySelectorAll(".view").forEach((v) =>
      v.classList.toggle("active", v.dataset.page === name));
    renderView();
  }
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => setView(tab.dataset.view));
  });

  // ── the lampband: stream health and the pause flag ───────────────
  function renderChrome() {
    const { stream, service, devices } = Store.state;
    const connected = stream === "connected";
    document.body.classList.toggle("runtime-held", connected && service.paused);
    el.state_word.textContent = !connected
      ? (stream === "connecting" ? "Starting" : "Stopped")
      : service.paused ? "Paused" : "Running";
    el.state_facts.innerHTML = connected
      ? `<b>${devices.size}</b> ${plural(devices.size, "face")} on the roster`
      : "the Resident is not running";
    el.service.textContent = connected ? "Stop service" : "Start service";
    el.service.disabled = serviceBusy;
    renderWheel();
  }

  function renderWheel() {
    const paused = Store.state.service.paused;
    el.wheel.disabled = Store.state.stream !== "connected";
    el.wheel.dataset.intent = paused ? "resume" : "pause";
    el.wheel_label.textContent = paused ? "Resume" : "Pause";
    el.wheel_icon.innerHTML = paused
      ? '<path d="M8 5v14l11-7z"/>'
      : '<rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/>';
  }

  el.wheel.addEventListener("click", () => {
    const verb = Store.state.service.paused ? "resume" : "pause";
    postOrToast("/api/control", { verb });
    // the Paused fact re-renders the wheel — no local truth
  });

  el.service.addEventListener("click", async () => {
    if (serviceBusy) return;
    serviceBusy = true;
    renderChrome();
    const connected = Store.state.stream === "connected";
    try {
      if (connected) {
        const res = await tauri.core.invoke("stop_resident");
        if (!res.stopped) toast(`the door is still held: ${res.reason}`);
      } else {
        const msg = await tauri.core.invoke("start_resident");
        toast(msg);
      }
    } catch (e) {
      toast(String(e));
    }
    serviceBusy = false;
    renderChrome();
  });

  // ── status: the roster, pure from its slices ─────────────────────
  function sortedDevices() {
    return [...Store.state.devices.values()].sort((a, b) => a.port.localeCompare(b.port));
  }

  function deviceCard(row) {
    const rosterEntry = Store.state.roster.get(row.device_id ?? "");
    // The keeper's formula: a device is LIVE, NEW, or PAUSED — and the
    // buttons are exactly the ones its state offers, nothing else.
    const saga = rosterEntry?.maintenance;
    const sagaRunning = saga?.state === "running";
    const isInstalling = sagaRunning || (installing.has(row.port) && !saga);
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

    // Class photos come straight from the resident (img display is
    // not CORS-gated): one URL per class, remembered; a class with no
    // declared image stores null and shows no broken frame.
    let photoUrl = row.class ? Store.state.photos.get(row.class) : undefined;
    if (row.class && photoUrl === undefined) {
      photoUrl = `${API}/api/device-image/${encodeURIComponent(row.class)}`;
      Store.putPhoto(row.class, photoUrl);
    }
    const photo = photoUrl
      ? `<img class="device-photo" alt="" data-class="${escapeHtml(row.class)}" src="${escapeHtml(photoUrl)}">`
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

  function renderStatus() {
    const devices = sortedDevices();
    const { stream } = Store.state;
    el.status_count.textContent = plural(devices.length, "device");
    el.device_list.innerHTML = devices.map(deviceCard).join("")
      || (stream === "connecting"
        ? '<div class="empty">Waiting for the house to speak\u2026</div>'
        : stream !== "connected"
          ? '<div class="empty">The Resident is not running \u2014 start the service above.</div>'
          : '<div class="empty">No faces on the bench — plug one in (data cable, not charge-only).</div>');
    // A photo that cannot load (no declared image, or the house was
    // down) is remembered as null — the frame steps aside honestly.
    el.device_list.querySelectorAll("img.device-photo").forEach((img) => {
      img.onerror = () => Store.putPhoto(img.dataset.class, null);
    });
  }

  el.device_list.addEventListener("click", async (event) => {
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
        "The face files return to ship state (its identity is kept), the face restarts, and it is tested before it goes live again. If the board is not running CircuitPython yet, the saga waits for you to hold BOOTSEL and replug \u2014 every step appears in the Log.",
      );
      if (ok) {
        const d = await postOrToast(`/api/maintenance/${port}`, { kind: "install" });
        if (d.started) installing.add(port);
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

  // ── log: the journal, newest first ───────────────────────────────
  // The house's own loudness, colored by the stylesheet's severities:
  // degraded lines and failed steps are bad; a passed admission is good.
  const toneOf = (l) =>
    l.text.includes("!!") || l.text.includes("\u2717") ? "bad"
      : l.text.includes("PASSED") ? "good" : "";

  function renderLog() {
    const lines = Store.journalLines();
    el.log_count.textContent = plural(lines.length, "line");
    el.log_stream.innerHTML = lines.map((l) => {
      const tone = toneOf(l);
      return `<div class="stream-row"${tone ? ` data-tone="${tone}"` : ""}>`
        + `<span class="row-when">${escapeHtml(l.ts.slice(11))}</span>`
        + `<span class="row-domain">${escapeHtml(l.domain)}</span>`
        + `<span class="line">${escapeHtml(l.text)}</span></div>`;
    }).join("")
      || '<div class="empty">Nothing has happened yet. Say something.</div>';
  }

  // ── media: frames read from the store, never commanded ───────────
  function renderMedia() {
    // Only faces the stream actually reaches get a tile: a New or
    // paused face can never blink, so a pane for it would be a lie.
    const devices = sortedDevices().filter((row) => row.streaming);
    const { stream, service } = Store.state;
    el.media_grid.innerHTML = devices.map(mediaPane).join("")
      || (stream !== "connected"
        ? '<div class="empty">The house is not speaking \u2014 start the service above.</div>'
        : service.paused
          ? '<div class="empty">The stream is paused \u2014 resume it to watch the faces.</div>'
          : '<div class="empty">No faces to watch.</div>');
    const recording = [...Store.state.jobs.values()]
      .filter((j) => j.kind === "record" && j.state === "recording");
    el.media_note.hidden = recording.length === 0;
    if (recording.length) {
      el.media_note.textContent =
        `Recording ${recording.map((j) => `${j.target}: ${j.index}/${j.total} frames`).join(", ")}`
        + " \u2014 the panes show the frames the GIF is taking.";
    }
  }

  function mediaPane(row) {
    const frame = Store.state.frames.get(row.port);
    const recording = [...Store.state.jobs.values()].some((j) =>
      j.kind === "record" && j.state === "recording" && j.target === row.port);
    const img = frame
      ? `<img class="media-frame" alt="live frame from ${escapeHtml(row.port)}" src="${escapeHtml(frame.png)}">`
      : '<span class="media-frame" aria-hidden="true"></span>';
    const age = frame
      ? `frame ${new Date(frame.at).toLocaleTimeString()}`
      : row.streaming
        ? "waiting for the first blink\u2026"
        : "the face is not on the stream";
    return `
      <article class="media-pane" data-port="${escapeHtml(row.port)}">
        <div class="device-head">
          <span class="chip ${row.streaming ? "on" : ""}"><span class="dot"></span>${escapeHtml(row.port)}</span>
          <span class="media-age">${escapeHtml(age)}</span>
        </div>
        ${img}
        <div class="device-actions">
          <button class="ghost-button" data-maction="shot">Screenshot</button>
          <button class="ghost-button" data-maction="record"${recording ? " disabled" : ""}>Record 4s</button>
        </div>
      </article>`;
  }

  el.media_grid.addEventListener("click", async (event) => {
    const button = event.target.closest("button[data-maction]");
    if (!button) return;
    const port = button.closest(".media-pane")?.dataset.port;
    if (!port) return;
    if (button.dataset.maction === "shot") {
      const d = await postOrToast(`/api/capture/${port}/save`, {});
      if (d.saved) toast(`saved ${d.saved}`);
    } else {
      const d = await postOrToast(`/api/record/${port}`, { secs: 4, fps: 3 });
      if (d.started) toast(`${port}: recording — the pane shows what the GIF takes`);
    }
  });

  // ── about: the published card ────────────────────────────────────
  function renderAbout() {
    const { service, devices } = Store.state;
    const connected = Store.state.stream === "connected";
    el.card_version.textContent = (service.version ?? "0.1").split(".").slice(0, 2).join(".");
    const facts = [
      ["Version", connected ? (service.version ?? "unknown") : "\u2014"],
      ["Faces", connected ? `${devices.size} connected` : "\u2014"],
      ["Resident", connected ? (service.paused ? "paused" : "running") : "offline"],
      ["License", "MIT"],
    ];
    el.about_facts.innerHTML = facts
      .map(([t, v]) => `<dt>${escapeHtml(t)}</dt><dd>${escapeHtml(v)}</dd>`).join("");
    renderLinks();
  }

  function renderLinks() {
    const groups = Store.state.links;
    if (!groups) return;
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
  }

  async function fetchLinks() {
    try {
      Store.putLinks(await getJSON("/api/destinations"));
    } catch { /* the card alone is enough when the house sleeps */ }
  }

  el.about_links.addEventListener("click", (event) => {
    const button = event.target.closest("button.about-link");
    if (!button) return;
    tauri?.core?.invoke("open_destination", { url: button.dataset.url })
      ?.catch((e) => toast(`refused: ${e}`));
  });

  // ── confirm dialog ───────────────────────────────────────────────
  el.confirm_dialog.addEventListener("click", (event) => {
    const answer = event.target.dataset?.confirm;
    if (!answer) return;
    el.confirm_dialog.hidden = true;
    confirmResolve?.(answer === "proceed");
    confirmResolve = null;
  });

  // ── the store → the views ────────────────────────────────────────
  const SLICE_ROUTES = {
    status: ["devices", "roster", "jobs", "photos", "stream", "service"],
    log: ["journal"],
    media: ["frames", "jobs", "devices", "stream", "service"],
    about: ["service", "devices", "links", "stream"],
  };

  function renderView() {
    if (activeView === "status") renderStatus();
    else if (activeView === "log") renderLog();
    else if (activeView === "media") renderMedia();
    else if (activeView === "about") renderAbout();
  }

  Store.subscribe((slices) => {
    renderChrome();
    // The house just answered: re-ask anything that needed it alive.
    if (slices.includes("stream") && Store.state.stream === "connected") {
      Store.retryPhotos();
      if (!Store.state.links) fetchLinks();
    }
    const routes = SLICE_ROUTES[activeView] ?? [];
    if (slices.some((s) => routes.includes(s))) renderView();
  });

  // ── first paint: the truth arrives on the wire ───────────────────
  setView("status");
  fetchLinks();

  // the surface is up: let the shell show it
  tauri?.core?.invoke("ready")?.catch?.(() => {});
})();
