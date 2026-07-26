# openmicrod and openmicro-hook: maintainer notes

This document preserves the design knowledge that used to live in source
comments in `crates/openmicrod/src/` and `crates/openmicro-hook/src/`. It is
organised by module. Read the "Cross-cutting contracts" section first: those
rules span several modules and breaking any of them produces silent failures,
not errors.

## Cross-cutting contracts

### Lock ordering

Everywhere in the daemon the lock order is **engine → device**. `ingress`,
`control`, `sleeper`, and the input-routing task in `main.rs` all take the
engine mutex first and the device mutex second. Any new code path that needs
both must do the same, or it can deadlock against the existing ones.

A consequence worth internalising: `DeviceLink::set_leds` runs while the caller
holds *both* mutexes. Anything a device backend does inside `set_leds` stalls
the entire daemon for its duration, which is why the BLE backend goes to such
lengths to bound its worst case (see the ble section).

### The hook event contract

Adapters (and the `openmicro-hook` binary) push events to the daemon as
newline-delimited JSON lines on a Unix socket. One line is one object:

```json
{"agent":"claude","session":"s1","state":"working"}
```

The four state names are exactly: `idle`, `thinking`, `working`,
`awaiting_approval`. These strings appear in three places that must agree: the
hook binary emits them, `ingress::parse_state` parses them, and
`control::state_name` re-emits them in TUI snapshots. Anything unparseable —
bad JSON or an unknown state — is silently ignored by ingress; there is no
error channel back to the sender.

`state = "idle"` is special: it does not mean "show idle", it means "this
session is over" — the daemon removes the session and frees its slot.

### Socket paths

The paths are defined once, in `openmicro_proto::paths`, because three separate
binaries must agree on them and a disagreement produces **no error anywhere**:
the hook is deliberately silent when it cannot connect, so a path mismatch just
means the macropad quietly never lights up. History: the daemon once fell back
to `$TMPDIR` (via `std::env::temp_dir()`) while the hook hard-coded `/tmp`,
leaving the hook writing to a socket nobody was listening on. Never duplicate
the path rule in a binary.

- Runtime directory: `$XDG_RUNTIME_DIR` when set, else the system temp dir
  (which honours `$TMPDIR`). Every caller must agree on the fallback too.
- Hook (adapter → daemon) socket: `<runtime>/openmicro.sock`.
- Control (TUI ↔ daemon) socket: `<runtime>/openmicro-ctl.sock`.

### Wire framing and firmware-facing constants

Device traffic (both directions on the cable) is framed with
`openmicro_proto::wire`; payloads are postcard-encoded protocol types. Numbers
the daemon relies on, defined in `openmicro-proto` and mirrored by the
firmware:

- `wire::MAX_PAYLOAD = 96` bytes. The firmware's RX FIFO is 64 bytes. A cable
  test pins down that an encoded `LedFrame` fits within `MAX_PAYLOAD`, because
  a frame that outgrew the limit would be **silently undeliverable** — nothing
  would error, the device would just never see a valid frame.
- `HEARTBEAT_MS = 1500`: how often the daemon re-sends the current frame.
- `DAEMON_TIMEOUT_MS = 6000`: after this long without a frame the device
  decides the daemon is gone and switches to its own "no daemon" animation.
  The proto crate statically asserts the timeout exceeds three heartbeats, so
  a couple of lost frames do not flap the device into fallback mode.

The heartbeat exists because an idle daemon and a crashed daemon would
otherwise look identical from the device's side: a daemon with nothing to
report still has to keep saying "still here".

### The hook's "always exit 0, never block" contract (load-bearing)

`openmicro-hook` is invoked by coding-agent hook mechanisms (Claude Code hooks,
Codex `notify`, etc.). If it ever blocked or returned a non-zero exit code it
could stall or fail the *agent*, which is unacceptable for a cosmetic LED
device. Therefore:

- Pushing is best-effort. If the daemon is down or the socket is missing, the
  hook silently succeeds.
- The socket write happens on a spawned thread with a 300 ms write timeout
  (`PUSH_WRITE_TIMEOUT`); the main thread waits at most 500 ms
  (`PUSH_DEADLINE`) for that thread and then exits regardless. A wedged daemon
  therefore costs an agent at most ~half a second, once.
- `main` always falls off the end and exits 0. Do not add error propagation,
  panics, retries, or logging-to-stderr that an agent might interpret as
  failure.

## openmicrod modules

### main.rs — task wiring and supervision

`main` builds the engine, opens the configured transport, and spawns every
long-running background task into a single `JoinSet`. Design decisions:

- **Config clamping at load.** `sleep_minutes` is clamped to
  `engine::MAX_SLEEP_MINUTES` when seeding the engine from config, even though
  `apply_command` clamps too: a hand-edited config file bypasses the runtime
  clamp and would otherwise seed the engine with an out-of-range value.
- **Transport fallback.** If cable or BLE connect fails, the daemon logs and
  falls back to the mock device rather than exiting. On the mock transport the
  input and battery channel senders are dropped immediately. On cable, the
  battery sender is dropped because the battery reading comes from the BLE
  Battery Service and the cable link has no equivalent yet — so battery is
  `None` on mock and cable.
- **One JoinSet, tagged tasks.** Every background task is spawned into one
  `JoinSet` returning `(&'static str, anyhow::Result<()>)` so a panic or early
  exit in *any* of them is surfaced in the logs (with the task's name) instead
  of silently vanishing.
- **Fatal vs best-effort tasks.** `ingress` and `control` are the daemon's
  core IPC surfaces; neither returns in normal operation, so any end of one of
  them (clean exit, error, or panic) means the daemon is no longer useful and
  must exit non-zero so systemd's `Restart=on-failure` kicks in.
  `battery-drain`, `input-routing`, and `sleeper` are best-effort: their end is
  logged and the daemon keeps running. A **panicked** task is always fatal,
  even a best-effort one: the closure never returned its name tag so we cannot
  tell which task died, a panicked ingress/control must still trigger a
  restart, and panics are unexpected enough that "restart to be safe" is the
  right default. If `join_next` returns `None`, every task has exited (all
  failures already logged) and the daemon shuts down.
- **Input-routing task and lock discipline.** Routing a device input event
  must respect the engine → device lock order and must not hold the engine
  lock across routing. So the task snapshots what it needs under one short
  engine lock, drops the lock, routes purely, then re-locks engine + device to
  apply the resulting action. Crucially, the slot→session map *and* the
  current focus are snapshotted under the **same** lock: reading them
  separately would let a hook event land in between and route a decision onto
  a session the user was not looking at.
- **Heartbeat task.** Ticks every `HEARTBEAT_MS` and re-renders. Tokio's
  interval fires its first tick immediately, which is desired: it announces
  the daemon as soon as it is up.
- **Activity clock.** A shared last-activity clock is touched by every
  processed hook event and every physical input, and read by the idle-sleep
  timer.
- The input channel comment trail: in phase P1 the daemon merely drained
  device input; routing input into the engine was the P2 step (now
  implemented as the input-routing task).

### engine.rs — state → LED frames

Central state holder: `SessionStore` (live sessions), `Mapping`
(session → slot), `pinned` (explicit selection), brightness, colors, `asleep`,
`sleep_minutes`.

- `MAX_SLEEP_MINUTES = 1440` (24 h). This clamp is **mirrored in the TUI's
  `adjust()`**; keep the two in sync. The clamp exists so a stuck key (or a
  bogus `Command`) cannot drive `sleep_minutes` arbitrarily high.
- `Mapping` assigns each session key a stable slot index,
  first-come-first-served, reusing freed slots. `release` on session end is
  what lets a new session take over slot 0.
- **Activity wakes.** Every entry point that represents activity — a TUI
  command, a hook event, a physical input action — clears `asleep` before
  doing its work, so the subsequent rerender shows live state. `frame()`
  returns `LedFrame::BLANK` whenever `asleep` is set.
- **Focus has one definition.** `focus_slot()` / `focused()` both derive from
  `focus::pick_owner`. The same definition is used to render the frame *and*
  to route a press, so the key that lights up and the session that gets
  approved cannot drift apart. Do not introduce a second notion of "current
  session".
- `Action::CycleFocus` orders live sessions by their stable slot index, starts
  from the pinned session if set (else the current owner), steps with
  `rem_euclid` so it wraps in both directions, and **pins** the result.
- `Action::FocusSlot` pins explicitly, so a deliberate press is not
  immediately overridden by the next session to report activity. The
  implementation resolves the slot to an owned key *before* assigning to
  `self.pinned` because `slot_lookup()` borrows `self`.
- `Action::Approve/Deny/Interrupt` are currently only logged. The decision is
  routed correctly; what is missing is a way to speak back to a running CLI,
  which needs one control channel per agent (tracked separately as the
  "adapters" work). Do not mistake the log line for a stub that was forgotten.
- `heartbeat()` just re-sends the current frame unchanged — see the heartbeat
  contract above for why that is necessary.
- `on_event` with `AgentState::Idle` removes the session and releases its
  slot, then rerenders; any other state upserts and assigns a slot.
- `to_config_fields()` returns exactly the trio (brightness, colors,
  sleep_minutes) the control plane persists to config.

### cable.rs — USB serial transport

The transport that actually works on hardware. BLE needs a GATT server the
firmware does not yet have; cable needs only the USB-Serial-JTAG console the
firmware already exposes for its logs, has no pairing to lose and no link to
drop. Both directions **share one byte stream with the firmware's log
output**, so everything is `wire`-framed and the reader skips anything that is
not a frame.

- Candidate ports are `/dev/ttyACM0` through `/dev/ttyACM3`, probed in order;
  the first that exists wins. More than one candidate matters because the
  device can move between enumerations.
- The `File` handle is shared via `Arc<Mutex<...>>` between the reader thread
  and the writer because the character device must be opened **once** — two
  independent opens would fight over it.
- **Raw mode is not optional, and the reason is subtle.** A tty in its default
  line discipline *rewrites the byte stream*: `ONLCR` turns every `0x0A` into
  CRLF, `IXON` eats `0x11`/`0x13` as flow control, and `ICANON` holds input
  until a newline. ASCII mode commands survive all of that — which is exactly
  why the first version of this transport appeared to work — but a
  postcard-encoded frame contains `0x0A` sooner or later, and every such frame
  was silently corrupted. The device simply never saw a valid frame. Never
  remove or reorder the `make_raw` call: the tty goes raw before a single
  byte moves.
- `make_raw` failing is fatal only when the path starts with `/dev/tty`. A
  plain file is not a tty, which only happens in tests; the framing is what
  those tests exercise, so the failure is tolerated for them.
- In `make_raw`, `VMIN = 0` and `VTIME = 1` (tenths of a second): reads return
  as soon as any byte is available and never block indefinitely, keeping the
  reader thread responsive to shutdown. The `unsafe` block is sound because
  the fd is live for the duration of the call and `termios` is a plain POD
  struct fully initialised by `tcgetattr` before `cfmakeraw`/`tcsetattr` read
  it.
- **Reader is a dedicated blocking thread, not an async task.** Reads from a
  character device are blocking; tokio's file I/O would push them to a
  blocking pool anyway. One dedicated thread is simpler, and its lifetime is
  the daemon's. Its loop:
  - A poisoned handle lock means the writer panicked while holding it;
    nothing useful is left to read, so the thread returns.
  - `Ok(0)` and `WouldBlock` sleep 20 ms before retrying.
  - A frame that fails to decode as an `InputEvent` is ignored, not fatal —
    the firmware may frame other message types later.
  - A closed input channel receiver means the daemon is shutting down: the
    thread stops rather than spinning.
  - Any other read error means the device went away: sleep 200 ms rather
    than spin; the daemon's heartbeat *writes* will surface the real problem.
- **Writes are deadline-bounded** (`WRITE_DEADLINE = 500 ms`).
  `write_all_before` loops on partial writes and `WouldBlock`/`Interrupted`
  with 5 ms naps, erroring out at the deadline instead of blocking forever —
  there is a test proving a stalled pipe errors at the deadline.
- `write_errors` counts consecutive failed frame writes and is public on
  purpose: a cable that has come loose looks exactly like an idle desk
  otherwise. Only the **first** failure logs (a detached cable would otherwise
  print on every heartbeat, forever); the counter resets to zero on the next
  successful write.

### ble.rs — Bluetooth transport

A real BLE `DeviceLink` backend on `bluer` (BlueZ), talking to (future)
OpenMicro firmware over the custom GATT service in `openmicro_proto::ble`. The
daemon keeps `MockDevice` as the default; this backend is used only when
config selects `Transport::Ble`. (Per config.rs: the firmware's GATT server
does not exist yet, so this cannot yet deliver a frame to real hardware.)

Timing constants and why those values:

- `DISCOVERY_TIMEOUT = 15 s`: cap on a single discovery scan.
- `backoff_delay(attempt) = min(2^attempt, 30) s` (`BACKOFF_CAP_SECS = 30`).
  Kept as a pure function precisely so it can be unit-tested without a
  Bluetooth adapter; it must not overflow for large attempt counts.
- `RECONNECT_COOLDOWN = 5 s`: minimum time between reconnect *attempts*
  triggered from `set_leds`. Bounds how often a down link can trigger a
  (bounded) discovery attempt, so a dead device can't turn every render into
  a stall.
- `RECONNECT_ATTEMPT_TIMEOUT = 2 s`: hard cap on a single in-call reconnect
  attempt from `set_leds`, independent of `discover_and_resolve`'s own 15 s
  discovery timeout. This is what bounds the worst-case time `set_leds` can
  hold its callers' locks while the link is down.

**The `set_leds` failure protocol.** `set_leds` runs while the caller holds
BOTH the engine and device mutexes, so everything in it must be tightly
bounded — a down link must not be able to stall the whole daemon. The
sequence:

1. The frame is cached in `self.last` *first*, so `last_frame()` stays
   correct no matter what happens below.
2. If the link is already known down and the cooldown has not elapsed, the
   write is skipped entirely — the (possibly stale) characteristic is not
   even touched.
3. On a write failure, the cooldown clock is restarted on **every** failure,
   not just when a discovery attempt is actually made. This is what
   guarantees at most one reconnect attempt per `RECONNECT_COOLDOWN`: the
   very next call, inside the cooldown window, hits the skip in step 2
   instead of reaching the failure branch at all.
4. The *first* failure since the link was last up only marks it down and
   returns — no discovery attempt. This keeps a brand-new failure cheap;
   recovery starts from the next call once the cooldown has passed.
5. Subsequent failures (after cooldown) make a single bounded reconnect
   attempt: `reconnect(1)` wrapped in `RECONNECT_ATTEMPT_TIMEOUT`. Combined
   with the cooldown this bounds the worst-case lock hold to ~2 s, at most
   once per 5 s while the link stays down. On success the write is retried
   once; on any failure or timeout, the frame stays cached and `connected`
   stays false so the next post-cooldown call tries again.

Two warnings attached to that path:

- It talks to real BlueZ/adapter state and **cannot be exercised without
  hardware in CI**; it has been validated on hardware only.
- It must **never fabricate success**: a failed or timed-out retry is
  reported exactly like an unrecovered failure.

Other notes:

- `reconnect(max_attempts)` is called from `set_leds` with `max_attempts = 1`
  so it can't stall the render path for the full exponential-backoff cap. A
  fuller standalone supervisor (retrying in the background, independent of
  any particular write) is future work. On success it aborts and respawns the
  input and battery notification tasks and swaps in the new LED handle.
- LED writes use write-without-response (`WriteOp::Command`) for low latency.
- Device matching: a device matches if it advertises the OpenMicro service
  UUID or its name starts with `ADV_NAME_PREFIX`.
- `bt_uuid16` expands a 16-bit SIG UUID to 128-bit form with the Bluetooth
  Base UUID (`0000xxxx-0000-1000-8000-00805f9b34fb`).
- Battery: the standard Battery Service (0x180F) / Battery Level (0x2A19)
  characteristic is **optional** — not every device exposes it. The level is
  a single byte, percentage 0..=100. Plain BLE Battery Service carries no
  charging state, so `charging` is always reported `false`. A best-effort
  initial read is done before subscribing so the UI has a value before the
  first notification.
- The caller owns the receiving halves of the input/battery channels;
  notification tasks decode and forward, and stop when the receiver is
  dropped. `Drop` for `BleDevice` aborts both tasks.

### device.rs — the DeviceLink trait and mock

`DeviceLink` is the minimal surface: `set_leds` plus `last_frame`.
`last_frame` is `#[allow(dead_code)]` on purpose: it is used by tests and by
future device backends and is deliberately kept on the trait surface.
`MockDevice` records the last frame and counts writes, which is what the
engine tests assert against.

### ingress.rs — hook event socket

Binds the hook socket (removing any stale socket file first), accepts
connections, and reads newline-delimited JSON `HookEvent`s
(`{agent, session, state}` — see the contract section for the four state
names). Per line: touch the activity clock, then lock engine → device and call
`Engine::on_event`. Unparseable lines are dropped silently. Multiple adapter
connections are served concurrently, one task per connection.

### control.rs — TUI command/snapshot socket

Binds the control socket. Each accepted connection is split into two tasks:

- **Snapshot writer**: once per second, serialises a `Snapshot` as one JSON
  line. Locks are taken briefly (battery, then engine) to build the snapshot,
  then released before writing.
- **Command reader**: reads newline-JSON `Command`s; invalid lines are
  skipped. Applies each under engine → device locks (mirroring ingress'
  order), then nudges the persist writer.

Snapshot content decisions:

- `battery` is a percentage 0..=100 when known, else `None` (e.g. on the mock
  or cable transports). `charging` is often unknown over plain BLE Battery
  Service, in which case it is `false`.
- `brightness`, `colors`, and `sleep_minutes` are carried in every snapshot
  so the TUI config panel can seed itself from the daemon's **real** config
  instead of hardcoded UI defaults. This was "Fix 1" of the final-branch
  review: opening the `[c]` config panel used to show — and then clobber the
  daemon with — the wrong values.
- `owner` is formatted `"agent:session"`. Sessions are sorted by slot.
- `state_name` emits the same four wire-format state names the hook uses.

Persistence rules:

- `persist` performs **blocking synchronous filesystem I/O** (read + write +
  rename), so it must NOT be called while any engine/device lock is held.
  Callers capture the fields to persist, drop their guards, and then invoke
  it (via the persist-writer task, which also uses `spawn_blocking`).
  Best-effort: errors are logged, never propagated.
- `persist_to` loads the current on-disk config first and only overwrites the
  three engine-owned fields, preserving `transport` (and any future fields).
  If the existing file fails to parse it **refuses to overwrite it** — a
  broken config is left for the user to inspect rather than silently
  replaced.
- `spawn_persist_writer` coalesces bursts: it drains all pending nudges
  (`try_recv` loop) before reading the engine once and writing once, so a
  rapid brightness sweep does not write the file dozens of times.

### config.rs — persisted settings

- `Transport` variants and the reasoning: `Mock` renders into memory (useful
  for testing the daemon alone); `Cable` runs over the firmware's
  USB-Serial-JTAG console; `Ble` requires a firmware GATT server **which does
  not exist yet**. `Cable` is the default because it is the only transport
  that currently reaches real hardware, needs no pairing, and cannot drop out
  mid-session. (The tests pin the default explicitly.)
- Config path: `~/.config/openmicro/config.toml` (falling back to `.` if
  `HOME` is unset).
- **Atomic save**: serialise to TOML, write to a temp file in the same
  directory, `sync_all`, then rename over the target — rename is atomic on
  the same filesystem. The temp file name embeds the PID and a process-wide
  atomic counter so concurrent saves never collide; a failed save removes its
  temp file. There are tests asserting concurrent saves leave a parseable
  file and no temp files are left behind.
- Missing config file → defaults (not an error). Unparseable config file → an
  error from `load_existing*`; the daemon's `load()` logs it and falls back
  to defaults, while the control plane's persist path refuses to overwrite
  (see control.rs).
- Backward compatibility: before colour meant "which agent", the `[colors]`
  table held per-state keys (`idle`, `working`, ...). A config from such an
  older build must still load — the user's brightness must not be lost
  because of a stale colour table. The unknown keys are simply ignored and
  colors fall back to defaults.
- Default `sleep_minutes` is 3; default brightness 200.

### render.rs — composing the frame

The visual decisions (what colour/effect/motion each state gets) all live in
`openmicro_effects::status` — this module only decides *which* session each
part of the board is talking about. That split matters: the firmware runs the
same `status` code when the daemon is not there to ask, so there is exactly
one definition of what, say, amber means. Do not encode visual policy here.

`render_frame` does three independent things, independent on purpose:

- Each occupied agent key gets its agent's colour and its state's effect.
  (The board tells you *who*, not just that something is busy — two agents in
  the same state still differ by colour.)
- The ring shows the focused session's state. When focus is empty it falls
  back to a quiet "no agents" breath rather than going dark: a
  working-but-idle device must be distinguishable from a dead one.
- The bottom action row lights **only** if the focused session is actually
  awaiting a decision. A waiting-but-unfocused session does not arm the row
  (the press would be ambiguous about which session it decides) — but its own
  key still pulses, the only per-key animation, so a waiting session is
  findable among busy neighbours.

The transparent-keycap status light mirrors the focused session (colour of the
agent, pulsing when waiting), so the one key readable from across the room
says who is waiting. Out-of-range or empty-slot focus values fall back
gracefully to the idle glow; they must never panic.

### action.rs — key → action routing

Routing follows the layout's roles (`openmicro_proto::layout`), so there is no
second opinion about which key is "Deny": agent keys (top two rows) select
which session the ring and action row are talking about; the bottom row
decides the selected session's pending request; the encoder adjusts brightness
(`BRIGHTNESS_STEP = 8` per detent, clamped later by the engine); the joystick
cycles the selection (`dir >= 4` maps to −1, otherwise +1).

Rules a future editor must not break:

- **The bottom row only resolves while the selected session is actually
  waiting on a decision** — which is also the only time those keys are lit.
  A press on a dark key doing nothing is the intended behaviour, not a gap:
  it keeps the lights and the semantics honest about each other. (Same
  principle as render.rs arming the row only for the focused session.)
- **Key releases carry no meaning.** Acting on both edges would double every
  decision.
- An **empty agent key is not selectable**: selecting an empty slot would
  silently blank the ring.
- **Interrupt** only routes while the focused session is `Thinking` or
  `Working` — again, the only time that key is lit.
- The **status key is an indicator, not a button**; it never acts.
- Unknown key ids must do nothing: the encoder press arrives as a reserved
  high id, and a wedged firmware could send anything at all. Neither may ever
  resolve to a decision. (A test sweeps ids like 13, 40, 0xFE, 0xFF.)
- Approve and Deny must never map to the same action — a mix-up here approves
  what the user rejected; a test pins this down.

### focus.rs — who owns the deck

`pick_owner` chooses which session owns the deck. Rules, in order:

1. If any session is `AwaitingApproval`, the most-recently-updated such
   session wins (preemption — a question outranks everything).
2. Else, if `pinned` names a live session, it wins.
3. Else, the most-recently-updated session wins.

### session.rs — session store

`SessionKey::kind()` derives the `AgentKind` from the agent name the hook
reported rather than storing it, so kind can never drift out of step with
`agent`. `updated_ms` is Unix-epoch milliseconds, refreshed on every update;
it is what focus recency is based on. `SessionStore::get` is
`#[allow(dead_code)]`: used by tests, deliberately part of the store's read
API.

### sleeper.rs — idle-sleep timer

Tracks last activity and, after enough idle minutes, tells the engine to blank
the LEDs.

- `TICK = 15 s`: how often the loop wakes to check idle time.
- `ActivityClock` is a `Clone`-able shared `Instant`; it is cloned into every
  input path, and every processed hook event or physical input calls
  `touch()`.
- `should_sleep` is deliberately pure so it is trivially testable: false when
  sleeping is disabled (`sleep_minutes == 0`) or the engine is already
  asleep; otherwise true once idle reaches the threshold.
- The serve loop reads `(sleep_minutes, asleep)` under a short engine lock,
  drops it, then — only when sleeping is due — re-locks engine before device
  (the global order) to perform the sleep.

Waking is not the sleeper's job: every activity path in the engine clears
`asleep` inline. `Engine::wake` exists as the explicit-wake entry point.

## openmicro-hook

The tiny binary every coding-agent adapter invokes. Its overriding contract —
always exit 0, never block the agent — is described in the cross-cutting
section; everything below operates under it.

Subcommands:

- `push --agent A --session S --state ST`: pushes a raw
  `{agent, session, state}` event. This is the **universal adapter
  contract** — any future agent integration can shell out to this.
- `claude-hook --state ST [--agent A]`: reads a Claude Code (or
  Claude-compatible) hook JSON object from stdin, extracts `session_id`, and
  pushes a state event. `--agent` exists so Claude-*compatible* agents (e.g.
  Grok Code) can reuse the same stdin mechanism; it defaults to `claude`.
  When stdin is empty, not JSON, or lacks a non-empty `session_id`, the
  session falls back to `"default"`.
- `codex-notify [PAYLOAD]`: takes the Codex CLI `notify` JSON from the first
  positional argument (Codex passes it as a trailing arg), else stdin, maps
  its event `type` to a state, and pushes a `codex` event.

Codex mapping knowledge:

- Codex's only *confirmed* notify event is `agent-turn-complete` (fired when
  the turn finishes and the agent waits for input) → `idle`.
- Heuristics for the rest, matched case-insensitively on substrings of
  `type`: anything containing `approval`, `request`, or `notification` →
  `awaiting_approval`; anything containing `complete`, `finish`, `done`,
  `idle`, or `stop` → `idle`; everything else — a known in-progress/start
  event, an unrecognised type, a missing type, or garbage input → `working`.
- Codex currently exposes **no stable session id** — the payload carries only
  a per-turn `turn-id` — so the session is `"default"` unless a
  `session_id`/`session-id` field appears in a future payload (both spellings
  are checked).

Push mechanics: build one newline-terminated JSON line (the ingress contract),
connect to the hook socket, write with a 300 ms write timeout on a spawned
thread, and give the whole operation a 500 ms deadline from the main thread.
Every failure mode — no daemon, connect refused, timeout — is deliberately
silent.
