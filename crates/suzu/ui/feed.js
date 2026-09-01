//! Receives Resident SSE messages and updates the store.
//!
//! Two homes, one file: the desktop shell republishes `/api/events` as
//! Tauri events, and the Resident serves this UI itself — in which case
//! the browser holds the SSE connection directly. Either way this file
//! is the whole of the client's network awareness: parse, route by
//! kind, ingest. A reconnect brings a fresh snapshot and the reducers
//! replace their slices — nothing appends, nothing duplicates.

(() => {
  "use strict";

  const Store = window.SuzuStore;
  const tauri = window.__TAURI__;

  const ingest = (raw) => {
    let msg;
    try {
      msg = typeof raw === "string" ? JSON.parse(raw) : raw;
    } catch {
      return;
    }
    if (msg.type === "snapshot") Store.ingestSnapshot(msg.snapshot ?? {});
    else if (msg.type === "journal") Store.ingestJournal(msg.line);
    else Store.ingestFact(msg);
  };

  if (tauri?.event?.listen) {
    tauri.event.listen("resident-event", (e) => ingest(e.payload));
    tauri.event.listen("resident-health", (e) => {
      Store.setStream(e.payload === "connected" ? "connected" : "reconnecting");
    });
    return;
  }

  // Served by the Resident: the stream is same-origin, and the browser's
  // own reconnect becomes the stream-health signal.
  const es = new EventSource("/api/events");
  es.onopen = () => Store.setStream("connected");
  es.onerror = () => Store.setStream("reconnecting");
  es.onmessage = (e) => ingest(e.data);
})();
