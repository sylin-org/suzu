#!/usr/bin/env node
// Downloads the suzu binary for this platform from the GitHub release,
// into vendor/ beside this package. Node >= 18 (fetch). Override the
// source with SUZU_BINARY_URL for offline or musl hosts.
"use strict";

const { mkdir, writeFile } = require("node:fs/promises");
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const VERSION = require("../package.json").version;

function assetFor(platform, arch) {
  if (platform === "win32" && arch === "x64")
    return { name: `suzu-v${VERSION}-x86_64-windows.zip`, inner: `suzu-v${VERSION}-x86_64-windows/suzu.exe` };
  if (platform === "linux" && arch === "x64")
    return { name: `suzu-v${VERSION}-x86_64-linux-gnu.tar.gz`, inner: `suzu-v${VERSION}-x86_64-linux-gnu/suzu` };
  throw new Error(`no suzu binary for ${platform}-${arch} yet — see https://github.com/sylin-org/suzu/releases`);
}

async function main() {
  const asset = assetFor(process.platform, process.arch);
  const url = process.env.SUZU_BINARY_URL
    ?? `https://github.com/sylin-org/suzu/releases/download/v${VERSION}/${asset.name}`;
  const dest = path.join(__dirname, "..", "vendor");
  await mkdir(dest, { recursive: true });

  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) throw new Error(`${url} → ${res.status}`);
  const buf = Buffer.from(await res.arrayBuffer());
  const archive = path.join(dest, asset.name);
  await writeFile(archive, buf);

  // Extraction without dependencies. tar.gz goes through tar (run
  // inside dest with bare names: a Windows drive path reads as a remote
  // host to tar). zip on Windows goes through Expand-Archive, because
  // GNU tar — first on PATH in many shells — cannot read zip.
  let extracted;
  if (asset.name.endsWith(".zip")) {
    extracted = spawnSync(
      "powershell",
      ["-NoProfile", "-Command", "Expand-Archive", "-Path", asset.name, "-DestinationPath", ".", "-Force"],
      { cwd: dest, stdio: "inherit" },
    );
  } else {
    extracted = spawnSync("tar", ["-xf", asset.name], { cwd: dest, stdio: "inherit" });
  }
  if (extracted.status !== 0) throw new Error("extraction failed");

  console.log(`suzu ${VERSION} installed to vendor/${path.basename(asset.inner)}`);
}

main().catch((e) => {
  console.error(`suzu postinstall: ${e.message}`);
  process.exit(1);
});
