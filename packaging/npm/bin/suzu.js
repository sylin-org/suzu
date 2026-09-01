#!/usr/bin/env node
// Runs the downloaded suzu binary with the arguments given. `npx suzu scan`.
"use strict";

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const VERSION = require("../package.json").version;

const candidates =
  process.platform === "win32"
    ? [`suzu-v${VERSION}-x86_64-windows/suzu.exe`]
    : [`suzu-v${VERSION}-x86_64-linux-gnu/suzu`, `suzu-v${VERSION}-x86_64-linux-musl/suzu`];

const fs = require("node:fs");
const binary = candidates
  .map((c) => path.join(__dirname, "..", "vendor", c))
  .find((p) => fs.existsSync(p));

if (!binary) {
  console.error("suzu: no binary installed — run: npm rebuild @sylin-org/suzu");
  process.exit(1);
}

const r = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
process.exit(r.status ?? 1);
