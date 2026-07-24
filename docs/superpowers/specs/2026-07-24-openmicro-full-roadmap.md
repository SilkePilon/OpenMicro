# OpenMicro — Full Build Roadmap (v1.1 → complete)

**Date:** 2026-07-24
**Context:** v1 host is done and merged to `master` (proto + daemon + TUI + hook + Claude adapter, MockDevice, 18 tests). This roadmap covers everything remaining, to be built autonomously via subagents.

## Hardware reality (read first)
The user left the Creator Micro 2 connected over USB but is away. Therefore:
- **No unattended firmware flashing.** Flashing needs the device physically in bootloader mode (hold boot button while plugging in — it enumerates as HID, not serial, so esptool can't auto-enter download mode), and no one is present to recover a bad flash. Firmware + the TUI installer are **built and made flash-ready**; the flash-and-validate step waits for the user.
- **Pinout** is being derived from software sources (QMK config, `wl-device-kit`, Input app) — see `docs/hardware/creator-micro-2-pinout-research.md`. If pins can't be fully derived, firmware uses clearly-marked TODO constants and the case must be opened later.
- The connected device (stock firmware) can be used to test **BLE scan/connect scaffolding** and USB enumeration, but not our custom GATT until our firmware is flashed.

## Shared protocol addition (P0)
Both host and firmware need the BLE transport spec. Add to `openmicro-proto` (no_std-safe):
- Custom GATT **service UUID**, **LED write char UUID**, **INPUT notify char UUID**; use standard Battery Service (0x180F) + Battery Level (0x2A19).
- The value written to the LED char is the postcard-encoded `LedFrame`; INPUT notify carries a postcard-encoded `InputEvent`. Add `Command` messages for brightness/sleep/config where needed.

## Sub-projects (build order)

### P1 — BLE `DeviceLink` (host, `bluer`)
Real backend behind the existing async `DeviceLink` trait. Discover + connect the ESP32-S3 by service UUID, write LED char, subscribe INPUT notify, read Battery, reconnect w/ backoff. Config selects `mock` vs `ble`. Unit-test encode/parse + connection state machine; integration against real device deferred until firmware is flashed (test BLE scan/connect scaffolding against the stock device where possible).

### P2 — Device input routing (host)
`InputEvent` → action on the owning/target agent: interrupt, focus, approve/reject, cycle. Define an `ActionRouter` + per-agent action backends (best-effort; some actions agent-specific). Wire from the BLE INPUT stream. Unit-test routing decisions with a mock action backend.

### P3 — Multi-agent adapters
A generic adapter contract + adapters for **Codex, T3 Code, Grok Code**, plus fixing the **Claude Code** session-id (real hook wrapper that reads the hook's stdin JSON for `session_id` rather than a guessed env var). Each adapter installs hooks that call `openmicro-hook`. `adapters/<agent>/` with hooks + install notes + any wrapper script.

### P4 — TUI config editing
A config-write command path over the control socket (currently read-only snapshots). TUI screens to edit: key↔agent mapping, per-state colors, brightness, sleep, pinned focus. Persist to `~/.config/openmicro/config.toml`. Daemon applies live.

### P5 — Battery / brightness / sleep (end-to-end)
Proto already has `Battery`. Add brightness/sleep `Command`s host→device; daemon pushes them; TUI shows battery + controls brightness/sleep. On-device behavior lives in firmware (P8).

### P6 — TUI firmware installer
Make setup one-command. TUI flow: detect the device, walk the user through bootloader entry, run `esptool` to flash the built firmware image, verify, and reconnect. Wire it fully; validation deferred to when the user can flash. Include a plain `openmicro flash` CLI path too.

### P7 — Packaging
`systemd --user` service for `openmicrod`, install/uninstall scripts, `openmicro` CLI subcommands (start/stop daemon, status, flash, install-agent <name>). Sensible XDG paths.

### P8 — Firmware crate (embedded Rust)
`firmware/` — no_std esp-hal + embassy + TrouBLE for ESP32-S3. Custom BLE GATT service (LED write / INPUT notify / Battery), WS2812 RGB render with on-device effects (solid/breath/pulse/rainbow), key-matrix + encoder + joystick scan emitting `InputEvent`. Pinout from research (P0 doc) or TODO constants. Target build must compile; flash-and-run deferred to the user.

**Status (2026-07-24): partially done, by design.**
- Done + tested: `crates/openmicro-effects` — the Solid/Breath/Pulse/Rainbow
  effect resolver moved out of the embedded crate into its own `no_std`,
  root-workspace, host-testable library (14 unit tests, TDD'd), so the
  animation logic is verified without any hardware or toolchain.
- Done, NOT compiled: `firmware/` — the full skeleton (BLE GATT server over
  TrouBLE, WS2812 render task, key/encoder/joystick input scanning, a
  `pins.rs` with every physical GPIO as a grouped `// TODO(pinout):`
  constant) exists as its own Cargo workspace, with dependency versions
  pinned and cross-checked against each crate's published `Cargo.toml` (see
  `firmware/README.md` and `.superpowers/sdd/p8-firmware-report.md` for the
  exact versions + rationale, including two deliberate deviations from the
  obvious "latest" picks where the docs showed real incompatibilities).
  **It has never been built** — no Xtensa/`espup` toolchain was installed
  (explicitly out of scope), so "target build must compile" above is not yet
  met. That, plus filling in the real pinout and flashing, is the remaining
  work and needs the physical device.

## Execution
Each sub-project: a just-in-time plan, then subagent implementation with TDD + review, committed on `openmicro-features`, merged to `master` when green. Firmware (P8) and installer (P6) are built but marked "awaiting on-hardware validation."

## Definition of done (this autonomous run)
- P1–P5, P7: built, tested (no hardware), merged.
- P6, P8: built, compiles/flash-ready, clearly marked as needing the user's on-hardware validation.
- A single `docs/hardware/` note tells the user exactly what physical steps remain (bootloader entry + flash + any un-derivable pins).
