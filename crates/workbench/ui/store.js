//! The store — the client's one mutable state (ADR-0004).
//!
//! Every fact the window shows arrived over the wire. Collections are
//! mutated only by typed reducers, one per fact kind; slices replaced
//! whole where the house replaces them whole, keyed where it keys.
//! Stream health is itself store state. Views subscribe; a change
//! re-renders the views that read the slice that changed. There is
//! nothing to poll, because the truth arrives.

(() => {
  "use strict";

  const listeners = new Set();
  const dirty = new Set();
  let notifyQueued = false;

  const state = {
    /// connecting · connected · reconnecting — the wire's own health
    stream: "connecting",
    service: { name: "suzu", version: null, paused: false },
    devices: new Map(), // port → device row
    roster: new Map(),  // device_id → individual
    jobs: new Map(),    // id → job
    journal: new Map(), // key → line (deduped; insertion order is arrival)
    frames: new Map(),  // port → { png: data URI, at: ms }
    photos: new Map(),  // class → url (fetched once per class)
    links: null,        // the About page's destinations, fetched once
  };

  // The client's journal keeps the house's own bound.
  const JOURNAL_CAP = 600;
  const JOBS_CAP = 40;

  function mark(...slices) {
    for (const s of slices) dirty.add(s);
    if (!notifyQueued) {
      notifyQueued = true;
      queueMicrotask(() => {
        notifyQueued = false;
        const slices = [...dirty];
        dirty.clear();
        if (slices.length === 0) return;
        for (const l of listeners) l(slices);
      });
    }
  }

  const dataUri = (pngB64) => `data:image/png;base64,${pngB64}`;
  const journalKey = (line) => `${line.ts}|${line.domain}|${line.text}`;

  // ── the reducers: the only writers ────────────────────────────────

  /// The connection-opening fact: replace the slices whole. A
  /// reconnect replaces, never appends — a dropped stream can
  /// duplicate nothing.
  function ingestSnapshot(snap) {
    if (snap.service) state.service = snap.service;
    state.devices = new Map((snap.devices ?? []).map((r) => [r.port, r]));
    state.roster = new Map((snap.roster ?? []).map((i) => [i.device_id, i]));
    state.jobs = new Map((snap.jobs ?? []).map((j) => [j.id, j]));
    state.frames = new Map(
      (snap.frames ?? []).map((f) => [f.port, { png: dataUri(f.png), at: Date.now() }])
    );
    state.journal = new Map((snap.journal ?? []).map((line) => [journalKey(line), line]));
    mark("service", "devices", "roster", "jobs", "frames", "journal", "stream");
  }

  /// One delta fact, routed by its type tag.
  function ingestFact(fact) {
    switch (fact.type) {
      case "devices": {
        state.devices = new Map((fact.rows ?? []).map((r) => [r.port, r]));
        mark("devices");
        break;
      }
      case "roster": {
        state.roster = new Map((fact.individuals ?? []).map((i) => [i.device_id, i]));
        mark("roster");
        break;
      }
      case "job": {
        state.jobs.delete(fact.job.id); // re-insert to refresh arrival order
        state.jobs.set(fact.job.id, fact.job);
        while (state.jobs.size > JOBS_CAP) {
          state.jobs.delete(state.jobs.keys().next().value);
        }
        mark("jobs");
        break;
      }
      case "frame": {
        state.frames.set(fact.port, { png: dataUri(fact.png), at: Date.now() });
        mark("frames");
        break;
      }
      case "paused": {
        state.service = { ...state.service, paused: !!fact.paused };
        mark("service");
        break;
      }
      default:
        break; // narrative facts carry no store state
    }
  }

  function ingestJournal(line) {
    state.journal.set(journalKey(line), line);
    while (state.journal.size > JOURNAL_CAP) {
      state.journal.delete(state.journal.keys().next().value);
    }
    mark("journal");
  }

  function setStream(stream) {
    if (state.stream === stream) return;
    state.stream = stream;
    mark("stream", "service");
  }

  function putPhoto(classId, url) {
    if (state.photos.has(classId)) return;
    state.photos.set(classId, url);
    mark("photos");
  }

  function putLinks(links) {
    state.links = links;
    mark("links");
  }

  function subscribe(listener) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }

  /// The journal, newest first.
  function journalLines() {
    return [...state.journal.values()].sort((a, b) => b.ts.localeCompare(a.ts));
  }

  window.SuzuStore = {
    state,
    subscribe,
    ingestSnapshot,
    ingestFact,
    ingestJournal,
    setStream,
    putPhoto,
    putLinks,
    journalLines,
  };
})();
