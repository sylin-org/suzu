//! Suzu workbench — the keeper's window. Everything it shows comes
//! from the Resident's loopback API; it invents nothing.

(async () => {
  "use strict";

  const API = "http://127.0.0.1:7899";
  const el = {};
  for (const id of [
    "lamp", "state-word", "state-facts", "wheel", "wheel-label", "wheel-icon",
    "status-count", "device-list", "log-count", "log-stream",
    "media-grid", "media-note", "about-facts", "about-links", "card-version",
    "toast", "confirm-dialog", "confirm-title", "confirm-detail",
  ]) {
    el[id.replaceAll("-", "_")] = document.getElementById(id);
  }

  let paused = false;
  let activeView = "status";
  let mediaTimer = null;
  let confirmResolve = null;

  const escapeHtml = (s) => String(s).replace(/[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  async function getJSON(path) {
    const r = await fetch(API + path);
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
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
      const r = await fetch(`${API}/api/control`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ verb }),
      });
      if (!r.ok) throw new Error(await r.text());
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
    const lifecycle = row.lifecycle ?? "unknown";
    const tone = lifecycle === "streaming" ? "good" : (lifecycle === "convalescing" ? "warn" : "info");
    const admission = rosterEntry?.admission;
    const admissionLine = admission
      ? `admission ${admission.passed ? "passed" : "FAILED"} · ${admission.steps.map((s) => s.name).join(", ")}`
      : "no admission verdict yet";
    const saga = rosterEntry?.maintenance;
    const sagaLine = saga
      ? `<div class="device-saga">${escapeHtml(saga.kind)} saga · ${escapeHtml(saga.state)} · ${escapeHtml(saga.steps.map((s) => s.name).join(" → ") || "starting")}</div>`
      : "";
    const maintenance = lifecycle === "undermaintenance";
    return `
      <article class="device-card" data-port="${escapeHtml(row.port)}">
        <div class="device-head">
          <span class="chip ${row.streaming ? "on" : ""}"><span class="dot"></span>${escapeHtml(row.port)}</span>
          <span class="device-class">${escapeHtml(row.class ?? "no class")}</span>
          <span class="pill ${tone}">${escapeHtml(lifecycle)}</span>
        </div>
        <div class="device-facts mono">
          ${escapeHtml(row.family ?? "?")}/${escapeHtml(row.variant ?? "?")} v${escapeHtml(row.version ?? "?")}
          · ${escapeHtml(row.proto ?? "no proto")}
          · id <b>${escapeHtml((row.device_id ?? "?").slice(0, 13))}…</b>
        </div>
        <div class="device-admission">${escapeHtml(admissionLine)}</div>
        ${sagaLine}
        <div class="device-actions">
          <button class="ghost-button" data-action="admission" ${maintenance ? "disabled" : ""}>Test again</button>
          <button class="ghost-button" data-action="shot" ${maintenance ? "disabled" : ""}>Screenshot</button>
          <button class="ghost-button" data-action="record" ${maintenance ? "disabled" : ""}>Record 4s</button>
          <button class="ghost-button" data-action="soft" ${maintenance ? "disabled" : ""}>Reinstall (soft)</button>
          <button class="danger-button" data-action="factory" ${maintenance ? "disabled" : ""}>Factory wipe</button>
        </div>
      </article>`;
  }

  async function pollStatus() {
    try {
      const d = await getJSON("/api/status");
      paused = d.paused === true;
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
      el.state_word.textContent = "Offline";
      el.state_facts.textContent = "the Resident is not answering";
      el.wheel.disabled = true;
    }
  }

  el.device_list?.addEventListener("click", async (event) => {
    const button = event.target.closest("button[data-action]");
    if (!button || button.disabled) return;
    const card = button.closest(".device-card");
    const port = card?.dataset.port;
    if (!port) return;
    const action = button.dataset.action;

    if (action === "admission") {
      button.disabled = true;
      await postOrToast(`/api/admission/${port}`, {});
      toast(`${port}: the exam runs — its verdict lands on the log`);
    } else if (action === "shot") {
      button.disabled = true;
      try {
        const r = await fetch(`${API}/api/capture/${port}/save`, { method: "POST" });
        const d = await r.json();
        if (d.saved) toast(`saved ${d.saved}`);
        else toast(`no shot: ${d.error ?? "unknown"}`);
      } catch (e) { toast(`no shot: ${e.message}`); }
      button.disabled = false;
    } else if (action === "record") {
      button.disabled = true;
      await postOrToast(`/api/record/${port}`, { secs: 4, fps: 3 });
    } else if (action === "soft" || action === "factory") {
      const kind = action;
      const ok = await confirmChange(
        kind === "factory"
          ? `Factory wipe ${port}?`
          : `Reinstall the face on ${port}?`,
        kind === "factory"
          ? "Every flash cell is erased, the runtime is rebuilt, and the face must pass its admission test before it streams again. The individual's identity is backed up first and restored after."
          : "The face files return to ship state and the face re-runs its admission test. The runtime is untouched.",
      );
      if (ok) {
        button.disabled = true;
        await postOrToast(`/api/maintenance/${port}`, { kind });
        toast(`${port}: the ${kind} saga began — follow it here`);
      }
    }
  });

  async function postOrToast(path, body) {
    try {
      const r = await fetch(API + path, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const d = await r.json().catch(() => ({}));
      if (!r.ok) toast(d.error ?? `${path} failed`);
      return d;
    } catch (e) {
      toast(`the house did not answer: ${e.message}`);
      return {};
    }
  }

  // ── log ─────────────────────────────────────────────────────────
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
        try {
          const r = await fetch(`${API}/api/shot/${port}.png`, { cache: "no-store" });
          const age = document.getElementById(`age-${port}`);
          if (r.ok) {
            const blob = await r.blob();
            const url = URL.createObjectURL(blob);
            img.src = url;
            if (age) age.textContent = `frame ${new Date().toLocaleTimeString()}`;
          } else if (age) {
            age.textContent = "no shot — unreachable";
          }
        } catch { /* one miss is not a story */ }
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
  await pollStatus();
  await renderAbout();
  renderWheel();
  setInterval(pollStatus, 2000);
  setInterval(() => { if (activeView === "log") pollLog(); }, 2000);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) stopMedia();
    else if (activeView === "media") startMedia();
  });

  // the surface is up: let the shell show it
  window.__TAURI__?.core?.invoke("ready")?.catch?.(() => {});
})();
