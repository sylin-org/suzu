//! The feed — the shell's SSE bridge, poured into the store.
//!
//! The Rust shell holds one connection to `/api/events` and republishes
//! every frame as a Tauri event; this file is the whole of the client's
//! network awareness: parse, route by kind, ingest. A reconnect brings
//! a fresh snapshot and the reducers replace their slices — nothing
//! appends, nothing duplicates.

(() => {
  "use strict";

  const tauri = window.__TAURI__;
  if (!tauri?.event?.listen) return; // opened outside the shell: nothing to feed on

  const Store = window.SuzuStore;

  tauri.event.listen("house", (e) => {
    let msg;
    try {
      msg = JSON.parse(e.payload);
    } catch {
      return;
    }
    if (msg.type === "snapshot") Store.ingestSnapshot(msg.snapshot ?? {});
    else if (msg.type === "journal") Store.ingestJournal(msg.line);
    else Store.ingestFact(msg);
  });

  tauri.event.listen("house-health", (e) => {
    Store.setStream(e.payload === "connected" ? "connected" : "reconnecting");
  });
})();
