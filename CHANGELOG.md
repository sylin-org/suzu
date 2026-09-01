# Changelog

Suzu follows the house style: every entry says what the bell learned.
Releases are tagged `v*` — the tag builds the archives.

## v0.1.0 — the first bell

The Resident, on its own feet. A small service that minds a fleet of
companion displays: it senses the machines, keeps their faces current,
and turns software events into light and one soft ring.

**What ships**

- The full device lifecycle on real hardware: detection and honest
  identification, backup-first adoption with read-back verification,
  admission gates before any face joins the stream.
- Factory-fresh ESP8266 boards onboard end to end, in Rust, through a
  native ROM-bootloader engine — erase, JEDEC-detected flash-size
  patch, write, verify — with no other tooling on the host.
- `suzu install`: the Resident deploys itself on Linux — binary,
  embedded resources, udev, and the right service file for systemd or
  OpenRC. Proven on Arch, Fedora Atomic, and Alpine testbeds.
- A self-contained binary: the hardware manifests, firmware payloads,
  and workbench UI travel inside the executable. An archive is a
  complete install.
- The workbench served by the Resident itself — any browser on the
  host is the family window — beside the Tauri desktop shell.
- The bell rope: `POST /api/say` (token-gated when the host sets
  `SUZU_API_TOKEN`), plus a GitHub Action that rings it.
- The suzu/1 wire contract, the faceplate wardrobe (numerals, slate,
  aurora, the lake), and the ADR paper trail behind every decision.
