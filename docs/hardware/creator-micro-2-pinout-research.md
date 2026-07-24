# Creator Micro 2 (ESP32, VID 0x303A / PID 0x8298) — hardware pinout research

Research date: 2026-07-24. Goal: gather MCU + GPIO pinout facts so custom ESP32
firmware can be written for the Work Louder **Creator Micro 2**.

## TL;DR / headline finding

- **MCU is confirmed ESP32-S3.** (VID `0x303A` = Espressif, native USB, BLE,
  USB-Serial-JTAG bootloader, ESP32-S3 named in multiple host tools.)
- **The actual GPIO pin assignments (matrix, RGB data, encoder, joystick ADC,
  touch, battery) are NOT publicly documented anywhere I could find.** Every
  host-side artifact (Work Louder Input app, `@worklouder/wl-device-kit`,
  microbridge) talks to the device over a **logical HID/JSON-RPC protocol** and
  never references physical GPIOs — host software has no reason to know them.
- **The one public QMK repo (`ForsakenRei/qmk-worklouder-micro`) is for the OLD
  atmega32u4 Creator Micro (v1), a completely different MCU.** Its pin numbers
  are AVR port names (`B6`, `D2`, `F1`…) and are **NOT transferable** to the
  ESP32-S3 Micro 2. Reported below only for provenance.
- To get the Micro 2 GPIO map you will almost certainly need to **open the case
  and buzz out the PCB**, dump/disassemble the stock firmware, or obtain FCC
  internal photos / a schematic from Work Louder.

## Board disambiguation (do not mix these up)

| Board | MCU | USB VID:PID | Source |
|---|---|---|---|
| Creator Micro (v1, "Micro Pad") | **atmega32u4** (AVR) | `0x574C:0xE6E3` (legacy WL VID) | QMK `keyboard.json`, ForsakenRei repo |
| **Creator Micro 2** (this target) | **ESP32-S3** | **`0x303A:0x8298`** (also `0x8297` in same family) | wl-device-kit `DEVICE_REGISTRY`, microbridge, Gnome-Input rebuild notes |
| Creator Micro 2 in bootloader | ESP32-S3 ROM / USB-Serial-JTAG | `0x303A:0x1001` ("Espressif USB JTAG/serial debug unit") | Gnome-Input `native-gnome-rebuild.md` §4 |
| Codex Micro (OpenAI variant) | ESP32-S3 | `0x303A:0x8360` (`project_2077`) | microbridge `crates/mb-device/src/ids.rs` |

`0x8298` and `0x8297` both map to device-type string `creator_micro_v2` /
layout `universal` in the shipping kit (see
`Gnome-Input/sidecar/vendor/wl-device-kit/dist/index.js`:
`{ type: "creator_micro_v2", layout: "universal" }`).

---

## 1. MCU

| Item | Value | Confidence | Source |
|---|---|---|---|
| Family | ESP32-S3 | **confirmed** | `Gnome-Input/native-gnome-rebuild.md` L112 ("This is Espressif's VID — these are ESP32-S3 devices"), L75/L80 Input-app udev script comment "ESP32-S3", `esptool-js` target `esp32s3`, product uses native USB + BLE which S3 supports (S2 lacks BLE) |
| Exact package / revision | unknown | unknown | not exposed by any host tool (esptool would read it live over serial, but no capture exists) |
| Native USB | Yes — USB-Serial-JTAG (no DTR/RTS modem control) | **confirmed** | `native-gnome-rebuild.md` §8 "No DTR/RTS on native USB-Serial-JTAG" |
| Flash size | unknown (≥4 MB implied) | likely | flash layout uses app @ `0x10000`, partition table @ `0x8000`, 2nd-stage bootloader @ `0x0` (`native-gnome-rebuild.md` §7 L242-243). Standard S3 layout; total size not stated |
| Default flash baud | `921600` (irrelevant on USB-Serial-JTAG) | confirmed | `native-gnome-rebuild.md` §7 L241 |
| Firmware image | single merged full-flash image written at offset `0x0` | confirmed | `native-gnome-rebuild.md` §7 |

**Note:** ForsakenRei QMK repo says `atmega32u4` — that is the **v1** board, not
this one. Do not use it for the Micro 2 MCU.

## 2. Per-key RGB LEDs

| Item | Value | Confidence | Source |
|---|---|---|---|
| Type (WS2812/SK6812?) | unknown for Micro 2 | unknown | not in any host artifact |
| RGB data GPIO | **unknown** | unknown | not documented anywhere; host controls LEDs by logical index over RPC (`v.oai.thstatus` / `rgbcfg`), never a GPIO |
| LED count / order | unknown exact count; "RGB" per-key + underglow present | likely | product page: "RGB"; microbridge notes describe per-key RGB + underglow that "changes based on the app in focus" |
| Layout | logical only (UI `layout: "universal"`) | — | Input app `device_config_data-*.js` stores a UI key/LED layout, not physical wiring |

Possibly-transferable, **unconfirmed** (v1 atmega32u4 board, ForsakenRei
`work_louder/micro/config.h` + `keyboard.json`):
- Per-key RGB matrix + underglow, WS2812 on AVR pin **`D1`** (`"ws2812": {"pin": "D1"}`),
  underglow `RGBLIGHT_DI_PIN D2`, `led_count: 8`, plus extra LED pins
  `WORK_LOUDER_LED_PIN_1..3 = B6/B7/B5`. **These are AVR ports — meaningless as
  ESP32 GPIO numbers.**

## 3. Key matrix

| Item | Value | Confidence | Source |
|---|---|---|---|
| Key count | **13 mechanical switches** (+ 1 touch sensor) | confirmed | worklouder.cc/creator-micro-2 product page; microbridge `device-hid.md` "13 mechanical switches" |
| Matrix vs direct GPIO | unknown for Micro 2 | unknown | not documented |
| Row / col GPIOs | **unknown** | unknown | not in any host artifact |
| Diode direction | unknown for Micro 2 | unknown | (v1 was `COL2ROW`) |

Possibly-transferable, **unconfirmed** (v1 atmega32u4, ForsakenRei `keyboard.json`):
- 4×4 matrix, `cols: ["B4","C6","C7","E6"]`, `rows: ["F1","F4","F5","F6"]`,
  `diode_direction: "COL2ROW"`, custom matrix with two extra direct pins
  (`F7`, `F0`) read for row 3 cols 0/3 (`matrix.c`). **AVR ports; not ESP32.**

## 4. Rotary encoder

| Item | Value | Confidence | Source |
|---|---|---|---|
| Count | 1 rotary encoder (the "dial") | confirmed | product page: "1x Rotary encoder"; microbridge notes "rotary encoder" |
| A / B GPIO | **unknown** | unknown | not documented; host sees it as `encoderIndex` + rotate events over RPC |
| Switch (push) GPIO | unknown (dial press exists logically) | unknown | microbridge bring-up lists "Dial press" as a logical event |

Possibly-transferable, **unconfirmed** (v1 had **two** encoders, ForsakenRei
`keyboard.json`): `{"pin_a":"D4","pin_b":"D6"}, {"pin_a":"B0","pin_b":"B1"}`.
**AVR ports; and the Micro 2 has only one dial — do not assume these.**

## 5. Joystick

| Item | Value | Confidence | Source |
|---|---|---|---|
| Present | 1 planar joystick w/ rubber cap | confirmed | product page: "1x Planar joystick"; microbridge notes |
| Analog X/Y ADC GPIO | **unknown** | unknown | not documented; host receives polar `v.oai.rad` events (`a` angle, `d` distance) / "sectors", never raw ADC pins |
| Joystick button | unknown (radial-menu open is logical) | unknown | UI opens an on-screen radial menu from the joystick |

The joystick is exposed logically as angle/distance + a 7-slot radial menu
(worklouder marketing: "7 slot radial menu"), so firmware clearly does ADC →
polar conversion on-device, but the ADC pins are not surfaced to the host.

## 6. Other peripherals

| Item | Value | Confidence | Source |
|---|---|---|---|
| Touch sensor | 1x capacitive touch | confirmed | product page "1x Touch sensor"; microbridge "capacitive touch". GPIO unknown |
| Wireless | BLE (PRO) + USB-C; BASE is USB-C wired only | confirmed | product page; `native-gnome-rebuild.md` §4a (same vendor-HID protocol over USB or BT `uhid`) |
| Battery | 2100 mAh (PRO only) | confirmed | product page |
| Battery ADC / charger pin | **unknown** | unknown | host reads `batteryPercentage` over RPC (wl-device-kit), not a GPIO/ADC pin |
| Screen / display | **Not on Creator Micro 2** per product page. wl-device-kit ships LVGL/`screen`/`display` code, but that is for other WL models (e.g. Nomad), not this board | likely | product page lists no screen; kit `wl_lvgl` is generic |
| I2C for screen | n/a / unknown | unknown | no screen on this SKU |
| Vendor HID config channel | usage page `0xFF00`, 64-byte reports, JSON-RPC | confirmed | microbridge `docs/device-hid.md`; `native-gnome-rebuild.md` §5 |

---

## Firmware implications (what's blocking a from-scratch firmware)

**Known / usable now:**
- Target chip = **ESP32-S3** with native **USB-Serial-JTAG** (no auto-reset
  DTR/RTS — flashing tools must tolerate unsupported serial control-line calls).
- Flashing: merged image at `0x0` (bootloader `0x0`, partition table `0x8000`,
  app `0x10000`) — a conventional ESP32-S3 partition scheme; standard
  esptool/esp-idf workflow applies. Enter ROM download via `sys.bootloader` RPC
  or the hardware boot combo; device appears as `0x303A:0x1001`.
- Peripheral inventory is fully known: **13 mechanical switches, 1 capacitive
  touch, 1 rotary encoder (with press), 1 planar joystick, per-key RGB +
  underglow, BLE + USB-C, 2100 mAh battery (PRO)**.

**Unknown — every physical GPIO. These MUST be recovered before firmware can
drive the hardware, and there is no software source for them:**
1. Key matrix rows/cols (or direct-GPIO) pins, and diode direction.
2. RGB LED data pin(s), LED chip type (WS2812 vs SK6812), exact count and chain order.
3. Rotary encoder A/B pins + dial switch pin.
4. Joystick X/Y ADC pins (+ any button).
5. Capacitive touch pin.
6. Battery sense / charger (ADC or fuel-gauge I2C) pin(s).
7. Flash total size and any strapping/boot pins.

**How to recover them (in rough order of effort):**
- Open the case and continuity-test the PCB against the ESP32-S3 module pads
  (fastest reliable method).
- Dump the stock firmware over USB-Serial-JTAG (`esptool read-flash`) and
  disassemble / string-scan the app partition for `gpio_`, `esp_rom_gpio`,
  `rmt`/`led_strip` init, `adc`, and `touch_pad` config — pin numbers are baked
  into the binary even though the host protocol hides them.
- Check FCC internal photos (search Work Louder's FCC grantee ID on fccid.io) —
  I could not locate the specific filing in this pass.

## Sources

Local (read-only):
- `/home/silke/Documents/GitHub/microbridge/docs/device-hid.md` — VID/PID, 13 switches, RGB, HID protocol
- `/home/silke/Documents/GitHub/microbridge/docs/hardware-bringup.md` — PID `0x8298`/`0x8297` = Creator Micro V2
- `/home/silke/Documents/GitHub/microbridge/crates/mb-device/src/ids.rs` — `CREATOR_MICRO_V2_PIDS = [0x8297, 0x8298]`, VID `0x303A`
- `/home/silke/Documents/GitHub/Gnome-Input/native-gnome-rebuild.md` §4, §7, §8 — ESP32-S3, `0x303A:0x8298` = "Creator Micro 2", bootloader `0x303A:0x1001`, flash layout
- `/home/silke/Documents/GitHub/Gnome-Input/sidecar/vendor/wl-device-kit/dist/index.js` — `DEVICE_REGISTRY` `creator_micro_v2` / `layout:"universal"`; generic esptool-js flasher (no GPIOs)
- `/home/silke/Documents/GitHub/input-linux/input_work/app/dist-electron/scripts/install-udev-worklouder.sh` — VID `303a`, "ESP32-S3" USB-serial-JTAG comment
- `/home/silke/Documents/GitHub/input-linux/input_work/app/dist/assets/device_config_data-*.js` — logical UI layout only (joystick sectors, encoder index), no GPIOs

Web:
- Product page: https://worklouder.cc/creator-micro-2 (13 switches, touch, rotary encoder, planar joystick, RGB, BLE/USB-C, 2100 mAh)
- QMK v1 board (DIFFERENT MCU, atmega32u4): https://github.com/ForsakenRei/qmk-worklouder-micro — files `work_louder/micro/config.h`, `keyboard.json`, `matrix.c`
