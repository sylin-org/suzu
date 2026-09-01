//! Suzu Workbench client interface.
//!
//! ADR-0004 defines one store and views as pure functions from
//! store slices to DOM. No event handler writes state into the page;
//! a handler only sends a command and lets stream events
//! re-render the interface. Nothing is polled or patched directly,
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
    "fp-dialog", "fp-title", "fp-detail", "fp-cards",
  ]) {
    el[id.replaceAll("-", "_")] = document.getElementById(id);
  }

  const API = "http://127.0.0.1:7899";
  const tauri = window.__TAURI__;

  let activeView = "status";
  let serviceBusy = false; // declared UI state: a start/stop is in flight
  let confirmResolve = null;

  const escapeHtml = (s) => String(s).replace(/[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

  // The Rust shell communicates with the Resident; the webview never makes a
  // cross-origin request, so CORS does not exist in this product.
  // The shell command accepts the body as a string. Serialize JSON at
  // this boundary so callers can pass plain objects.
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
      if (!r.ok) toast(d.message ?? `${path} failed (${r.status})`);
      return d;
    } catch (e) {
      toast(`the Resident did not respond: ${e.message ?? e}`);
      return {};
    }
  }
  function deviceAction(port, action, body = {}) {
    const verb = action === "factory_reset" ? "factory-reset" : action;
    return postOrToast(`/api/device/${port}/${verb}`, body);
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

  // ── the faceplate chooser (ADR-0005) ─────────────────────────────
  // One confirmation flow handles install, reinstall, and faceplate changes. When the class declares
  // faceplates, the dialog shows them — captured previews where they
  // exist, pictogram and words where they don't. When none are
  // declared, it is exactly the plain confirm it always was.
  const MOUNT_CAPTIONS = {
    "usb-down": "display upright — connector at the bottom",
    "usb-up": "display inverted — connector at the top",
    "usb-left": "left-mounted — connector at the left",
    "usb-right": "right-mounted — connector at the right",
  };

  // Render the connector edge from the mount declaration instead of
  // maintaining separate art for each faceplate.
  function mountPictogram(mount) {
    const stub = {
      "usb-down": '<rect x="26" y="27" width="12" height="3" rx="1"/>',
      "usb-up": '<rect x="26" y="0" width="12" height="3" rx="1"/>',
      "usb-left": '<rect x="0" y="13" width="3" height="12" rx="1"/>',
      "usb-right": '<rect x="61" y="13" width="3" height="12" rx="1"/>',
    }[mount] || "";
    return '<svg width="52" height="25" viewBox="0 0 64 30" fill="currentColor" aria-hidden="true">'
      + '<rect x="4" y="3" width="56" height="24" rx="4" fill="none" stroke="currentColor" stroke-width="1.5"/>'
      + '<rect x="16" y="8" width="32" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1"/>'
      + stub + "</svg>";
  }

  function faceplateCard(fp, selected) {
    const img = fp.preview
      ? `<img class="fp-preview" alt="" src="${API}${escapeHtml(fp.preview)}">`
      : "";
    return `<button type="button" class="fp-card${selected ? " selected" : ""}" data-fpid="${escapeHtml(fp.id)}">`
      + `${img}<span class="fp-name">${escapeHtml(fp.name)}</span>`
      + `<div class="fp-blurb">${escapeHtml(fp.blurb ?? "")}</div>`
      + `<div class="fp-mount">${mountPictogram(fp.mount)}<span>${escapeHtml(MOUNT_CAPTIONS[fp.mount] ?? "")}</span></div>`
      + "</button>";
  }

  async function fetchFaceplates(classId) {
    if (!classId || Store.state.faceplates.has(classId)) return;
    Store.state.faceplates.set(classId, []); // in-flight marker
    try {
      Store.putFaceplates(classId, await getJSON(`/api/faceplates/${encodeURIComponent(classId)}`));
    } catch {
      Store.putFaceplates(classId, []); // the words still work; retried on reconnect
    }
  }

  let fpResolve = null;
  let fpChoice = null;

  function confirmFaceplateChange(classId, title, detail) {
    const list = Store.state.faceplates.get(classId) ?? [];
    if (list.length === 0) {
      return confirmChange(title, detail).then((ok) => ({ ok, faceplate: null }));
    }
    el.fp_title.textContent = title;
    el.fp_detail.textContent = detail;
    el.fp_cards.innerHTML = list.map((fp, i) => faceplateCard(fp, i === 0)).join("");
    fpChoice = list[0].id;
    el.fp_dialog.hidden = false;
    return new Promise((resolve) => {
      fpResolve = (proceed) => resolve(proceed ? { ok: true, faceplate: fpChoice } : { ok: false, faceplate: null });
    });
  }

  el.fp_cards.addEventListener("click", (event) => {
    const card = event.target.closest(".fp-card");
    if (!card) return;
    fpChoice = card.dataset.fpid;
    el.fp_cards.querySelectorAll(".fp-card").forEach((c) => c.classList.toggle("selected", c === card));
  });

  el.fp_dialog.addEventListener("click", (event) => {
    const answer = event.target.dataset?.fp;
    if (!answer) return;
    el.fp_dialog.hidden = true;
    fpResolve?.(answer === "proceed");
    fpResolve = null;
  });

  // ── media subscription (ADR-0004 amendment) ──────────────────────
  // Entering Media asserts the watch; leaving arms a 10-second
  // delay. Returning within the delay keeps the subscription active.
  // If a Resident restart clears the flag while Media is open, the
  // client subscribes again.
  const MEDIA_LINGER_MS = 10000;
  let mediaLinger = null;

  function watchMedia(on) {
    call("POST", "/api/ui", { watch_media: on ? "on" : "off" }).catch(() => {});
  }

  function armMediaLinger() {
    clearTimeout(mediaLinger);
    mediaLinger = setTimeout(() => {
      mediaLinger = null;
      watchMedia(false);
    }, MEDIA_LINGER_MS);
  }

  // ── navigation: client-only UI state ──────────────────────────────
  function setView(name) {
    activeView = name;
    if (name === "media") {
      clearTimeout(mediaLinger);
      mediaLinger = null;
      watchMedia(true);
    } else if (Store.state.mediaWatched) {
      armMediaLinger();
    }
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

  // ── header: stream health and pause state ─────────────────────────
  function renderChrome() {
    const { stream, service, devices } = Store.state;
    const connected = stream === "connected";
    document.body.classList.toggle("runtime-paused", connected && service.paused);
    el.state_word.textContent = !connected
      ? (stream === "connecting" ? "Starting" : "Stopped")
      : service.paused ? "Paused" : "Running";
    el.state_facts.innerHTML = connected
      ? `<b>${devices.size}</b> connected ${plural(devices.size, "device")}`
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
    // The paused event updates the button through the store.
  });

  el.service.addEventListener("click", async () => {
    if (serviceBusy) return;
    serviceBusy = true;
    renderChrome();
    const connected = Store.state.stream === "connected";
    try {
      if (connected) {
        const res = await tauri.core.invoke("stop_resident");
        if (!res.stopped) toast(`the Resident is still running: ${res.reason}`);
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

  // ── device status ─────────────────────────────────────────────────
  function sortedDevices() {
    return [...Store.state.devices.values()].sort((a, b) => a.port.localeCompare(b.port));
  }

  function deviceCard(row) {
    const rosterEntry = Store.state.roster.get(row.device_id ?? "");
    // Display the lifecycle reported by the Resident and only the
    // actions explicitly offered for that state. A running maintenance
    // procedure temporarily displays INSTALLING.
    const maintenance = rosterEntry?.maintenance;
    const maintenanceRunning = maintenance?.state === "running";
    const lc = maintenanceRunning ? "installing" : (row.lifecycle ?? "new");
    const pill = maintenanceRunning
      ? "INSTALLING"
      : { live: "LIVE", new: "NEW", paused: "PAUSED" }[lc] ?? escapeHtml(lc.toUpperCase());
    const pillTone = maintenanceRunning ? "warn" : ({ live: "good", new: "warn", paused: "info" }[lc] ?? "info");
    const lock = maintenanceRunning ? "disabled" : "";
    const currentStep = [...(maintenance?.steps ?? [])].pop();
    const maintenanceLine = maintenanceRunning
      ? `<div class="device-maintenance">installing \u2014 step ${currentStep ? `${currentStep.index} of ${currentStep.total}: ${escapeHtml(currentStep.name)}` : "starting\u2026"}</div>`
      : maintenance?.state === "failed"
        ? `<div class="device-maintenance">the last ${escapeHtml(maintenance?.kind ?? "maintenance procedure")} failed \u2014 see the log, or try again</div>`
        : "";

    let line = "";
    if (lc === "live") {
      line = row.last_data_s != null
        ? `on the stream \u00b7 last data ${row.last_data_s}s ago`
        : "on the stream";
    } else if (lc === "paused") {
      line = "paused - not receiving updates";
    } else if (!row.proto) {
      line = `not installed - not on the stream`;
    } else {
      line = "installed - joining the stream\u2026";
    }

    // If the installed faceplate is older than the declaration, show
    // the admission failure and offer an update.
    const versionStep = rosterEntry?.admission?.steps?.find((s) => s.name === "faceplate-version");
    const stale = lc === "new" && versionStep && !versionStep.ok;
    if (stale) line = escapeHtml(versionStep.detail);

    // The aggregate publishes its legal verbs. This view chooses labels
    // and order; lifecycle rules remain in the Resident.
    const allows = (action) => (row.actions ?? []).includes(action);
    const tools = [
      allows("pause") ? `<button class="ghost-button" data-action="pause" ${lock}>Pause</button>` : "",
      allows("resume") ? `<button class="ghost-button" data-action="resume" ${lock}>Resume</button>` : "",
      allows("identify") ? `<button class="ghost-button" data-action="identify" ${lock}>Identify</button>` : "",
      allows("update") && lc === "new"
        ? `<button class="ghost-button" data-action="update" ${lock}>Update Faceplate</button>`
        : allows("update")
          ? `<button class="ghost-button" data-action="faceplate" ${lock}>Faceplate…</button>` : "",
      allows("install")
        ? `<button class="ghost-button" data-action="install" ${lock}>${lc === "new" ? "Install Firmware" : "Reinstall Firmware"}</button>` : "",
      allows("factory_reset") ? `<button class="danger-button" data-action="factory" ${lock}>Factory Reset</button>` : "",
    ].join("");

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
        ${maintenanceLine}
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
        ? '<div class="empty">Waiting for the Resident\u2026</div>'
        : stream !== "connected"
          ? '<div class="empty">The Resident is not running \u2014 start the service above.</div>'
          : '<div class="empty">No devices connected. Use a data-capable USB cable.</div>');
    // Cache failed class photos as null to avoid repeatedly loading a missing image.
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
      await deviceAction(port, action);
    } else if (action === "install") {
      const row = Store.state.devices.get(port);
      await fetchFaceplates(row?.class);
      const c = await confirmFaceplateChange(
        row?.class,
        `Install firmware on ${port}?`,
        "The device files are restored to the packaged state while preserving its identity. The device then restarts and runs admission tests. If CircuitPython is not installed, follow the BOOTSEL instructions shown in the Log.",
      );
      if (!c.ok) return;
      const body = {};
      if (c.faceplate) body.faceplate = c.faceplate;
      const d = await deviceAction(port, "install", body);
      if (d.message) toast(d.message);
    } else if (action === "identify") {
      button.disabled = true;
      // the utterance is the request: identify device COM24
      try {
        const d = await deviceAction(port, "identify");
        if (d.message) toast(d.message);
      } finally {
        // Identify does not change device state, so restore the button here.
        button.disabled = false;
      }
    } else if (action === "update") {
      button.disabled = true;
      // Update the faceplate files on the existing MicroPython installation.
      const d = await deviceAction(port, "update");
      if (d.message) toast(d.message);
    } else if (action === "faceplate") {
      const row = Store.state.devices.get(port);
      await fetchFaceplates(row?.class);
      const c = await confirmFaceplateChange(
        row?.class,
        `Change the faceplate on ${port}?`,
        "The faceplate files are rewritten in place, then the device restarts and runs admission tests. No bootloader step is required.",
      );
      if (!c.ok) return;
      const body = {};
      if (c.faceplate) body.faceplate = c.faceplate;
      const d = await deviceAction(port, "update", body);
      if (d.message) toast(d.message);
    } else if (action === "factory") {
      const ok = await confirmChange(
        `Factory reset ${port}?`,
        "The flash is erased and firmware is restored from packaged artifacts. Device identity is backed up first, restored afterward, and verified by admission tests.",
      );
      if (ok) {
        button.disabled = true;
        await deviceAction(port, "factory_reset");
        toast(`${port}: factory reset started; follow the Log for progress`);
      }
    }
  });

  // ── log: the journal, newest first ───────────────────────────────
  // Derive log severity from maintenance and admission result markers.
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
      || '<div class="empty">No events recorded.</div>';
  }

  // ── media: frames read from the store, never commanded ───────────
  function renderMedia() {
    // Show previews only for devices currently streaming frames.
    const devices = sortedDevices().filter((row) => row.streaming);
    const { stream, service } = Store.state;
    el.media_grid.innerHTML = devices.map(mediaPane).join("")
      || (stream !== "connected"
        ? '<div class="empty">The Resident is not running \u2014 start the service above.</div>'
        : service.paused
          ? '<div class="empty">The stream is paused \u2014 resume it to view device updates.</div>'
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
        ? "waiting for the first frame\u2026"
        : "the device is not streaming";
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
      if (d.saved) toast(d.message);
    } else {
      const d = await postOrToast(`/api/record/${port}`, { secs: 4, fps: 3 });
      if (d.message) toast(d.message);
    }
  });

  // ── product information ───────────────────────────────────────────
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
    } catch { /* Keep the static product information when the Resident is unavailable. */ }
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
    // Retry resources that require a running Resident after reconnecting.
    if (slices.includes("stream") && Store.state.stream === "connected") {
      Store.retryPhotos();
      if (!Store.state.links) fetchLinks();
      Store.resetFaceplates(); // Reload declarations after reconnecting.
    }
    // A fresh snapshot that says unwatched while Media is open means
    // the flag was reset under us (resident restart) — assert again.
    if (slices.includes("media") && activeView === "media" && !Store.state.mediaWatched) {
      watchMedia(true);
    }
    const routes = SLICE_ROUTES[activeView] ?? [];
    if (slices.some((s) => routes.includes(s))) renderView();
  });

  // ── initial render ────────────────────────────────────────────────
  setView("status");
  fetchLinks();

  // When hidden to the tray, release the media subscription after the delay.
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      if (activeView === "media" && Store.state.mediaWatched) armMediaLinger();
    } else if (activeView === "media") {
      clearTimeout(mediaLinger);
      mediaLinger = null;
      watchMedia(true);
    }
  });

  // Notify the shell that the initial UI is ready.
  tauri?.core?.invoke("ready")?.catch?.(() => {});
})();
