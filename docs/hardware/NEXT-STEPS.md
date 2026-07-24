# What's left for you (the physical hardware steps)

> **Update, 2026-07-24:** the GPIO pinout has been recovered from Work Louder's
> own published firmware and is wired into `firmware/src/pins.rs` — Step 1 below
> is done, and you no longer need to open the case. What remains needs the board
> in hand: the HAL plumbing in `main.rs`'s task bodies, the key-ID map, and the
> MAX77972's I2C pins. See `creator-micro-2-pinout-findings.md`.
>
> Restoring the vendor firmware also no longer depends on taking a backup first
> — Work Louder publish their images, and the TUI downloads them.

Everything that can be built and tested without the hardware loop is **done** (host software: daemon, TUI, adapters, packaging, effect core, installer — 118 tests, v1.1.0). What remains needs you physically present with the Creator Micro 2: recovering the GPIO pinout (possibly by opening the case), and being there if a flash goes wrong. Bootloader mode itself no longer needs you — OpenMicro asks the firmware to reboot into it over HID.

## Step 1 — Get the GPIO pinout (one-time) — **DONE**
Recovered by disassembling the vendor's own published firmware; see
`creator-micro-2-pinout-findings.md`. The two manual routes below are kept only
as a fallback if something in that map turns out wrong on the bench:

- **A. Read-flash + disassemble (no case opening):** put the device in bootloader mode (run `openmicro`, **Device → Reboot into bootloader mode**; it then enumerates as `303a:1001`), then:
  ```
  esptool --chip esp32s3 read_flash 0x0 0x400000 stock-firmware.bin
  ```
  This is also your **safety backup** — keep it. Then disassemble/grep the binary for `gpio_`, `led_strip`/`rmt`, `adc`, `touch_pad` init to recover the pins. (I can help with the disassembly when you're back.)
- **B. Open the case + continuity-test** the PCB against the ESP32-S3 module pads for: key matrix rows/cols + diode direction, the WS2812 data pin + LED count/order, encoder A/B + press, joystick X/Y ADC (+ button), touch pin, battery-sense pin.

Fill the results into `firmware/src/pins.rs` (every unknown is a grouped `// TODO(pinout):` there — one file to edit).

## Steps 2 and 3 — just run the wizard
```
openmicro
```
Pick **Set up my macropad**. The TUI drives the rest end to end. It shows the custom-firmware warning (including that reverting is possible **only** from a backup you take first), watches USB and Bluetooth with a spinner until it finds the device, tells you exactly what to do next based on what it found — swap to a USB cable if it is only on Bluetooth, or reboot it into bootloader mode for you (no button — it sends the firmware a `sys.bootloader` RPC) — and moves on by itself the moment the device shows up in bootloader mode.

Its firmware menu then does the remaining work, greying out anything it cannot do yet and saying why:

- **Back up the stock firmware** — do this first. It dumps all 4 MiB to `~/.local/share/openmicro/stock-firmware.bin` and is the only route back (**Firmware → Restore the stock firmware**).
- **Build firmware from source** — needs the Xtensa toolchain (`cargo install espup && espup install`; details in `firmware/README.md`). Read "Where the firmware build stands" below first.
- **Download prebuilt firmware** — opens a picker listing every published version. The images come from `.github/workflows/firmware.yml`, which builds the firmware and attaches `openmicro-fw.bin` whenever you publish a release. No release exists yet, so this stays empty until you cut one — trigger the workflow manually ("Run workflow") first to see whether the firmware actually compiles.
- **Flash the device** — picks the right tool for the image: `espflash` for a from-source ELF, `esptool` for a merged release `.bin`.

Finally it asks which coding agents to wire up, detecting what is installed on this machine and merging the hooks into each agent's own config (backing the previous file up first).

After flashing, reset the device, set `transport = "ble"` in `~/.config/openmicro/config.toml`, and start the daemon (`systemctl --user enable --now openmicrod`). Your agent keys should then light up with live state.

## Where the firmware build stands

The firmware has now been compiled for real, on CI (`.github/workflows/firmware.yml`,
run it from the Actions tab). The toolchain half works: espup installs, the
crates resolve, and hundreds of dependencies build. Three blockers were found
and fixed on the way:

1. The espup toolchain ships `rust-src` but **no precompiled `core` for
   xtensa-esp32s3-none-elf** — its `lib/rustlib` only has the host triple. Fixed
   by building the standard library from source (`[unstable] build-std` in
   `firmware/.cargo/config.toml`).
2. `esp-println` panics in its build script unless exactly one output backend is
   enabled. Now `jtag-serial`, since this board has no UART header.
3. `trouble-host` refuses to build without `central` or `peripheral`. Now
   `peripheral` + `gatt`.

**What is still broken:** `esp-hal-embassy 0.9.1` does not compile against the
`esp-hal` it resolves to (1.1.1). It imports `esp_hal::sync::Locked`, which no
longer exists, and `RawPriorityLimitedMutex` no longer implements `RawMutex`.
The version comments in `firmware/Cargo.toml` explain why each pin was chosen,
but that reasoning came from reading published manifests rather than from a
build — the resolved set is not actually mutually compatible.

Fixing it means picking a set of esp-hal / esp-hal-embassy / esp-radio /
trouble-host / embassy versions that agree, most easily by starting from a
current `esp-hal` example for the ESP32-S3 and matching its `Cargo.toml`. That
is bring-up work worth doing alongside the pinout, not before it — nothing is
flashable until `firmware/src/pins.rs` is filled in anyway.

None of this touches the host side: `openmicro-effects` is host-tested, and
`cargo test --workspace` at the repo root does not build the firmware at all.

## Known follow-ups (v1.2, not blocking)
- BLE auto-reconnect is wired but minimal (bounded 2s attempt, 5s cooldown) — a proper background supervisor is nicer.
- `Approve`/`Interrupt` device actions are logged but not yet executed per-agent (needs a per-agent control channel).
- Firmware `main.rs` task bodies are wired to the modules but only fully validated once you compile+flash.
