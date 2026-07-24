# OpenMicro — Design Spec

**Date:** 2026-07-24
**Status:** Approved (design), pending implementation plan
**Repo:** `/home/silke/Documents/GitHub/OpenMirco`

## Overview

OpenMicro is an all-Rust, open control plane for the Work Louder **Creator
Micro 2** (ESP32-S3) macropad. It bridges AI coding agents — **Claude Code,
Codex, T3 Code, Grok Code**, and more — to the device: per-key RGB mirrors each
agent's live state, and keys route actions back to the owning agent. It replaces
the proprietary Codex/OAI firmware+app pairing with a stack the user fully owns.

Three subsystems, one workspace:

1. **Custom firmware** (embedded Rust, ESP32-S3) — drives RGB + inputs, speaks a
   custom BLE GATT protocol.
2. **Host daemon** (`openmicrod`) — receives agent state via per-agent hooks,
   runs the focus policy, drives the device over BLE.
3. **Interactive TUI** (`openmicro`) — the UI. A rich terminal client. **No menu
   bar / tray icon, no GUI window.**

### Why this exists

The Creator Micro 2 ships with firmware that does **not** implement the OAI
agent-key protocol (verified: keys emit plain HID keystrokes, LED writes are
ignored, no vendor-channel notifies). The Codex firmware that does is
proprietary and not publicly downloadable. Rather than depend on it, OpenMicro
writes its own firmware + host and supports many agents instead of one.

## Goals

- All-Rust codebase: firmware, daemon, TUI, hook shim, shared protocol crate.
- Multi-agent: Claude Code first; Codex / T3 Code / Grok Code and others via a
  uniform hook-adapter pattern.
- Per-agent key color + live state; rich on-device effects; bidirectional input;
  battery/brightness/sleep.
- Wireless (BLE) as the primary transport in v1.
- The TUI is genuinely nice to use — live, interactive, responsive.

## Non-Goals

- No menu-bar/tray app, no Electron/Tauri GUI.
- No OAI/ChatGPT-desktop interop (custom protocol, deliberately). USB transport
  is a later fallback, not a v1 goal.
- No cloud, telemetry, or account.

## Key Decisions (with rationale)

| Decision | Choice | Why |
|---|---|---|
| Project home | `OpenMirco/` | `firmware/` already there; name fits. |
| Agent integration | **Per-agent hooks/plugins** | Accurate, official event sources; no brittle log-scraping. |
| CLI ↔ daemon | **Headless daemon + TUI client** | Device keeps working with no terminal open; no tray. |
| Firmware stack | **Embedded Rust (esp-hal, no_std)** | All-Rust, modern, fully owned. |
| Wire protocol | **Custom clean protocol** | Optimized for multi-agent zones/effects; both ends are ours. |
| Transport | **BLE in v1** (USB later fallback) | Cable-free. Higher risk in embedded Rust — mitigated below. |
| First agent | **Claude Code** | Best hooks; immediately testable from inside it. |
| Sequencing | **Software-first, mock device** | Quarantines BLE risk to one milestone; host testable with no hardware. |

## Research: BLE in embedded Rust (de-risking the v1 BLE choice)

- **TrouBLE** (Embassy's Rust BLE Host) supports ESP32 via `esp-radio`, BLE
  4.x/5.x. Real, active, young.
- No canned HID-over-GATT profile in Rust — **not needed.** The custom protocol
  is a plain **custom GATT service**: one `LED` write characteristic
  (host→device frames) + one `INPUT` notify characteristic (device→host events)
  + standard Battery Service. This is TrouBLE's core competency and far simpler
  than reimplementing HOGP.
- **Host side:** Linux talks to that GATT service via **BlueZ using the `bluer`
  crate** (mature async Rust). No hidraw, no `libappindicator`/tray issues.
- **Chip:** device pairs over BT *and* exposes native USB → almost certainly
  **ESP32-S3** (S2 has no radio). Must confirm via bootloader.
- **RGB/input:** `esp-hal` RMT + `smart-leds` (WS2812) and GPIO/ADC are
  well-supported.

Sources: TrouBLE docs (`docs.embassy.dev/trouble-host`), ESP32 Rust BLE book
(`esp32.implrust.com/bluetooth/trouble`), `imliubo/codex-micro-4-core2`
(reference C++ firmware, protocol shape).

## Architecture

```
OpenMirco/
  Cargo.toml                 # workspace
  crates/
    openmicro-proto/         # shared no_std+std: message types + postcard codec
    openmicrod/              # daemon (std, tokio)
    openmicro/               # interactive TUI client (ratatui)
    openmicro-hook/          # tiny CLI that agent hooks call
  firmware/                  # no_std esp-hal + embassy + trouble (ESP32-S3)
  adapters/claude-code/      # hook install + templates
  docs/superpowers/specs/
```

### Data flow

```
agent event → agent hook → openmicro-hook → daemon unix socket
   → state engine → focus policy picks owner → LedFrame
   → bluer writes LED characteristic → firmware renders RGB (on-device effects)

device input → firmware INPUT notify → bluer → daemon → routed action
TUI  ⇄  daemon control socket (subscribe snapshots, push config)
```

## Components (one clear purpose each)

### `openmicro-proto`
The contract shared by firmware and daemon so they cannot drift.
- Types: `AgentState { Idle, Thinking, Working, AwaitingApproval }`,
  `Effect { Solid, Breath, Pulse, Rainbow, ... }`,
  `LedSlot { color: Rgb, effect: Effect, brightness: u8 }`,
  `LedFrame { slots: [LedSlot; N] }` (N = agent-key count, ~6; fixed once pinout is confirmed),
  `InputEvent { Key{id,edge}, Encoder{delta}, Joystick{dir} }`,
  `Battery { pct: u8, charging: bool }`.
- `#![no_std]` with `alloc`; `std` feature for the host. `postcard` encoding.
- Unit-tested: encode/decode roundtrip.

### firmware (ESP32-S3, no_std)
Embassy async tasks:
- **BLE GATT server** (trouble): custom service with `LED` (write) + `INPUT`
  (notify) characteristics; standard Battery Service. Advertise on boot;
  re-advertise on disconnect.
- **LED renderer** (`smart-leds` over RMT): applies `LedFrame`; runs effects
  on-device for smooth animation independent of host tick rate.
- **Input scanner**: key matrix + encoder + joystick → `InputEvent` notifications.
- **Battery** task: sample + notify.
- Pure logic (effect curves, color math) unit-testable on host.

### `openmicrod` (daemon, std + tokio)
- **Device link**: `bluer` GATT client. Discover/connect the S3, write `LED`,
  subscribe `INPUT`, read Battery. Reconnect with backoff. Behind a `DeviceLink`
  trait with a **MockDevice** impl for hardware-free testing.
- **Hook ingress**: unix socket at `$XDG_RUNTIME_DIR/openmicro.sock`, newline
  JSON events from adapters (`{agent, session, state}`), best-effort.
- **State engine**: sessions keyed `agent:session_id` (state + timestamp).
  Maps states → per-slot color/effect → `LedFrame`.
- **Focus policy**: exactly one session owns the deck (most-recent or pinned);
  `AwaitingApproval` may preempt. Integrations never touch the device directly.
- **Config**: TOML at `~/.config/openmicro/config.toml` — key↔agent mapping,
  colors, effects, brightness, sleep.
- **Control socket**: for the TUI — snapshot stream + config writes.

### `openmicro` (TUI, ratatui)
- Live dashboard: agents, states, current focus, device connection, battery.
- Config screens: map keys↔agents, pick colors/effects, brightness.
- Pairing helper: scan/connect the device.
- Connects to the daemon control socket. No tray, no window. Closing it does not
  stop the daemon.
- Render snapshot tests.

### `openmicro-hook`
Tiny CLI: `openmicro-hook <agent> <session> <state>` → writes the daemon socket.
The single thing every agent hook invokes.

### adapters/claude-code
Installs Claude Code hooks (session lifecycle / notification events) that call
`openmicro-hook`. Template + install/remove logic. Pattern repeats per agent.

## Error Handling

- **BLE drop**: daemon reconnects with backoff; TUI shows "disconnected";
  session state preserved; firmware idles LEDs and re-advertises.
- **Hook socket**: best-effort; malformed events dropped and counted.
- **Bad config**: fall back to defaults, warn in TUI/log.
- **No device present**: daemon runs with MockDevice-equivalent "detached"
  state; TUI shows not-connected; everything else works.

## Testing (TDD)

- `openmicro-proto`: encode/decode roundtrip unit tests.
- `openmicrod`: state-engine + focus-policy unit tests; integration test feeds
  fake hook events through a `MockDevice` and asserts the emitted `LedFrame`.
  Entire host validated with no hardware.
- `openmicro`: render snapshot tests.
- firmware: host-side unit tests for pure logic; on-device manual bring-up
  (blink → GATT → integrate).

## Milestones

**v1 (software-first, then firmware bring-up):**
1. `openmicro-proto` with tests.
2. `openmicrod`: MockDevice link + hook ingress + state engine + focus policy +
   per-agent **color+state**; config.
3. `openmicro-hook` + **Claude Code** adapter.
4. `openmicro` TUI: live dashboard + basic config.
5. firmware: confirm ESP32-S3 + pinout → BLE GATT service + **solid-color** LED
   render on real hardware → integrate with daemon over `bluer`.

**v1.1:** rich effects (breath/pulse); input events + routing (interrupt / focus
/ approve); battery / brightness / sleep.

**Later:** more agents (Codex, T3 Code, Grok Code); USB fallback transport; OTA
firmware update.

## Open Dependencies (hardware, gate firmware only)

1. **Confirm chip = ESP32-S3** + flash size — hold boot button while plugging
   USB → `esptool chip_id`. Take a full `read_flash` backup first (reversible).
2. **GPIO pinout** — LED type/data pin, key-matrix rows/cols, encoder pins,
   joystick ADC pins — open the case, photograph the PCB.

The v1 host + TUI + hook milestones need neither and start immediately.
