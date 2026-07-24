#!/usr/bin/env bash
#
# OpenMicro installer (host side).
#
# Builds the workspace in release mode, installs the three host binaries into
# ~/.local/bin, and installs the systemd *user* unit for the daemon. Idempotent:
# re-running overwrites the binaries and unit in place. Does NOT enable or start
# the service or touch the firmware; it prints the next steps instead.
set -euo pipefail

# Resolve repo root from this script's location so it can be run from anywhere.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"

BIN_DIR="${HOME}/.local/bin"
UNIT_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
UNIT_NAME="openmicrod.service"

echo "==> Building release binaries (this may take a while)..."
cargo build --release --workspace --manifest-path "${REPO_ROOT}/Cargo.toml"

echo "==> Installing binaries to ${BIN_DIR}"
mkdir -p "${BIN_DIR}"
for bin in openmicrod openmicro openmicro-hook; do
    install -m 0755 "${REPO_ROOT}/target/release/${bin}" "${BIN_DIR}/${bin}"
    echo "    installed ${bin}"
done

echo "==> Installing systemd user unit to ${UNIT_DIR}/${UNIT_NAME}"
mkdir -p "${UNIT_DIR}"
install -m 0644 "${SCRIPT_DIR}/${UNIT_NAME}" "${UNIT_DIR}/${UNIT_NAME}"

echo "==> Reloading systemd user manager"
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
else
    echo "    systemctl not found; skipping daemon-reload"
fi

cat <<EOF

OpenMicro installed.

Make sure ${BIN_DIR} is on your PATH, then:

  # Enable + start the daemon now and on login:
  systemctl --user enable --now openmicrod

  # Open the TUI:
  openmicro

  # Or inspect state from the CLI:
  openmicro status
  openmicro install-agent claude

If the daemon should keep running after you log out, enable lingering once:

  loginctl enable-linger "\$USER"
EOF
