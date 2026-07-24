#!/usr/bin/env bash
#
# OpenMicro uninstaller (host side).
#
# Stops and disables the systemd user unit, then removes the unit and the three
# installed binaries. Pass --purge to also remove the user config directory
# (~/.config/openmicro). Best-effort: missing files are not an error.
set -euo pipefail

PURGE=0
for arg in "$@"; do
    case "${arg}" in
        --purge) PURGE=1 ;;
        *) echo "unknown argument: ${arg}" >&2; exit 2 ;;
    esac
done

BIN_DIR="${HOME}/.local/bin"
UNIT_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
UNIT_NAME="openmicrod.service"
CONFIG_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/openmicro"

if command -v systemctl >/dev/null 2>&1; then
    echo "==> Stopping and disabling ${UNIT_NAME}"
    systemctl --user disable --now "${UNIT_NAME}" 2>/dev/null || true
fi

echo "==> Removing systemd user unit"
rm -f "${UNIT_DIR}/${UNIT_NAME}"

echo "==> Removing binaries from ${BIN_DIR}"
for bin in openmicrod openmicro openmicro-hook; do
    rm -f "${BIN_DIR}/${bin}"
done

if command -v systemctl >/dev/null 2>&1; then
    echo "==> Reloading systemd user manager"
    systemctl --user daemon-reload || true
fi

if [ "${PURGE}" -eq 1 ]; then
    echo "==> Purging config directory ${CONFIG_DIR}"
    rm -rf "${CONFIG_DIR}"
fi

echo "OpenMicro uninstalled."
