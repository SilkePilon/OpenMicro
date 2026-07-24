# What's left for you (the physical hardware steps)

Everything that can be built and tested without the hardware loop is **done** (host software: daemon, TUI, adapters, packaging, effect core, installer — 118 tests, v1.1.0). Three things remain that need you physically present with the Creator Micro 2, because they involve the boot button and (possibly) opening the case, and a bad flash needs someone there to recover.

## Step 1 — Get the GPIO pinout (one-time)
The MCU is confirmed **ESP32-S3**, but no GPIO pin numbers are public (see `creator-micro-2-pinout-research.md`). Pick one:

- **A. Read-flash + disassemble (no case opening):** put the device in bootloader mode (hold the small boot button on the bottom while plugging in USB — it enumerates as `303a:1001`), then:
  ```
  esptool --chip esp32s3 read_flash 0x0 0x400000 stock-firmware.bin
  ```
  This is also your **safety backup** — keep it. Then disassemble/grep the binary for `gpio_`, `led_strip`/`rmt`, `adc`, `touch_pad` init to recover the pins. (I can help with the disassembly when you're back.)
- **B. Open the case + continuity-test** the PCB against the ESP32-S3 module pads for: key matrix rows/cols + diode direction, the WS2812 data pin + LED count/order, encoder A/B + press, joystick X/Y ADC (+ button), touch pin, battery-sense pin.

Fill the results into `firmware/src/pins.rs` (every unknown is a grouped `// TODO(pinout):` there — one file to edit).

## Step 2 — Build the firmware
Install the Xtensa Rust toolchain and build (details in `firmware/README.md`):
```
espup install && source ~/export-esp.sh
cd firmware && cargo build --release
```
The embedded crate was written against current esp-hal / TrouBLE but **never compiled here** (no toolchain), so expect to fix a few API/version details on the first real build — the effect logic (`openmicro-effects`) is already host-tested, so the bugs will be in the hardware glue, not the animation math.

## Step 3 — Flash it (super easy from here)
Put the device in bootloader mode again (boot button + replug), then either:
```
openmicro flash              # auto-detects the built image + device
```
or open the TUI (`openmicro`), press **`f`**, and follow the checklist (it tells you exactly what's missing until all three items are green, then flashes).

After flashing, reset the device. Set `transport = "ble"` in `~/.config/openmicro/config.toml`, start the daemon (`systemctl --user enable --now openmicrod`), and your agent keys should light up with live state. Install per-agent hooks with `openmicro install-agent claude` (and `codex`/`grok`/`t3`).

## Known follow-ups (v1.2, not blocking)
- BLE auto-reconnect is wired but minimal (bounded 2s attempt, 5s cooldown) — a proper background supervisor is nicer.
- `Approve`/`Interrupt` device actions are logged but not yet executed per-agent (needs a per-agent control channel).
- Firmware `main.rs` task bodies are wired to the modules but only fully validated once you compile+flash.
