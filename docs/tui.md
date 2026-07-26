# The `openmicro` TUI/CLI: design notes and hard-won knowledge

This document preserves the design decisions, hardware quirks, protocol
contracts and bug post-mortems that used to live as comments in
`crates/openmicro/src/`. The source is now comment-free; treat this file as
the crate's institutional memory. It is organised module by module, with the
cross-crate contracts and "do not do this" warnings called out explicitly.

## The application shape (`main.rs`, `app.rs`)

`openmicro` takes **no subcommands**. Running the binary opens a menu from
which every action is reachable: setting the device up, flashing firmware,
wiring coding agents, controlling the background service, and removing it all
again. The interface is a *linear prompt transcript* rather than a full-screen
app (in the style described by `docs/tui-style.md`): each action appends its
own record and returns to the menu, so the terminal ends up holding a readable
history of what was done.

### Menu and startup

- The wordmark is figlet's **ANSI Shadow** typeface, chosen to match
  `npx skills`. It is 74 columns wide; `WORDMARK_MIN_COLUMNS` (78) gates it so
  a narrow terminal skips it rather than wrapping it into nonsense. A test
  asserts the gate leaves room for the wordmark itself.
- The intro badge is black-on-cyan like `skills`' own badge. Colour `0` is
  black in the 256-colour palette — the styling toolkit has no dedicated
  `black` helper, hence `fg256(0, …)` under `bg_cyan`.
- At startup, if the daemon is installed but down, the app offers *once* to
  start it (almost everything else is more useful with it running). It stays
  silent when no service is installed — the question would only be noise, and
  the menu hint says so anyway.
- Cancelling an action (Esc/Ctrl-C) returns to the menu rather than quitting.
  Quitting is its own menu entry, so a stray Esc never loses work.

### `Snapshot` — the cheap menu state read

`Snapshot::read()` is **deliberately USB-only**: a Bluetooth scan takes
seconds, and redrawing the menu after every action must feel instant. The
guided setup does the slow, thorough probe behind a spinner instead.

`firmware_hint()` only claims a release version for the image at the download
cache path (`firmware::cache_image()`). `resolve_image` prefers a local
source build, and calling that build by the last downloaded release's name
would name the wrong binary. A test pins this
(`firmware_hint_prefers_the_downloaded_version_over_the_path`).

### `run_job` — the one spinner shape

Everything slow in the app has the same shape — spin, produce log lines or an
error, print them — so it lives in one place and every action reports success
and failure identically. On failure, tool output is mostly progress chatter;
**only the last line is the failure**. So the last line renders as an error
and the rest as plain info — rendering all of it as errors would bury the one
line that matters in a wall of red markers.

### Guided setup flow

1. Warns that OpenMicro replaces the vendor firmware (unsupported by the
   vendor, may void warranty; the way back is Firmware → Restore the stock
   firmware) and requires an explicit confirm (default **No**).
2. Probes. A device found over Bluetooth already running OpenMicro firmware
   skips flashing entirely and goes straight to agent wiring.
3. Enters bootloader mode **without buttons**: the firmware is asked to
   reboot itself (see `wldevice`).
4. Obtains an image (existing / download / build), confirms, checks the
   serial port is uncontended, flashes, then starts the firmware.

### Why flashing does not restart the device — and `start_firmware` does

Flashing deliberately leaves the device in the bootloader: entering the
bootloader set the **force-download bit**, which *survives a reset*, so a
device that simply rebooted would come straight back to download mode and
never run the firmware that was just written. Clearing the bit and resetting
(`wldevice::exit_bootloader`) is what actually starts the new firmware. The
TUI's "Starting the new firmware" step exists purely for this.

### Serial-port contention (`port_is_clear`)

Interrupting the user here is worth it: **ModemManager opening the port
mid-transfer kills a 4 MiB transfer partway through** with an error that reads
like a hardware fault. The flow warns and lets the user stop and fix it first.

### Firmware menu specifics

- A forced `$OPENMICRO_FIRMWARE_URL` bypasses the release list entirely, so
  there is no version to pick — the menu instead shows where the image will
  come from.
- A release with no flashable asset is still listed (its hint says why), but
  picking it must *say so* rather than silently behaving like "Back".
- Restoring stock firmware is possible because Work Louder publish
  unencrypted merged images to a public GitHub repo — going back is just a
  download. The restore flow warns that it overwrites OpenMicro, names the
  exact image path being written, and defaults the confirm to No.

### Agents flow

- Warns when `openmicro-hook` is not on `PATH`: hooks will install but stay
  inert until it is.
- Agents already wired appear in a locked "Already wired up" section; agents
  detected-but-unwired are preselected.
- `install_agents` installs each chosen agent even if one fails — the user
  picked several and deserves the ones that worked — but the overall result
  is an `Err` whenever anything failed, so the UI can never show a clean
  success for a partial run.

### Live view (`live_status`, `spawn_snapshot_reader`)

- `live_frame` is **pure**, so the layout is testable without a daemon, a
  terminal, or a device. The focused (owner) session is the one the macropad
  is actually showing; it gets the `▸` marker and bold row.
- The `stop` flag is essential: it is dropped/set when the screen exits, and
  that is what stops the background socket-reader thread. Without it, every
  visit to this screen **leaked a thread reconnecting forever**, and the
  daemon spawns a 1 Hz writer task per connection — so each visit also leaked
  a daemon-side task.
- The socket read blocks, so the flag is only checked between lines and
  between reconnects; the reader thread may therefore outlive the screen by
  at most one snapshot (the daemon writes at 1 Hz). What matters is that it
  does not outlive it forever.
- This is the one screen that is not a prompt: it redraws on a timer rather
  than on input, but uses the same frame machinery as the prompts so it
  collapses into the transcript the same way when it exits.

### Settings menu

- Every change is sent to the daemon, which applies it live and persists it —
  there is **no separate save step**.
- Hints are seeded from the daemon's live config (`read_current_settings`) so
  the menu shows what settings currently *are*, not just what they could be.
  That helper returns `None` rather than an error: not knowing the current
  brightness is not worth interrupting the user over — the menu says
  "unknown".
- Colour identifies **which agent**, not what it is doing — the LED effect
  and underglow motion carry the state. So the picker selects a colour per
  agent, and the hint shows the current one.

### Uninstall flow

Before removing anything, the confirmation manifest names the **actual
paths**: "Settings" is not enough to consent to deleting a directory, and the
user cannot otherwise see what a label covers. Failures are reported per
target and the summary is shown as an error if anything failed.

## Coding-agent adapters (`agents.rs`)

Every OpenMicro adapter ultimately does one thing: make the agent run
`openmicro-hook` on lifecycle transitions (see `adapters/README.md`). This
module turns those hand-written install docs into something the TUI can do
for the user. The file-mutating half is deliberately thin: all the
interesting logic (`merge_claude_hooks`, `insert_codex_notify`, the
`*_installed` predicates) is pure string-in/string-out so it is unit-tested
without touching a real `~/.claude/settings.json`.

### The event contract

`HOOK_EVENTS` maps the four lifecycle transitions OpenMicro cares about, as
(Claude-compatible event name → OpenMicro state):

| Event | State |
|---|---|
| `UserPromptSubmit` | `thinking` |
| `PreToolUse` | `working` |
| `Notification` | `awaiting_approval` |
| `Stop` | `idle` |

`HOOK_MARKER` (`"openmicro-hook"`) is the substring that identifies a hook
command as ours; it is what keeps installation idempotent (we never append a
second copy of our own hook) and what scopes uninstall to our entries only.

### Per-agent mechanisms

- **Claude Code** — JSON hooks in `~/.claude/settings.json`.
- **Grok Code** — same mechanism, but in `~/.grok/user-settings.json` and
  with `--agent grok` passed to the hook.
- **Codex CLI** — a `notify = ["openmicro-hook", "codex-notify"]` **root**
  key in `~/.codex/config.toml`.
- **opencode** — a whole plugin file at
  `~/.config/opencode/plugin/openmicro.js`. opencode auto-loads every file in
  its plugin directory, so unlike the other adapters there is nothing to
  merge: the file is ours entirely; installing overwrites it and uninstalling
  deletes it. The plugin's first line is a JS comment
  (`// Installed by OpenMicro …`) which doubles as the ownership marker.

Detection uses two signals: config-directory markers under `$HOME`
(e.g. `.claude`/`.claude.json`) **or** an executable on `PATH` — the latter
catches a fresh install that has never been run and so has no config
directory yet. `has_marker` is split out from `is_present` because it is the
only half that respects the `home` argument; `is_present` also consults the
real `PATH`, which no scratch directory can control, so a test written
against it would pass or fail depending on what the machine running it
happens to have installed. Tests assert through `has_marker` for exactly this
reason.

`home()` falls back to `/` when `$HOME` is unset (practically impossible), so
callers never need their own error branch. `read_config` returns `""` for a
file that does not exist yet — an absent file is a perfectly installable
starting point — and a whitespace-only config is treated as `{}` so a brand
new `settings.json` merges instead of failing to parse.

### Claude-style JSON merge rules

`merge_claude_hooks` is pure (contents in, contents out) and:

- preserves every unrelated key — serde_json's `preserve_order` feature keeps
  the user's key order too;
- keeps existing user hook groups for the same events alongside ours;
- is idempotent: a group already invoking `openmicro-hook` is never
  duplicated;
- **refuses** (errors) when the file is not a JSON object, or when `hooks` /
  `hooks.<event>` exist with an incompatible shape — refusing beats
  clobbering the user's config.

`remove_claude_hooks` is the exact inverse and equally conservative: only
groups whose command mentions `openmicro-hook` are dropped; the user's own
hooks for the same events stay; an event array (or the `hooks` object) left
empty *by the removal* is deleted so uninstalling leaves the file as it was
found rather than littered with empty scaffolding. Crucially, **only keys the
removal pass actually emptied are dropped** — an event the user left empty on
purpose (e.g. `"SessionStart": []`) is theirs and was never ours to tidy
away. A file that existed only to hold our hooks reduces to the empty string,
which the caller turns into deleting the file.

### Codex `notify` rules

TOML requires root keys to appear **before** the first `[table]`, so
`insert_codex_notify` inserts near the top of the file (after any leading
comment/blank block, so a file that opens with a header comment keeps reading
naturally) rather than appending. A `# OpenMicro …` comment line is written
above it. Re-running is a no-op.

`root_notify_line` finds the first `notify =` assignment *before* any
`[table]` header: a `notify` inside a table is a **different key** and must
be ignored (a test pins that `[mcp_servers.x] notify = […]` does not count as
a conflict). If a root `notify` already points at something else, install
**errors** rather than overwriting — overwriting would silently break
whatever the user had wired up; the error tells them to chain or remove it
themselves.

`remove_codex_notify` only removes a `notify` that mentions our marker
(someone else's notify is not ours to touch), and also drops the comment we
wrote immediately above and the blank line we inserted below, so removal
leaves no trace. A file left entirely blank becomes the empty string
(→ deleted).

### opencode plugin rules

- `opencode_plugin_installed` requires byte-for-byte equality with the
  current `OPENCODE_PLUGIN` text. An **older** OpenMicro plugin therefore
  counts as *not installed*, so re-running the installer refreshes it rather
  than reporting "already installed" and leaving a stale file behind.
- `opencode_plugin_is_ours` (contains the marker) decides deletion on
  uninstall — a plugin the user wrote themselves is never overwritten or
  deleted; install against a foreign file errors and tells the user to move
  it aside.
- The plugin's text *is* the whole adapter contract: it must push all four
  states (`thinking`, `working`, `awaiting_approval`, `idle`), handle
  `session.idle`, `permission.ask` and `permission.replied` — if a state
  stops being pushed the macropad silently shows stale colours. A test
  asserts each of these strings is present in the plugin source.

### Write discipline

Installs and uninstalls are **atomic and non-destructive**: the merged config
is written to a `<file>.openmicro.tmp` sibling then renamed over the
original; before an existing file is replaced its previous contents are
backed up to `<file>.openmicro.bak` (only when the file actually existed — a
first-time install has nothing to preserve). An unchanged config is not
rewritten at all. On uninstall, a config that became empty is deleted
outright rather than left as an empty file the agent might not expect, and
the install-time backup is removed too (it has served its purpose).

`sibling()` appends the suffix to the **whole** filename, extension included
(`settings.json` → `settings.json.openmicro.bak`): that keeps the original
name recognisable and never collides with the live config, unlike replacing
the extension.

`hook_binary()` looks for `openmicro-hook`: hooks call it by bare name, so it
must be resolvable from the agent's environment — a `None` is worth warning
about before installing.

## Flashing (`flash.rs`)

Split into pure, unit-tested helpers (image resolution, USB classification,
argv construction, tool discovery) and thin side-effecting drivers.

**Honesty contract:** the flash path is designed to stop with a clear,
actionable error at the first missing prerequisite. It NEVER reports a
successful flash it did not perform. (Historically this crate was developed
on a machine with no Xtensa toolchain and no way to enter the bootloader
unattended, so the error paths *are* the product.)

### Image resolution

Order: an explicit `--image` wins but must exist and must not be a directory
(a directory technically "exists" but is never a valid image — it must fail
clearly rather than be handed to esptool). Otherwise the default build output
`firmware/target/xtensa-esp32s3-none-elf/release/openmicro-fw` is looked up
against the current directory and every ancestor containing a `firmware/`
dir; failing that, the download cache. A local source build **wins over a
stale download**. `resolve_default` takes its roots and cache path explicitly
so the "nothing found" path is testable — reading the real cache directory
made the test depend on whether the developer had ever downloaded firmware.

### Flash layout and esptool argv — the reset options are not incidental

A single **merged image** is written at offset `0x0` (bootloader `0x0`,
partition table `0x8000`, app `0x10000` are all inside it), per
`docs/hardware/creator-micro-2-pinout-research.md`.

- `--before usb-reset` drives the USB-Serial-JTAG reset. It works whether or
  not the chip is already in download mode — which matters because the USB id
  alone cannot tell those apart: `303a:1001` is both the ROM bootloader and
  any firmware exposing the same console. A previous `no-reset` version
  assumed download mode and failed with a bare "Write timeout" against a
  device that was merely running.
- `--after no-reset`: entering the bootloader via `sys.bootloader` set the
  force-download bit, and it survives a reset — resetting here would just
  land back in download mode. The caller clears the bit and resets; see
  `wldevice::exit_bootloader`.
- `--after watchdog-reset` (used on exit) is the only reset that works on
  this board: native USB-Serial-JTAG means no DTR/RTS lines for the classic
  auto-reset.

### esptool version differences

esptool 5 renamed every subcommand (`read_flash` → `read-flash`) and prints a
deprecation warning for the old spelling; esptool 4 only knows the old one.
`esptool_major` asks `esptool version` — asking is cheap and beats guessing
wrong in either direction — and `subcommand()` spells accordingly.

### Tool discovery

`which()` honours `$PATH`, then falls back to `~/.local/bin` and
`~/.cargo/bin` — where `pip install --user`, `uv tool install` and
`cargo install` put things without always being on the PATH of a non-login
shell.

### ELF vs merged image — do not hand an ELF to esptool

`is_elf` checks the `\x7fELF` magic. This distinguishes the two image kinds:
the `firmware/` build output is an **ELF**, a downloaded release asset is a
**merged flash image**. Guessing from the file extension is not good enough —
the build output has none. The distinction is load-bearing:

- An ELF requires `espflash`, which derives the bootloader, partition table
  and app offsets itself. Handing a bare ELF to `esptool write_flash 0x0`
  would write a file the chip cannot boot — the code **refuses** rather than
  produce a bricked device.
- A merged `.bin` works with either tool; `esptool` is preferred because that
  is the documented layout.
- `espflash` is invoked with `--non-interactive` so it does not drop into its
  serial monitor: the TUI owns the terminal and a monitor would never return.

### ModemManager (`port_contention`)

ModemManager probes every new `ttyACM` device — on this board that means it
opens the very port a flash is streaming over. The symptom is a transfer that
dies partway through with "Packet content transfer stopped", which looks like
a hardware fault and is not one. The warning suggests
`sudo systemctl stop ModemManager` and points at `docs/troubleshooting.md`
for a permanent udev exclusion.

### Bootloader gating (`require_bootloader`)

The USB id is only a hint: `303a:1001` is what the ROM bootloader looks like
*and* what any firmware exposing the USB-Serial-JTAG console looks like —
OpenMicro's own included. So a device carrying that id is accepted, and the
esptool invocation resets it into download mode itself (`--before
usb-reset`). What must be rejected: a device that is absent, or one still
running the **vendor** firmware on its own product id — that one cannot be
reset this way and needs the HID route first.

## Getting firmware (`firmware.rs`)

Two ways to obtain an image: **build** the `firmware/` crate, or **download**
a prebuilt release. Building needs the Xtensa Rust toolchain (`espup`) — a
large one-time install absent on most machines; downloading needs a published
release. Neither is guaranteed, so every entry point reports precisely which
of the two is available and why the other is not, and never claims to have
produced an image it did not produce.

### URLs, caches and overrides

- Release list: `https://api.github.com/repos/SilkePilon/OpenMicro/releases`
  (assets are attached by `.github/workflows/firmware.yml`). Overridable via
  `$OPENMICRO_RELEASES_URL` — a fork, a mirror, or a `file://` path (which is
  how the fetch+parse path is exercised end-to-end in tests: curl reads
  `file://` URLs without touching the network).
- `$OPENMICRO_FIRMWARE_URL` bypasses the release list entirely and downloads
  one specific URL (self-hosted or locally built image). When this succeeds,
  the cached version file is **removed**: the cached image is now a
  completely different binary, and leaving the old release tag behind would
  have the menu offer "use the firmware I already have — version vX" for
  something that is not vX.
- Stock firmware: Work Louder publish the stock Creator Micro 2 firmware at
  `https://api.github.com/repos/worklouder/cm-v2-fw-releases/releases`, as
  **unencrypted merged flash images**. This is what makes going back to the
  vendor firmware possible without having taken a backup first. Overridable
  via `$OPENMICRO_STOCK_RELEASES_URL`.
- Download cache: `~/.cache/openmicro/firmware/openmicro-fw.bin`, with a
  sibling `.version` file recording which release tag the cached image came
  from (so the CLI/wizard can say *which version* is cached; writing it is
  best-effort — a failure only costs the ability to display it later).
- Stock images are cached **per-version** (`stock-<sanitised-tag>.bin`) and
  separately from our own image, so restoring never races with flashing
  OpenMicro.

### Building

The Xtensa toolchain only exists inside the environment `~/export-esp.sh`
sets up, so the build runs under `sh -c "set -e; . <export>; cd <dir>; cargo
build --release"` — `build_script` is pure so the exact invocation (including
`set -e`, so a failed source aborts) is unit-tested. `shell_quote`
single-quotes with `'\''` escaping and round-trips through a real shell in a
test. After a successful exit code the artifact's existence is confirmed via
`resolve_image` — **trust the filesystem, not the exit code**.

### Release parsing and picking

`parse_releases` is pure so the shape of the GitHub API response is pinned by
tests rather than discovered at runtime. Rules:

- Drafts are dropped (not downloadable).
- A release with no matching asset is **kept but marked uninstallable**
  (`blocker()`), which is more honest than silently omitting a version the
  user can see on GitHub.
- A rate-limited or errored API responds with an *object*, not an array; its
  `message` field is surfaced in the error.
- Our asset matcher: name ends `.bin` and contains `openmicro` (never the
  checksums file). Stock matcher: ends `.bin` and contains `merged` — the
  "merged" part matters because the vendor also ships per-partition images,
  and **only the merged one can be written at offset `0x0`** the way
  `flash.rs` does it.
- `pick_release` with an explicit version accepts only an exact tag match —
  silently falling back to a different version is the last thing anyone wants
  from a firmware installer; the error lists what is available. Without one,
  the newest release that actually has an asset wins, preferring stable over
  pre-release.

### Downloads are atomic and paranoid

curl is used with `--fail` (an HTML 404 page is never mistaken for firmware),
`--location` (the "latest release" URL is a redirect), `--retry 3`, and a
User-Agent header — **GitHub's API rejects requests without one**. The file
is written to a `.part` temp path and renamed into place only after a
successful transfer, so a half-downloaded image can never be flashed. An
empty download is refused outright.

### Test infrastructure warning

Environment variables are process-wide, but `cargo test` runs tests on
parallel threads: every test that sets one must hold the shared `ENV_LOCK`
mutex, or another test will observe it (two tests once raced over
`OPENMICRO_RELEASES_URL`). The lock is poison-tolerant so one failing test
does not cascade into the rest.

## Vendor HID control (`wldevice.rs`)

This module is the software route into and out of the ESP32-S3 ROM
bootloader. Background: the Creator Micro 2 has **no usable boot-button
workflow**. Its stock firmware enumerates as a composite HID device on its
own product id, so the hardware USB-Serial-JTAG peripheral is not on the bus
at all and esptool's usual reset dance has nothing to talk to. Work Louder's
own tooling instead asks the *firmware* to reboot into download mode, over a
vendor HID interface.

### Wire format (contract)

Matches `@worklouder/wl-device-kit` and was independently confirmed against
microbridge's `mb-device` crate, which drives the same hardware:

```
interface: usage page 0xFF00 on VID 0x303A
report:    [0]=0x06 report id, [1]=channel, [2]=payload len (0..=61), [3..]=UTF-8
payload:   {"method":"sys.bootloader","params":null,"id":<0..999>}
```

- Channel `2` is the JSON-RPC stream; channel `1` is device debug logging.
- Reports are 64 bytes, zero-padded (a test asserts the padding is zeroes,
  not stack garbage). Messages longer than 61 payload bytes split across
  consecutive reports; an empty message still produces one zero-length
  report, matching the vendor implementation.
- The RPC request is **hand-built, not serialized from a struct, on
  purpose**: the firmware wants the compact form with *no* `"jsonrpc":"2.0"`
  member, these keys in this order, and it **rejects ids of 1000 or more**.
  Request ids are not random — just varied enough (pid + counter, mod 1000)
  to match replies within a session.

Coming back out is a different mechanism entirely: once in download mode the
device is a plain CDC serial port speaking the esptool protocol, and the only
reset that works on USB-Serial-JTAG is the RTC **watchdog** reset.

### USB identifiers

| Constant | Value | Meaning |
|---|---|---|
| `WL_VID` | `0x303A` | Espressif's vendor id, which Work Louder ships under |
| `APP_PIDS` | `0x8297`, `0x8298`, `0x8360` | firmware running: two Creator Micro 2 revisions + Codex Micro |
| `CODEX_MICRO_PID` | `0x8360` | the same hardware under ChatGPT branding |
| `BOOTLOADER_PID` | `0x1001` | ESP32-S3 ROM bootloader (USB JTAG/serial debug unit) |
| `WL_USAGE_PAGE` | `0xFF00` | vendor HID usage page carrying the RPC channel |
| `RTC_CNTL_OPTION1_REG` | `0x6000_812C` | register holding the force-download-boot bit |

The force-download bit set by `sys.bootloader` lives in
`RTC_CNTL_OPTION1_REG`, and **on the battery-backed Pro model it survives an
unplug** — so it must be cleared explicitly, or the device re-enters download
mode on every boot.

Beware: Espressif ships a lot of devices under `0x303A`; classification only
recognises our specific PIDs.

### The `303a:1001` ambiguity — the central hazard of this module

`303a:1001` is both the ESP32-S3 ROM bootloader *and* any firmware exposing a
USB-Serial-JTAG console — which OpenMicro's firmware does, for its logs.
**Presence of that id on the bus is therefore not evidence of download mode;
only a successful esptool sync is.** This shaped several functions:

- `classify` is pure over `(vid, pid)` pairs and reports the conservative
  answer (`Bootloader` wins if both are somehow seen, since that is the state
  flashing cares about). It cannot resolve the ambiguity — that needs I/O.
- `resolve_ambiguous(mode, firmware_answered)` encodes the rule: the ROM
  bootloader never answers, so a reply means firmware is running
  (`App(BOOTLOADER_PID)`). It never changes an unambiguous answer — a stray
  reply must not rewrite `App` or `Absent`. This rule was split from the I/O
  so it is testable; it fixes a real bug where **the TUI said "bootloader
  mode" forever**, including immediately after the device was told to go back
  to normal.
- `usb_mode_raw()` reads sysfs only — cheap, side-effect free, leaves the
  ambiguity unresolved. Use it anywhere the answer only distinguishes
  "something on the bus" from "nothing", **and in any loop**: the probing
  variant can wait up to `IDENTIFY_TIMEOUT` (~600 ms), which is not something
  to do every 250 ms. A test asserts `usb_mode_raw` completes in well under
  `IDENTIFY_TIMEOUT / 2` — callers rely on it doing no device I/O.
- `usb_mode()` pays for the probe only when the id cannot settle it. And if
  the **daemon is running, that is already the answer**: the daemon only ever
  connects to live firmware, and probing behind its back would put two
  readers on one character device — each taking a share of the bytes, so the
  daemon could swallow our banner (menu reports "bootloader mode" for a
  working device) and we could swallow its key presses.
- `download_mode_responds` runs `esptool … --before no-reset --after
  no-reset chip-id`: it succeeds only if a ROM bootloader is actually
  listening. `sync_args` must never reset the chip — it runs against a device
  that may be mid-boot, and resetting would change the very state being
  observed.

### Entering the bootloader (`enter_bootloader`)

- Success is defined as **the bootloader actually showing up on USB — not
  the HID write succeeding** — because the device frequently drops off the
  bus before it can acknowledge, so a write error is not evidence of failure.
  This mirrors what Work Louder's own tool does. Only if the device never
  re-enumerates (10 s timeout) does the write error become the useful part of
  the story.
- It checks the current mode with the **raw** (non-probing) read: the
  `download_mode_responds` check just after resolves the ambiguity better —
  it asks the bootloader directly instead of inferring from our firmware's
  silence.
- If the ambiguous id is present but nothing answers a sync, the device is
  assumed to be OpenMicro firmware on USB-Serial-JTAG and is reset with
  `--before usb-reset` (`usb_reset_into_bootloader`). This route needs no
  vendor RPC because the peripheral esptool drives is on the bus — and it
  works even when the firmware has crashed, which is exactly when it is most
  needed. If that also fails, the fallback advice is to hold the power button
  ~8 seconds.
- The HID write path (`send_rpc`) tries the interface whose usage page is the
  vendor one first; if the HID backend does not report usage pages (which
  happens on some Linux setups) it falls back to every other interface of a
  matching device, since writing to the wrong one merely errors. Open
  failures append a hint about hidraw permissions.

### Leaving the bootloader (`exit_bootloader`, `exit_args`)

Two steps, **both required, in one esptool invocation**: clear the
battery-backed force-download bit (`write-mem 0x6000812C 0`) so the device
does not simply re-enter download mode, then perform an RTC **watchdog**
reset (`--after watchdog-reset`). A plain reset does not re-sample the boot
straps on USB-Serial-JTAG, and the board exposes no DTR/RTS lines for the
classic auto-reset, so watchdog-reset is the only thing that works.
`exit_args` is pure so the exact argv is pinned by a test — getting `--after`
wrong leaves the device stuck re-entering download mode on every boot.

Also deliberate: `--before usb-reset` rather than `no-reset` here, because by
the time this runs the device may already have rebooted (a flash resets on
its way out), and `no-reset` against a device not sitting in download mode
just times out.

`exit_bootloader` checks presence with the **raw** read, never the probing
one. It runs immediately after flashing, when the device is sitting in the
ROM bootloader and therefore silent — and the only question is "is anything
on the bus", which the id answers by itself. Probing here is what once hung
the wizard at "Starting the new firmware": the step that exists to get *out*
of the bootloader was waiting on a reply that a bootloader never sends.

### The ignored hardware test (`bootloader_and_back`)

Ignored by default — needs a real device on USB, and resets it:

```
cargo test -p openmicro -- --ignored --nocapture bootloader_and_back
```

It exists because the wizard hang above could not have been caught by any
unit test: the cause was a blocking read on a character device, and the unit
tests stood in with ordinary files, which report end-of-file at once. Only
real hardware is silent the way a ROM bootloader is silent. The test asserts
the thing that cannot be faked — that the enter/exit sequence finishes at all
(bounded at 90 s; the bug was unbounded, so any finite bound catches it), and
that firmware genuinely answers afterwards (the USB id cannot prove that,
since the ROM bootloader shares it).

## Display-mode switching (`display.rs`)

### Why serial and not the daemon

The obvious route would be a `Command` to the daemon, passed on to the
device. That does not work yet: the daemon reaches the device over BLE, and
the firmware's GATT server is still a sketch, so nothing the daemon sends
arrives. The firmware *does* expose USB-Serial-JTAG for its logs, and the RX
half of that stream is free — so a short command down the same port switches
modes on an already-flashed device, with no BLE and no rebuild. **When the
GATT server lands, this should move onto the daemon path**, and this module
becomes the fallback for a device with no daemon.

Sharing one stream with the log output has two consequences that everything
in this module is shaped by: the port must be opened **raw and
non-blocking**, and commands must carry a **prefix**.

### The command prefix (`!`) — firmware contract, do not remove

The prefix is not decoration; dropping it breaks the device. The port carries
the firmware's log output *and* its command input, and a tty in its default
line discipline **echoes whatever it receives straight back out** — so every
log line the firmware printed arrived back at the firmware as input. With
bare letters as commands that is a feedback loop: `link:` contains `i`
(identify) and `n` (normal), and any line containing a `d` put the board into
demo mode on its own. A byte the logs never emit ends the loop. Tests pin
that the prefix is neither alphanumeric (appears in ordinary words) nor
whitespace (appears in every line).

`COMMAND_PREFIX` (`b'!'`) **must match `COMMAND_PREFIX` in
`firmware/src/main.rs`**. The mode command bytes — `n` (normal), `d` (demo),
`i` (identify) — **must match `handle_serial_command` in
`firmware/src/main.rs`**; changing one side without the other leaves the menu
silently doing nothing. Command bytes must also never be whitespace: the
firmware skips whitespace (so a line-buffered terminal's newline is not read
as an unknown command), which means a whitespace command byte would be
silently dropped. A trailing newline is sent after each command so the same
thing can be typed into a terminal by hand; the firmware ignores it.

### Port discovery

The ESP32-S3's USB-Serial-JTAG shows up as an ACM device; candidates are
`/dev/ttyACM0..3` in order. Only a handful ever exist, so an ordered guess
beats depending on a port-enumeration crate.

### `open_raw` — both properties are load-bearing

- **Non-blocking** (`O_NONBLOCK`, `VMIN=0`, `VTIME=0`): a `read` on a tty
  with nothing to say blocks forever. The first version of
  `firmware_answers_on` looped `while now < deadline` around a blocking read,
  so the deadline was only ever checked *between* reads and the first read
  never returned. A device sitting in the ROM bootloader is silent by
  definition, which meant the identify probe hung the setup wizard at
  "Starting the new firmware" — the one step whose job was to get the device
  out of the bootloader.
- **Raw** (`cfmakeraw`): the default line discipline echoes received bytes
  back to the sender and rewrites the stream on the way through (see the
  prefix story above).
- A plain file is not a tty; the termios setup is skipped for one rather than
  failing, because the tests exercise the framing over ordinary files.
- Safety notes for the two `unsafe` blocks: the `CString` path outlives the
  `libc::open` call and is NUL-terminated; the fd is handed straight to
  `File::from_raw_fd`, which takes ownership and closes it. In `make_raw`,
  `tcgetattr` fully initialises the zeroed `termios` before anything reads
  it; a non-tty simply returns early.

`write_all_raw` retries `WouldBlock`/`Interrupted` with 5 ms sleeps under a
500 ms deadline — a non-blocking tty can report short stalls.

### Firmware identification (`firmware_answers_on`)

`FIRMWARE_BANNER` (`"openmicro-fw"`) is the reply to `!?` and **must match
`IDENTITY` in `firmware/src/main.rs`** — if these drift, the TUI silently
reports every running device as being in bootloader mode (a test pins the
exact string). `IDENTIFY_TIMEOUT` is 600 ms: the firmware answers
immediately; the ROM bootloader answers never — the timeout is purely how
long "never" takes.

The read loop reads in slices rather than to end-of-file: this is a character
device, so there **is no EOF**, and the firmware is also emitting its own
periodic log lines that must be read past. `WouldBlock` is the normal
"nothing yet" answer on a non-blocking port; the deadline is what actually
bounds the loop — with a blocking descriptor a silent device never returns
from the first read at all, and no deadline written in the loop can help. The
accumulation buffer is bounded (cleared at 8 KiB) so a chatty device cannot
grow it forever.

### Sending a mode (`send`)

The daemon is stood down for the write and started again afterwards
(`daemon::with_paused`): it holds the same port, and two readers on one
character device take a share of the bytes each. Demo and identify keep
running across the daemon restart because the firmware ignores host frames in
those modes.

### Test-suite warnings worth keeping

- The silent-tty regression test uses a **FIFO with no writer** because a
  plain file cannot model a quiet device (it reports EOF straight away) —
  that is exactly how the original bug slipped through. With a blocking
  descriptor the test does not fail, it *hangs*, which is what the wizard
  did.
- **FIFO wrinkle — do not "simplify" the probe into a single read:** a FIFO
  opened `O_RDWR` loops back, so the first read returns the probe's own `!?`
  bytes and it is the *second* read that blocks. Verified by hand.

## Daemon control (`daemon.rs`)

Two independent questions, deliberately kept apart because they disagree more
often than expected:

- **Is it running?** Answered by connecting to the control socket. That is
  the only thing that matters to the rest of the app, and it is true whether
  the daemon was started by systemd, by hand, or from a different session.
- **Is it installed as a service?** Answered by looking for the systemd user
  unit file. This decides whether "start it for me" is even on offer.

Details and contracts:

- The control socket path comes from `openmicro_proto::paths::control_socket()`
  (a cross-crate contract with the daemon; it ends in `openmicro-ctl.sock`
  under the runtime dir).
- The unit is `openmicrod.service`; its path honours `$XDG_CONFIG_HOME` and
  falls back to `~/.config/systemd/user/`. `packaging/install.sh` installs
  it, and the "no unit" error says so.
- `is_socket_live` **connects rather than stats**: a stale socket file left
  behind by a crashed daemon does not count — a stat-based check would call
  that "running" and every later screen would show a disconnected daemon it
  thought was up. It takes the path as an argument so it is testable without
  mutating process-wide state.
- `have_systemctl` exists because systemd is not usable in a container or on
  a non-systemd distro — the UI should say so rather than offering to fail;
  the error tells the user to run `openmicrod` by hand.
- `start()` waits (5 s) for the socket after `systemctl start`:
  systemd reporting success only means it *forked* the daemon; the socket is
  what the rest of the app depends on. Reporting success before the socket
  exists would make the very next screen show "disconnected".
- `wait_until_stopped` exists because `systemctl stop` returns once systemd
  is satisfied, which is not quite the same instant the daemon's file
  descriptors are closed.
- `with_paused(job)` — the serial-port sharing rule again: the daemon holds
  the device's serial port for as long as it is up, and two readers on one
  character device split the byte stream. Anything talking to the device
  directly must have the port to itself, so the daemon is stood down for the
  duration and put back afterwards. A daemon that was not running is left
  alone — including afterwards, so this never *starts* a service the user
  deliberately stopped. A daemon that is running but has no unit cannot be
  paused (error: stop it by hand). Restarting is best-effort: the job's
  result is what matters, and a failure to bring the daemon back is appended
  to the log rather than masking it.

## Probing (`probe.rs`)

Answers: is the macropad plugged in, is it reachable over Bluetooth, and
which firmware is it running? The setup wizard branches entirely on this, so
the classification rules are pure functions over a `Probe` value; the I/O
lives in one place. The BLE half needs a bounded scan (seconds), so callers
run `probe()` on a worker thread and keep rendering.

- `BleState::Absent` vs `BleState::Unavailable` is a real distinction: "we
  looked and found nothing" vs "we could not look" (no adapter, `bluetoothd`
  down, scan failed to start). They lead to different wizard text; conflating
  them would tell the user to plug in a cable for a device that is not even
  paired. `Unavailable` must never be reported as a reachable connection.
- Stock-firmware detection over BLE is a **heuristic** — the stock firmware's
  advertised name is not documented. The hints are lowercase fragments:
  `creator micro`, `codex micro`, `micro 2`. Positive identification of
  OpenMicro firmware is the advertised GATT service UUID
  (`OPENMICRO_SERVICE_UUID`) or the `ADV_NAME_PREFIX` name prefix — both from
  `openmicro_proto::ble`, a cross-crate contract with the firmware.
- `connection()`: a cable wins over BLE because it is the only transport that
  can flash. `firmware()`: bootloader mode is reported before any firmware
  guess (no firmware is running then, and it is the state the flashing flow
  waits for); the OpenMicro BLE service is the only positive identification;
  a vendor USB product id means stock.
- `probe()` skips the BLE scan whenever USB already answers the question: the
  scan costs seconds and holds the Bluetooth adapter — and a cabled device is
  exactly the case where the wizard needs to react quickly.
- `scan_ble` builds its own single-threaded Tokio runtime so the synchronous
  TUI can call it from a worker thread without a runtime of its own. Any
  failure to reach BlueZ is `Unavailable`, not an error. There is a **hard
  outer bound of `timeout * 2`** around the whole async scan: the discovery
  loop already stops at `timeout`, but the BlueZ handshake before it
  (session, adapter, `set_powered`) is unbounded D-Bus I/O — and a scan that
  never returns would wedge the wizard's poll loop, which only ever keeps one
  probe in flight.
- In the scan loop, a positive OpenMicro identification returns immediately;
  a stock-looking device is only remembered as the best answer so far,
  because it may be sitting next to an OpenMicro one still to be discovered.

## Uninstall (`uninstall.rs`)

Deliberately **itemised rather than a single "remove everything" button**:
the agent hooks live inside config files the user owns, and each category
deserves a separate, informed yes. `survey` answers "what is actually here"
so the UI offers only things that exist; `remove` does the work and reports
per-target outcomes.

- `ALL_TARGETS` order is the removal order: **Service first, so nothing is
  holding files open**. `remove()` re-sorts whatever selection order the user
  made into this order (a test pins it).
- The three binaries `packaging/install.sh` puts in `~/.local/bin`:
  `openmicrod`, `openmicro`, `openmicro-hook`.
- Config is `~/.config/openmicro` (settings and the first-run marker); cache
  is `~/.cache/openmicro` **and** `~/.local/share/openmicro` (downloaded
  firmware, re-downloadable).
- Service removal asks systemd to disable and stop **before** the unit file
  goes, so it does not linger as a failed unit until the next daemon-reload;
  a `daemon-reload` follows the file removal. "Not installed / not enabled"
  from `systemctl disable` is a perfectly fine state to be uninstalling from
  and is logged, not fatal.
- Agent hooks are removed by `agents::uninstall` per agent (merge back out of
  the user's config, never delete the user's own settings).
- `remove_path` returns `Ok(false)` for "was not there to begin with" — a
  success for uninstall purposes, not an error.
- Best-effort per target: one failure does not abort the rest, every outcome
  is reported, and `summarise` never claims a clean uninstall when something
  failed.

## Daemon client DTOs (`client.rs`)

The daemon publishes line-delimited JSON snapshots (1 Hz) on the control
socket; commands go the other way as single JSON lines
(`openmicro_proto::Command`).

The whole point of `SnapshotDto`'s serde defaults is **version skew
tolerance** — a snapshot from an older daemon build must still parse:

- `brightness` defaults to **200** and `sleep_minutes` to **3** — these match
  the historical hardcoded UI defaults (`ConfigUiState::new(200)`,
  `sleep_minutes: 3`) rather than zero, so an old daemon does not make the UI
  show 0% brightness.
- `colors` defaults to `openmicro_proto::AgentColors::default()`. A stale
  daemon that sends the retired *per-state* palette shape (e.g. keys `idle`,
  `working`) must fall back to defaults rather than refusing the snapshot —
  refusing would make the TUI report the device as unreachable. A test pins
  this exact old shape.
- `battery`/`charging` are optional for the same reason.

`PALETTE` is the small preset colour list the settings menu offers (white,
red, green, blue, orange, purple); `app.rs::colour_name` maps the same RGB
values back to names, so keep the two in sync when editing.

## The prompt toolkit (`prompt/`)

A clack-style terminal prompt toolkit reproducing the exact look and feel of
`npx skills add`, per the extracted spec in `docs/tui-style.md`. That spec is
the authority for nearly every magic number below — when in doubt, check it
before "fixing" anything that looks odd.

The rendering model: the transcript is **append-only in the normal buffer**
(no alternate screen). Committed blocks are printed once with a gray gutter;
only the active prompt redraws in place, with a cyan gutter (clack prompts)
or dim gutter (the search prompt). All frame construction is pure
(`state -> Vec<String>`) in the submodules — that is what makes layout and
palette testable with plain `cargo test`; `mod.rs` owns environment detection
and the ergonomic entry points.

### `mod.rs`

- `Cancelled` is returned on Esc/Ctrl-C — and also whenever stdout is not a
  terminal, so a headless run can never hang on a prompt. It implements
  `Error` so `?` folds it into `anyhow` flows.
- Capability detection mirrors `npx skills add`: unicode glyphs fall back to
  ASCII **only under `TERM=linux`**; ANSI is dropped when stdout is not a tty
  or `NO_COLOR` is set to any non-empty value (the no-color.org convention).
  Terminal width is read once at startup (default 80).
- `Item::group` is accepted for API stability but **not rendered**: the spec
  documents the group glyphs (`▾`/`▸`/`◐`) without a layout for group rows,
  so inventing one would break the "same exact theme" goal.
- `MultiOpts.detail_lines` is a fixed body-row count so the frame height
  never changes as the cursor moves; `required` makes Enter silently ignored
  while nothing is selected.
- `Ui::style()` exists so callers composing their own text honour the palette
  instead of hardcoding ANSI — that is what keeps `NO_COLOR` working.
  `Ui::symbols()` similarly lets a caller drawing its own live region (the
  agent-activity view) keep the same gutter glyphs as the prompts around it.
- The module carried a `#![allow(dead_code)]` from the period when the
  toolkit landed before the menu app consumed it; the original note said to
  remove it once `main.rs` was wired up.

### `style.rs`

Every style call goes through one `Style` value so `NO_COLOR` and non-tty
output are handled in exactly one place: when colour is disabled the helpers
return the text untouched and the rest of the renderer never thinks about it.
`Style` is copy-cheap by design so pure frame builders can take it by value.

Codes are plain SGR with the **specific reset partners** the spec lists
(`39` for foreground colours, `22` for bold/dim, `24` underline, `27`
inverse, `29` strikethrough, `49` background) rather than a blanket
`ESC[0m` — a full reset would clobber enclosing styles when helpers nest
(e.g. dim text inside a dim gutter). A test asserts every pair so a refactor
cannot silently swap, say, gray (90) for dim (2).

`BANNER_GRADIENT` is `[250, 248, 245, 243, 240, 238]` — one 256-colour per
banner row, light at the top; the exact six codes the spec extracted from
`npx skills add`.

### `symbols.rs`

`TERM=linux` is the *whole* fallback condition — keep any smarter detection
out of the glyph layer. The ASCII column follows the spec table exactly,
**including its two deliberate non-ASCII entries** (`—` for the bar end and
`•` for info), because that is what clack itself ships. The search-prompt
glyphs below the spec table have no specced ASCII forms; the stand-ins keep
layout aligned (all single-cell except `ellipsis`, whose callers pad by
content anyway). Tests assert every Unicode glyph **by codepoint** so a
lookalike (e.g. U+25C8 for U+25C6) cannot sneak in via copy-paste.

### `term.rs`

- `RawGuard` is the RAII raw-mode (and optional cursor-hide) guard
  constructed at the top of every interactive prompt. `Drop` runs on normal
  return, on `?`-propagated errors, and during panic unwinding, so the user's
  shell is never left wedged in raw mode with a hidden cursor. Restore errors
  are deliberately ignored — cleanup must never panic. `hide_cursor` is true
  for clack prompts but **false for the search prompt**: its caret is a fake
  `inverse(' ')` and the original tool never hides the cursor there.
- `FrameZone` erases the previous frame with `ESC[{n}A` + `\r` + `ESC[J` and
  writes the whole new frame **in a single `write`** — clearing rows one by
  one makes large prompts visibly flash. Lines end `\r\n` because raw mode is
  active; the cursor ends at column 0 just below the frame, exactly where the
  next erase expects it.
- `n` must be the count of **physical rows** (wrapped), not logical lines —
  a logical-line count leaves ghost rows on narrow terminals. The zone keeps
  the last frame so a `Resize` can re-count its rows under the new width.
  This is best-effort (terminals that reflow scrollback wrap the old frame
  themselves), but it keeps the prompt itself intact — more than the original
  tool manages, which ignores SIGWINCH and corrupts.
- `next_event` filters out key **releases**: they arrive on Windows and under
  the kitty keyboard protocol, and acting on both press and release
  double-steps every keystroke.
- In raw mode Ctrl-C is an ordinary key event, not SIGINT — that is what lets
  prompts return `Cancelled` instead of the process dying mid-frame.

### `render.rs`

- The five log flavours (info/success/step/warn/error) differ *only* in the
  symbol and its colour — the spec is explicit — so they share one frame
  builder. Info is blue `●`, success green `◆`, step green `◇`, warn yellow
  `▲`, error red `■`.
- An empty message line renders as the bare prefix with **no trailing
  spaces** (the spec calls this out; trailing blanks show up as selectable
  whitespace when copying from the terminal).
- Note-box geometry per spec: content is `["", …wrapped body…, ""]`,
  `n = width(title)`, box width `t = max(widest content line, n) + 2`; body
  wraps at `columns - 6`, clamped to at least 10 so a pathologically narrow
  terminal still produces a box instead of underflowing; the header dash run
  is `t - n - 1` but never below one. The top-left corner *is* the step
  symbol and the bottom-left is `├`, so the rail continues into the next
  step. (The spec's worked example: title "Note", body "hi" → the exact
  frame asserted in tests.)
- Banner rows beyond the sixth keep the darkest gradient shade — the wordmark
  is six rows, but a taller one must not be corrupted.

### `width.rs`

The redraw strategy only works if the erase count is the number of *physical*
rows the previous frame occupied: rendered width after stripping ANSI, with
CJK and emoji as double-width, divided by the column count. Under-counting
leaves ghost rows on narrow terminals; over-counting eats lines of the
transcript above the prompt. The spec's "Rendering model" section is explicit
that this must be wrapped rows, not logical lines.

- The width table is hand-rolled rather than pulling in `unicode-width`: the
  prompt only needs "is this in the East Asian Wide/Fullwidth or emoji
  blocks", and a small range table keeps the crate's dependency set flat. The
  table (condensed from Unicode `EastAsianWidth.txt`) is **deliberately
  coarse and errs towards wide**: an occasional narrow codepoint inside a
  wide range costs one blank column, while a missed wide codepoint corrupts
  the redraw.
- Zero-width ranges cover combining marks, variation selectors, zero-width
  spaces/joiners and the BOM — they attach to the previous cell.
- Control characters count as width 0: ANSI is stripped before measuring, so
  a control char never legitimately reaches the counter, and counting a stray
  one must not inflate the row estimate.
- `strip_ansi` handles CSI sequences (`ESC [ … final-byte`, the only kind
  this module emits) plus a defensive skip of any other two-byte escape.
- An empty line still occupies one physical row — that is what the printed
  `\r\n` produces on screen.
- `wrap` is greedy word wrap; words longer than the width are hard-cut
  mid-word (the spec calls this out for the detail pane). Widths are display
  cells, so CJK text wraps correctly. `wrap_clipped` clips to a line budget
  and appends the ellipsis, shrinking the last line so the ellipsis never
  exceeds the pane width — used by the fixed-height detail pane, which must
  never grow the frame as the cursor moves.

### `spinner.rs`

The spinner is the only animation in the whole interface. Two independent
motions share one tick — 80 ms in Unicode (`◒ ◐ ◓ ◑`), 120 ms in ASCII
(`• o O 0`; the `•` is the spec's own ASCII table entry, mirroring clack):
the four-frame magenta glyph, and a trailing-dot counter.

- The spec models the dots as a float counter: `+0.125` per tick, wrapping to
  0 at 4.0, displaying `min(floor(counter), 3)` dots — one more dot every
  640 ms on a 2.56 s cycle. With the wrap at exactly 4.0 that is a 32-tick
  cycle of 8 ticks per dot count, so `dots_for_tick` uses integer arithmetic
  (`(tick % 32) / 8`) and reproduces the model without float drift; a test
  re-simulates the float model literally and compares tick for tick.
- Trailing ASCII dots are stripped from incoming messages so the animation
  does not stack on literal ones — but a trailing `…` is kept, which is why
  the original shows `Parsing source….`.
- **Cleanup is the paranoid part.** The render thread hides the cursor and
  holds raw mode (so Ctrl-C arrives as a key event that can be caught and
  repainted on, instead of SIGINT killing the process mid-frame), and
  `Drop` treats a still-active spinner as cancelled — an early `?` return or
  a panic can never leave the terminal in raw mode with a hidden cursor and a
  stale frame.
- On Ctrl-C mid-spin there is no way to unwind the main thread from the
  render thread, so it takes the emergency exit: repaint the frame as
  cancelled, restore the cursor and cooked mode, and
  `std::process::exit(130)` — the conventional SIGINT status.
- The shared `rows` atomic mirrors the last frame's physical row count so the
  finishing thread (which does not own the render loop's frame state) can
  erase the live frame before painting the final line.
- The tick sleep is implemented by polling the event queue in ≤20 ms slices
  so Ctrl-C and resizes are noticed mid-tick; a resize re-counts the frame
  that was actually drawn under the new width so the next erase is exact.
- On a non-tty there is nothing to animate: the final
  `stop`/`error`/`cancel` line is the only output, keeping piped logs clean.
- `error()` is the one place the error triangle is **red** rather than
  yellow — the spec: "yellow (red when a spinner errors)".

### `select.rs` (clack select and confirm)

- Palette: cyan gutter and step glyph while the prompt is live, gray once
  committed — getting that split wrong is the most visible way to diverge
  from `npx skills add`. The active row is a green radio with an
  **underlined** (not recoloured) label and a dim parenthesised hint; the
  hint appears **only on the row the cursor is on**. Inactive rows are fully
  dim with no hint.
- `clack_window_start` implements clack's sliding window: sticky until the
  cursor nears an edge, then scrolls just enough. The magic numbers are
  clack's own — scrolling begins once `cursor >= start + max_items - 3` going
  down, and `cursor < start + 2` going up — and the spec pins them, so **do
  not "fix" the asymmetry**.
- Overflow is marked by a literal dim `...` replacing the first/last visible
  row — clack's own convention, not a count.
- Selection keys **wrap at both ends** — deliberately unlike the search
  prompt, which clamps; the spec is explicit that the two differ.
- The collapsed (submitted) frame is green `◇` with the value dim; cancelled
  is red `■` with the value strikethrough+dim, plus a trailing bare gutter —
  clack emits that extra gutter so the cancel notice that follows stays
  attached to the rail. The gutter turns gray: the prompt is now transcript.
- Confirm renders the choice inline (`● Yes / ○ No`), any arrow key toggles
  (exactly like clack), Enter submits.
- Empty option lists and non-tty stdout return `Cancelled` immediately
  rather than blocking forever on a pipe.
- The live window height keeps clack's minimum of 5 rows and otherwise fits
  the screen minus the frame's fixed chrome (4 rows).

### `multiselect.rs` (the searchable multi-select)

This is the one prompt in `npx skills add` that is *not* clack (it is
`src/prompts/search-multiselect.ts` there), with its own palette: the gutter
is **dim (not cyan) in every state**, and the active step glyph is **green
(not cyan)**. The state machine and frame builder are pure so every layout
rule the spec calls out is asserted in tests.

Layout rules that are contracts with the spec, not accidents:

- Section-header dash runs are **literal counts from the original tool,
  never width-filled**: two leading dashes, then 12 trailing on the locked
  ("always included") header and 29 on the list header. There is deliberately
  no alignment between the two.
- The locked section shows bulleted bold rows, truncated at `max_shown` with
  `…and N more`.
- The item window is **cursor-centred**:
  `start = clamp(cursor - maxVisible/2, 0, len - maxVisible)` — deliberately
  different from clack's sticky window in `select.rs`.
- Overflow in both directions shares **one** combined dim line joined by two
  spaces (`↑ 4 more  ↓ 3 more`) — never two separate lines.
- The search caret is a fake `inverse(' ')` glued to the end of the query;
  there is no left/right text cursor to move. The real terminal cursor stays
  visible (see `RawGuard` note).
- Row glyph palette: selected green; hovered-but-unselected cyan; otherwise
  dim — same glyph either way, only the colour moves. The cursor row's label
  is underlined and pointed at with a cyan `❯`.
- The fixed-height detail pane always renders exactly `detail_lines` body
  rows whether the current item has a long detail, a short one, or none, so
  the frame height never changes as the cursor moves.
- The `Selected:` summary shows up to three labels then ` +N more`. When
  nothing is selected the whole line (label included) is dim
  (`Selected: (none)`); otherwise the label is green.
- The submitted collapse is green `◇` plus one dim line joining the selection
  **in selection order**, and has **no `└` footer** — the rail runs straight
  into the next step. The cancelled collapse strikes through the word
  `Cancelled`.

Behaviour rules:

- `selected` keeps **insertion order** because the summary and the return
  value both present choices in the order the user made them.
- Filtering is a case-insensitive substring match over label *and* value —
  no fuzzy matching, no highlight, per spec. It returns indices into `items`
  so the frame builder can reach hints/details without cloning.
- Arrows **clamp** (no wraparound). Space toggles the cursor row. Any
  printable character (without Ctrl/Alt) appends to the query and resets the
  cursor to 0; backspace pops and also resets. When `searchable` is false the
  search line is omitted and typing does nothing.
- A `required` prompt swallows Enter **silently** while nothing is selected —
  the spec insists there is no error message.
- `selected_labels` falls back to the raw value for entries not present in
  `items` (e.g. seeded `initial_selected` values that a catalogue change
  later removed).
