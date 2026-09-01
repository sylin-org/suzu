# @sylin-org/suzu (npm)

A tiny bell for your machines — companion displays for software events.
This package downloads the suzu binary from the
[GitHub release](https://github.com/sylin-org/suzu/releases) at install
time and exposes it as `suzu` on the path.

```sh
npx @sylin-org/suzu scan
npm i -g @sylin-org/suzu
suzu version
```

The binary carries its own hardware manifests, firmware payloads, and
workbench UI — nothing else to install. Deploy the service with
`sudo suzu install`, then open `http://127.0.0.1:7899`.

Platform notes: Windows x64 and Linux x64 (glibc) are carried by the
release archives. On musl hosts (Alpine), point the installer at the
musl archive: `SUZU_BINARY_URL=https://github.com/sylin-org/suzu/releases/download/v0.1.0/suzu-v0.1.0-x86_64-linux-musl.tar.gz npm i -g @sylin-org/suzu`.

## Publishing (maintainers)

The `sylin-org` npm org exists and holds no packages; publishing needs a
token from an account with access to it.

```sh
cd packaging/npm
# bump "version" to match the release tag
npm publish --access public
```
