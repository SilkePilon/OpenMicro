# openmicro-proto — protocol, layout and shared contracts

`openmicro-proto` is the shared vocabulary between the firmware, the daemon, the
hooks and the TUI. It is `no_std` (via `#![cfg_attr(not(feature = "std"), no_std)]`)
with `alloc`, because the same types must compile for `xtensa-esp32s3-none-elf`
and for the host. The crate's own tests run against the no_std build, so `std`
is pulled in explicitly under `#[cfg(test)]` — the framing tests need `Vec` to
collect frames. The `paths` module is gated behind the `std` feature because it
touches the filesystem and environment; everything else is shared by both ends.

**The general rule for this crate: anything both ends must agree on lives here,
once.** Two implementations of any of these tables or constants are two chances
for the key that lights up and the key that acts to disagree.

## types.rs — the shared model

### AgentKind and brand colours

`AgentKind` (Claude, Codex, Grok, Opencode, Other) identifies *who* occupies a
slot. State alone answers "is something busy?"; the colour answers "who?", which
is the question that matters when three sessions are running at once.

- `Other` is deliberately not folded into one of the named agents: a mystery
  agent should look like a mystery, not impersonate Claude. `from_name` matches
  the names the hooks report (`claude`, `codex`, `grok`, `opencode`),
  case-insensitively, and anything unknown — including the empty string — maps
  to `Other` rather than defaulting to Claude.

- The brand palette:

  | Agent    | RGB               | Reads as |
  |----------|-------------------|----------|
  | Claude   | `255, 120, 30`    | orange   |
  | Codex    | `230, 230, 230`   | white    |
  | Grok     | `160, 60, 255`    | purple   |
  | Opencode | `0, 200, 190`     | teal     |
  | Other    | `120, 120, 120`   | grey     |

  These were chosen to be unmistakable from each other across a dim room at a
  glance, which matters more than matching a brand hex exactly — they are 5 mm
  diffused emitters seen through a translucent keycap, not a screen. Codex's
  white is held slightly below full scale (`230` rather than `255`) because a
  saturated WS2812 white next to a coloured neighbour swamps it.

- **Separation requirement:** telling sessions apart at a glance is the whole
  point of the palette, so a test asserts that every pair of brand colours has a
  Manhattan RGB distance (sum of per-channel absolute differences) greater
  than 120 — real separation, not mere inequality. Any future recolouring must
  keep that spread.

### Motion — the ring's vocabulary

The underglow ring is the device's status line, and the `Motion` variants are
its words. Each one has a deliberately different *shape* — swell, travel,
flash — not just a different speed, because speed alone is hard to read at a
glance when two motions are only ever on screen at different times (there is
nothing to compare a speed against).

| Motion      | Shape                        | Means                          |
|-------------|------------------------------|--------------------------------|
| `Off`       | dark                         | —                              |
| `Breath`    | whole ring swells together   | calm; nothing is happening     |
| `Spin`      | a comet travels round        | something is running           |
| `Alert`     | sharp double-blink           | a decision is waiting on you   |
| `Aurora`    | slow multi-hue drift         | no host at all — device alone  |
| `Searching` | one dim dot orbiting slowly  | host up, daemon down           |

### Glow — intent, not pixels

`Glow` (colour, motion, brightness, speed) is sent as *intent*, not as pixel
data. A `Spin` streamed at 60 fps would be roughly 1.4 kB/s of BLE writes just
to say one word, and it would stutter on every dropped frame; worse, it could
not run at all when the daemon is the thing that has gone away. So the firmware
owns the animation and the host owns the meaning.

`Glow::speed` is a rate where `NOMINAL_SPEED = 128` is the motion's nominal
speed and 255 is roughly twice it. This is what lets Thinking and Working share
`Spin` and still be distinguishable.

### Heartbeat and timeout — a cross-component timing contract

- `HEARTBEAT_MS = 1500`: the daemon resends the current frame this often even
  when nothing changed. Without it, silence is ambiguous: a daemon with nothing
  new to say looks exactly like one that has died, and the device cannot know
  whether to keep showing the last frame or fall back to its own "no daemon"
  animation.
- `DAEMON_TIMEOUT_MS = 6000`: how long the device waits before deciding the
  daemon is gone. Comfortably more than three heartbeats, so a couple of
  dropped BLE writes do not make the display flicker between live and
  disconnected.
- A compile-time assertion enforces `DAEMON_TIMEOUT_MS > HEARTBEAT_MS * 3`
  ("the timeout must tolerate several missed heartbeats"). Do not tighten the
  timeout or slow the heartbeat without keeping that ratio.

### ActionKeys and the action colours

`ActionKeys` says which of the bottom-row keys are live. Keys light *only* when
a press would do something — an always-lit button that usually does nothing
teaches you to ignore it, which is the opposite of what an approval prompt
needs.

- `interrupt` is a separate flag rather than part of the decision pair because
  it answers a different question: it is offered whenever there is work to
  stop, not when a decision is pending.
- `any()` is deliberately `approve || deny` only — it answers "is a decision
  being offered / is the bottom row visible?". It must **not** include
  `interrupt`: that key is about ongoing work, not a pending question, and
  callers asking "is something waiting on me?" would get the wrong answer if it
  counted.

Colour constants shared by both ends:

- `APPROVE_COLOR = 0, 230, 70` — green for yes.
- `DENY_COLOR = 255, 30, 20` — red for no.
- `INTERRUPT_COLOR = 180, 25, 15` — dim red for stop. It shares a hue with
  `DENY_COLOR` because both are the negative answer, and sits at a lower
  brightness because it is a standing option rather than a reply to a question.
- A test pins the readability requirement: approve must be dominantly green
  (`g > r + 100`) and deny dominantly red (`r > g + 100`) — a "deny" key that
  leans green is actively dangerous.

### AgentColors — the user-tunable palette

`AgentColors` is the per-agent key colour table, overridable by the user, and
defaults to the brand palette. It replaced an older per-*state* palette. The
design rule: colour answers "which agent", the effect answers "doing what" —
one channel per question, so both stay readable when six sessions are lit at
once. Colouring by state instead meant three simultaneous Claude sessions were
indistinguishable, which is exactly the case that needs telling apart.

`#[serde(default)]` on the struct is deliberate migration armour: a config file
written by an older build has `idle`/`thinking`/… keys under this table, and
defaulting each field lets the rest of that file survive being read (the old
shape parses to the defaults) instead of taking the whole config — including
the user's brightness — down with it. A test
(`an_older_configs_color_table_does_not_break_parsing`) pins this.

### Commands, frames and events

- `Command` (SetBrightness / SetAgentColor / SetSleepMinutes) is what the TUI
  sends to the daemon over the control socket, as JSON.
- `LedFrame` is the whole display in one struct: `slots` (one `LedSlot` per
  agent key, top two rows), `glow` (the ring), `actions` (which action keys are
  offered), `status`, and `brightness`.
  - `status` is the transparent-keycap status light: the focused agent's colour
    and state. It is carried separately from `slots` because it is not an agent
    slot — it mirrors whichever slot has the focus.
  - `brightness` (master brightness) is carried explicitly because the action
    keys are sent as booleans, not pixels — the device knows their colours but
    not how bright the user wants them, and reading brightness back off an
    agent slot would fail whenever no session happens to be lit.
- `LedFrame` and `InputEvent` are encoded with **postcard**
  (`postcard::to_allocvec` / `from_bytes`); `InputEvent` covers key
  press/release, encoder delta and joystick direction. `Battery` carries a
  percentage and charging flag.

`SLOT_COUNT = 6`: one entry per agent key. See layout.rs for where six comes
from.

## wire.rs — framing for the cable link

Over USB-Serial-JTAG the LED/input protocol shares one byte stream with the
firmware's own log output — `esp_println` writes to the same TX FIFO. So a
reader has to be able to find frames inside a stream that also contains
arbitrary human-readable text, and must never mistake a log line for a frame.

```text
0xF5  len  payload[len]  sum
```

- **`FRAME_START = 0xF5`** is the start marker. It is not a legal byte anywhere
  in UTF-8 — not as a lead byte (lead bytes stop at `0xF4`) and not as a
  continuation byte (those stop at `0xBF`) — so it cannot appear in log output
  at all, which is what makes resynchronisation possible. An earlier attempt
  used `0x7E`, which is `~` — plainly ASCII, and so plainly able to appear in a
  log line. A test (`the_marker_cannot_occur_in_utf8_text`) pins that the
  marker stays an illegal UTF-8 byte; the entire resynchronisation strategy
  rests on it.
- `len` is the payload length, capped at **`MAX_PAYLOAD = 96`**. A
  postcard-encoded `LedFrame` is around 50 bytes; 96 leaves room to grow
  without allowing a bogus length byte to make a reader wait for 255 bytes that
  will never arrive.
- `sum` is the sum of the payload bytes, mod 256.
- A frame occupies `payload + 3` bytes on the wire (`framed_len`).

**Why a checksum at all:** it is not for corruption — USB already guards
against that. It exists for **resynchronisation**: after a frame is truncated
(a device reset mid-write), the half-frame keeps consuming bytes and will
misread the *next* frame's header, and the checksum is what makes that misread
get discarded instead of rendered as a garbage display. The cost of a
truncation is therefore about one extra frame — under two seconds, given the
heartbeat. A wedged reader would be forever; ~3 s of stale display is
acceptable, and a test pins that the reader recovers within roughly one extra
frame.

**Why length-prefixing rather than escaping:** payload bytes are never special,
so a payload containing the marker byte survives untouched (postcard output
will contain any given byte sooner or later). There is no escaping scheme to
get wrong on either end.

`encode` returns `None` — never a partial frame — if the payload is too long
or the output buffer too small, because a reader could not distinguish a
partial frame from a truncated one.

### Reader

`Reader` is an incremental, byte-at-a-time state machine
(Hunting → Length → Payload → Sum), fed one byte at a time because that is how
both ends receive: the firmware polls a FIFO and the host reads whatever a
`read` happened to return. Holding the partial frame inside the reader means
neither side needs its own buffering.

Behavioural details that matter:

- In the Length state, a length of zero, or one beyond `MAX_PAYLOAD`, means the
  marker was not really a frame start; the reader goes back to hunting rather
  than waiting for bytes that will never come. If the bogus "length" byte is
  itself the marker, it is treated as a fresh frame start (`0xF5 0xF5 len …` —
  the second marker is the real one).
- `Feed::Bad` (checksum mismatch) is surfaced rather than swallowed so it can
  be logged — a stream of `Bad`s means the framing is out of step, not that
  the display is idle.
- `in_frame()` is true while part-way through a frame. It lets a caller that
  multiplexes framed data with plain single-byte text commands on one stream
  tell the two apart: a byte fed while `in_frame()` is true belongs to the
  frame and must not also be interpreted as a command. (Without this, a
  frame's payload bytes get handed to the command parser as well and it
  complains about each one.)

## layout.rs — the physical key layout

This is shared vocabulary rather than firmware-private detail: the daemon needs
it to decide which keys to light and to route a press onto an action, and the
firmware needs it to decide which LED to paint. Putting it in one place is what
stops the two ends disagreeing about which key is "Deny".

### Where the numbers come from

Not guessed. Work Louder's shipping firmware embeds its own default
`keymap.json`, and one of the profiles in it is an **agent** profile — the
vendor's own take on exactly this use case:

```text
"keymap": [
  ["KV_OAI_AG00",  "KV_OAI_AG01"],
  ["KV_OAI_AG02",  "KV_OAI_AG03",  "KV_OAI_AG04",  "KV_OAI_AG05"],
  ["KV_OAI_ACT06", "KV_OAI_ACT07", "KV_OAI_ACT08", "KV_OAI_ACT09"],
  ["KV_OAI_ACT10", "KV_OAI_ACT11", "KV_OAI_ACT12"]
]
```

So the board is 13 keys in rows of **2, 4, 4, 3**, numbered row-major from the
top left, and the vendor splits them into six `AG` (agent) keys and seven `ACT`
(action) keys. That six is why `SLOT_COUNT` is six.

### The physical picture

```text
        ┌────┐┌────┐
        │  0 ││  1 │            row 0 — agent slots 0..1
        └────┘└────┘
  ┌────┐┌────┐┌────┐┌────┐
  │  2 ││  3 ││  4 ││  5 │      row 1 — agent slots 2..5
  └────┘└────┘└────┘└────┘
  ┌────┐┌────┐┌────┐┌────┐
  │  6 ││  7 ││  8 ││  9 │      row 2 — reserved, dark
  └────┘└────┘└────┘└────┘
  ┌────┐┌────┐┌────┐
  │ 10 ││ 11 ││ 12 │            row 3 — Approve / Deny / Status
  │ ✓  ││ ✗  ││ ◉  │            (12 = transparent keycap, agent colour)
  └────┘└────┘└────┘
```

Row 3 is the row opposite the dial. Its rightmost key carries the transparent
keycap and shows the focused agent's colour; approve and deny sit immediately
to its left.

### Key assignments and the reasoning behind them

- `KEY_COUNT = 13`, `ROWS = [2, 4, 4, 3]` (sums to `KEY_COUNT`),
  `UNDERGLOW_COUNT = 8` (the ring), `AGENT_KEYS = [0..=5]`.
- `INTERRUPT_KEY = 6` — stop whatever the selected session is doing. It lives
  on its own row (row 2's leftmost) rather than in the bottom three because it
  answers a different question: the bottom row responds to a request the agent
  made, while this one interrupts work nobody asked about. Mixing it into the
  decision row would make "deny" and "stop" neighbours, which is one wrong
  press away from being annoying.
- `RESERVED_KEYS = [7, 8, 9]` — no assigned meaning yet, and held dark on
  purpose: an unlit key reads as "nothing here", where a lit one invites a
  press that does nothing.
- Bottom row, left to right: `APPROVE_KEY = 10`, `DENY_KEY = 11`,
  `STATUS_KEY = 12`. Approve is ordered before deny left-to-right — rather
  than the reverse — so the safe answer is the one you reach first.
- `STATUS_KEY` is the key with the **transparent keycap**, which passes far
  more light than the tinted ones — the one key on the board that reads
  clearly from across a room. It shows the focused agent's own colour, and it
  is an indicator, not a button: pressing it does nothing. Approve and deny
  sitting immediately to its left is what makes them findable without looking:
  you find the lit status key, and the decision keys are next to it.
- A test (`the_bottom_row_is_the_three_action_keys`) pins the user-facing
  promise that the bottom row, left to right, is approve / deny / status; if a
  renumbering breaks that, the labels on a physical device silently become
  wrong.

### LED_FOR_KEY — the chain order (the one measured number)

`LED_FOR_KEY` maps a key id to its LED's position along the WS2812 chain, and
**this is the one table in the module that did not come out of the vendor's
keymap**. The order the chain is physically routed in is a PCB fact stated
nowhere in the vendor image (the strip is written as a flat array).

**The chain runs backwards relative to the key numbering: chain position `i`
is key `KEY_COUNT - 1 - i`.** This was *measured on the device, not assumed*:
with an identity map, the keys the firmware believed were the bottom row lit
up along the top, and a probe holding chain index 0 lit the bottom-right key —
the transparent-keycap key, which is the *last* key in row-major order.

The first attempt was the identity map, on the reasoning that a board numbered
row-major is probably wired row-major. It is not, and nothing in the vendor
image says so either way; only looking at the hardware settled it. **To
re-check after any board revision:** build with `OPENMICRO_IDENTIFY=1`, which
lights one chain position at a time and logs its index.

Compile-time assertions guard the tables: the rows must account for every key;
every LED index must be on the chain and used exactly once (a duplicated or
out-of-range LED index would leave one key permanently dark and another
double-driven, which is maddening to debug by eye); and `AGENT_KEYS` must have
exactly `SLOT_COUNT` entries.

### Roles

- `KeyRole` classifies every key (Agent(n) / Interrupt / Reserved / Approve /
  Deny / Status). `role_of` is total over `0..KEY_COUNT` by construction, and a
  test keeps it that way if the rows are ever renumbered; off-board ids return
  `None`.
- `ActionRole` (Approve / Deny) is split out from `KeyRole` so code that only
  ever deals with the action row does not have to handle `Agent`/`Reserved`
  cases that cannot occur there. It knows its own key id and backlight colour.
- `row_col` gives the (row, column) of a key for laying out a preview in the
  TUI.

## ble.rs — GATT identifiers

Both host (bluer) and firmware (trouble) build their UUID types from these
`u128` values; the canonical string forms are recorded here. **If either end
drifts from these, discovery and the characteristic lookups silently fail.**

| Constant                 | Value                                             | Direction / purpose            |
|--------------------------|---------------------------------------------------|--------------------------------|
| `OPENMICRO_SERVICE_UUID` | `9e7a0001-0000-4000-8000-0026bb765291`            | custom service                 |
| `LED_CHAR_UUID`          | `9e7a0002-0000-4000-8000-0026bb765291`            | LED frame write (host→device)  |
| `INPUT_CHAR_UUID`        | `9e7a0003-0000-4000-8000-0026bb765291`            | input event notify (device→host) |
| `BATTERY_SERVICE_UUID`   | `0x180F` (standard Battery Service)               | battery                        |
| `BATTERY_LEVEL_UUID`     | `0x2A19` (standard Battery Level characteristic)  | battery level                  |
| `ADV_NAME_PREFIX`        | `"OpenMicro"`                                     | advertised-name discovery fallback |

A test asserts the three 128-bit UUIDs are pairwise distinct.

## paths.rs — socket locations (std-only)

Three separate binaries (daemon, hook, TUI) have to agree on these paths, and
when they disagree **nothing reports an error**: the hook is deliberately
silent when it cannot connect, so a mismatch just means the macropad quietly
never lights up. The rule lives here so there is only one of it.

- `runtime_dir()`: `$XDG_RUNTIME_DIR` when set, otherwise the **system temp
  directory** (`std::env::temp_dir()`, which honours `$TMPDIR`). Every caller
  must agree on the fallback too — this is a lesson learned: a daemon that fell
  back to `$TMPDIR` while the hook hard-coded `/tmp` left the hook writing to a
  socket nobody was listening on. Do not hard-code `/tmp` anywhere.
- `hook_socket()`: `<runtime_dir>/openmicro.sock` — adapters push agent-state
  events here.
- `control_socket()`: `<runtime_dir>/openmicro-ctl.sock` — the TUI's command
  and snapshot channel.
