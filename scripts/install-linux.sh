#!/usr/bin/env bash
# Install a checkout-built Suzu Resident on any systemd Linux host.
set -euo pipefail

SUZU_SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SUZU_REPO_DIR="$(cd -- "${SUZU_SCRIPT_DIR}/.." && pwd)"
SUZU_ACTION="install"
SUZU_KEEPER="${SUDO_USER:-${USER:-}}"
SUZU_PREFIX="/usr/local"
SUZU_BINARY="${SUZU_REPO_DIR}/target/release/suzu"
SUZU_START=1
SUZU_PURGE_STATE=0

usage() {
    printf '%s\n' \
        "Usage: sudo scripts/install-linux.sh [install|verify|uninstall] [options]" \
        "" \
        "  --user NAME       Unix account that keeps the Resident" \
        "  --binary PATH     Built suzu binary (default: target/release/suzu)" \
        "  --prefix PATH     Installation prefix (default: /usr/local)" \
        "  --no-start        Install and enable, but do not start now" \
        "  --purge-state     With uninstall, remove /var/lib/suzu/NAME" \
        "  -h, --help        Show this help"
}

if [[ $# -gt 0 && "${1}" != --* && "${1}" != "-h" ]]; then
    SUZU_ACTION="${1}"
    shift
fi
while [[ $# -gt 0 ]]; do
    case "${1}" in
        --user) SUZU_KEEPER="${2:?--user needs an account}"; shift 2 ;;
        --binary) SUZU_BINARY="${2:?--binary needs a path}"; shift 2 ;;
        --prefix) SUZU_PREFIX="${2:?--prefix needs a path}"; shift 2 ;;
        --no-start) SUZU_START=0; shift ;;
        --purge-state) SUZU_PURGE_STATE=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'Unknown option: %s\n' "${1}" >&2; usage >&2; exit 2 ;;
    esac
done

case "${SUZU_ACTION}" in install|verify|uninstall) ;; *) usage >&2; exit 2 ;; esac
[[ "${SUZU_PREFIX}" == /* && "${SUZU_PREFIX}" != *[' |']* ]] || {
    printf 'The prefix must be an absolute path without spaces or pipes.\n' >&2
    exit 2
}
[[ "${SUZU_KEEPER}" =~ ^[a-z_][a-z0-9_-]*[$]?$ ]] || {
    printf 'Not a safe Unix account name: %s\n' "${SUZU_KEEPER}" >&2
    exit 2
}
command -v systemctl >/dev/null || { printf 'systemd is required.\n' >&2; exit 1; }
getent passwd "${SUZU_KEEPER}" >/dev/null || {
    printf 'The keeper account does not exist: %s\n' "${SUZU_KEEPER}" >&2
    exit 1
}

SUZU_UNIT="suzu@${SUZU_KEEPER}.service"
SUZU_BINDIR="${SUZU_PREFIX}/bin"
SUZU_RESOURCE_DIR="${SUZU_PREFIX}/share/suzu"
SUZU_UNIT_FILE="/etc/systemd/system/suzu@.service"
SUZU_RULE_FILE="/etc/udev/rules.d/60-suzu.rules"

verify_install() {
    test -x "${SUZU_BINDIR}/suzu"
    test -r "${SUZU_RESOURCE_DIR}/hardware/classes/esp8266-oled/signature.yaml"
    test -r "${SUZU_RESOURCE_DIR}/firmware/README.md"
    systemctl is-enabled --quiet "${SUZU_UNIT}"
    if [[ "${SUZU_START}" -eq 1 ]]; then
        systemctl is-active --quiet "${SUZU_UNIT}"
    fi
    printf 'Suzu installation verified: %s (%s)\n' "${SUZU_UNIT}" "${SUZU_BINDIR}/suzu"
}

if [[ "${SUZU_ACTION}" == "verify" ]]; then
    verify_install
    exit 0
fi

[[ "$(id -u)" -eq 0 ]] || {
    printf 'Install and uninstall need root; run this script with sudo.\n' >&2
    exit 1
}

if [[ "${SUZU_ACTION}" == "uninstall" ]]; then
    systemctl disable --now "${SUZU_UNIT}" 2>/dev/null || true
    rm -f -- "${SUZU_UNIT_FILE}" "${SUZU_RULE_FILE}" "${SUZU_BINDIR}/suzu"
    rm -rf -- "${SUZU_RESOURCE_DIR}"
    systemctl daemon-reload
    if command -v udevadm >/dev/null; then
        udevadm control --reload-rules
    fi
    if [[ "${SUZU_PURGE_STATE}" -eq 1 ]]; then
        SUZU_STATE_TARGET="/var/lib/suzu/${SUZU_KEEPER}"
        [[ "${SUZU_STATE_TARGET}" == /var/lib/suzu/* && "${SUZU_STATE_TARGET}" != /var/lib/suzu/ ]] || exit 1
        rm -rf -- "${SUZU_STATE_TARGET}"
        printf 'Removed service state: %s (not recoverable from this installer)\n' "${SUZU_STATE_TARGET}"
    else
        printf 'Preserved service state in /var/lib/suzu/%s.\n' "${SUZU_KEEPER}"
    fi
    printf 'Uninstalled %s. The shared suzu-hw group was preserved.\n' "${SUZU_UNIT}"
    exit 0
fi

test -x "${SUZU_BINARY}" || {
    printf 'No built binary at %s; run cargo build --release -p suzu first.\n' "${SUZU_BINARY}" >&2
    exit 1
}
test -d "${SUZU_REPO_DIR}/hardware/classes" || exit 1
test -d "${SUZU_REPO_DIR}/firmware" || exit 1
command -v groupadd >/dev/null || { printf 'groupadd is required.\n' >&2; exit 1; }
getent group suzu-hw >/dev/null || groupadd --system suzu-hw

install -d -m 0755 "${SUZU_BINDIR}" "${SUZU_RESOURCE_DIR}"
install -m 0755 "${SUZU_BINARY}" "${SUZU_BINDIR}/suzu"
install -d -m 0755 "${SUZU_RESOURCE_DIR}/hardware" "${SUZU_RESOURCE_DIR}/firmware"
cp -a "${SUZU_REPO_DIR}/hardware/." "${SUZU_RESOURCE_DIR}/hardware/"
cp -a "${SUZU_REPO_DIR}/firmware/." "${SUZU_RESOURCE_DIR}/firmware/"
chown -R root:root "${SUZU_RESOURCE_DIR}"
find "${SUZU_RESOURCE_DIR}" -type d -exec chmod 0755 {} +
find "${SUZU_RESOURCE_DIR}" -type f -exec chmod 0644 {} +

SUZU_UNIT_TMP="$(mktemp)"
trap 'rm -f -- "${SUZU_UNIT_TMP}"' EXIT
sed \
    -e "s|@SUZU_BINDIR@|${SUZU_BINDIR}|g" \
    -e "s|@SUZU_RESOURCE_DIR@|${SUZU_RESOURCE_DIR}|g" \
    "${SUZU_REPO_DIR}/packaging/systemd/suzu@.service" > "${SUZU_UNIT_TMP}"
install -m 0644 "${SUZU_UNIT_TMP}" "${SUZU_UNIT_FILE}"
install -m 0644 "${SUZU_REPO_DIR}/packaging/udev/60-suzu.rules" "${SUZU_RULE_FILE}"

systemctl daemon-reload
if command -v udevadm >/dev/null; then
    udevadm control --reload-rules
    udevadm trigger --subsystem-match=tty --action=add
    udevadm settle
fi
systemctl enable "${SUZU_UNIT}"
if [[ "${SUZU_START}" -eq 1 ]]; then
    systemctl restart "${SUZU_UNIT}"
fi
verify_install
