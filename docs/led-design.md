# What the lights mean

The board has three display surfaces and they answer three different questions.
Keeping them separate is the whole design: when six things are lit at once, a
surface that tries to say two things says neither.

| Surface | Question it answers | Vocabulary |
|---|---|---|
| Agent keys (rows 0–1) | **Who** is busy, and with what? | colour = agent, effect = state |
| Action keys (rows 2–3) | **What can I do right now?** | lit only when a press would act |
| Underglow ring | **Does this need me?** | motion shape |

## The layout

Thirteen keys in rows of **2, 4, 4, 3**.

```
      ┌────┐┌────┐
      │  0 ││  1 │              agent slots 0–1
      └────┘└────┘
┌────┐┌────┐┌────┐┌────┐
│  2 ││  3 ││  4 ││  5 │        agent slots 2–5
└────┘└────┘└────┘└────┘
┌────┐┌────┐┌────┐┌────┐
│STOP││    ││    ││    │        6 = interrupt, 7–9 reserved
└────┘└────┘└────┘└────┘
┌────┐┌────┐┌────┐
│ ✓  ││ ★  ││ ✗  │             10 approve · 11 always · 12 deny
└────┘└────┘└────┘
 ◌ ◌ ◌ ◌ ◌ ◌ ◌ ◌               8-LED underglow ring
```

This is not a guess. Work Louder's shipping firmware embeds its own default
`keymap.json`, and one of the profiles in it is an **agent** profile — the
vendor's own take on this exact use case:

```json
"keymap": [
  ["KV_OAI_AG00",  "KV_OAI_AG01"],
  ["KV_OAI_AG02",  "KV_OAI_AG03",  "KV_OAI_AG04",  "KV_OAI_AG05"],
  ["KV_OAI_ACT06", "KV_OAI_ACT07", "KV_OAI_ACT08", "KV_OAI_ACT09"],
  ["KV_OAI_ACT10", "KV_OAI_ACT11", "KV_OAI_ACT12"]
]
```

Six `AG` keys and seven `ACT` keys, numbered row-major. That six is why
`SLOT_COUNT` is six. Their lighting code splits the same way we do, into
`syncKeysLighting` and `syncAmbientLighting`, and their effect vocabulary
(`solid`, `snake`, `rainbow`, `gradient`, `shallow_breath`) maps almost directly
onto the motions below — `snake` is `Spin`, `shallow_breath` is `Breath`.

Everything above is recovered fact. The one thing that is **not** is
`layout::LED_FOR_KEY`, the map from key id to position along the WS2812 chain:
nothing in the vendor image states the physical routing order, so it defaults to
identity. If the lights land on the wrong keys, that one table is the only thing
to change.

To establish it, build with `OPENMICRO_IDENTIFY=1 cargo build --release`: the
firmware then lights one chain position at a time and logs its index to serial,
so the physical order can be read off the device and written into the table.

## Agent colours

Colour says **who**, not what. Three simultaneous Claude sessions are the case
that actually needs telling apart, and colouring by state made them identical.

| Agent | Colour | RGB |
|---|---|---|
| Claude | orange | `255, 120, 30` |
| Codex | white | `230, 230, 230` |
| Grok | purple | `160, 60, 255` |
| anything else | grey | `120, 120, 120` |

White sits below full scale because a saturated WS2812 white swamps a coloured
neighbour. An unrecognised agent stays grey rather than being folded into
Claude — a mystery agent should look like one.

All four are user-overridable (`AgentColors`, TUI → Settings → *Colour for an
agent*), and a test asserts they stay far enough apart in RGB to be
distinguishable.

## Agent state, on the key

Only one state animates. Six competing per-key animations are unreadable; one is
findable.

| State | Effect | Brightness |
|---|---|---|
| Idle | Breath | 45% |
| Thinking | Breath | 80% |
| Working | Solid | 100% |
| Awaiting approval | **Pulse** | 100% |

## The ring

The ring shows the **focused** session — the one the action keys act on. Motions
differ by *shape*, not speed, because two of them are never on screen together,
so there is nothing to compare a speed against.

| Situation | Motion | Colour |
|---|---|---|
| No host at all | `Aurora` — slow cyan→blue drift | its own hues |
| Host up, daemon down | `Searching` — one dim dot orbiting | amber |
| Daemon up, nothing running | `Breath`, very dim | pale white |
| Agent idle | `Breath`, slow | agent's |
| Agent thinking | `Spin` | agent's |
| Agent working | `Spin`, faster | agent's |
| Agent awaiting approval | `Alert` — sharp double-blink | agent's |

Two details worth stating:

- **Neither disconnected state is dark.** Dark is indistinguishable from broken,
  flat, or unplugged. Offline and daemon-down differ in *both* colour and
  motion, so neither channel alone has to carry it.
- **Aurora ignores the agent colour**, because in that state there is no agent.
  Its hue band stops at 168 rather than running further into the blues: the wheel
  turns violet at 172, and violet reads warm again, which would undo the point of
  a cool ambient state.

## The action keys

They light **only** when a press would do something. A lit button that usually
does nothing teaches you to ignore it, which is precisely wrong for an approval
prompt. Their appearing *is* the notification.

| Key | Colour | Lit when |
|---|---|---|
| 10 Approve | green `0,230,70` | focused session awaits approval |
| 11 Always | amber `255,170,0` | same |
| 12 Deny | red `255,30,20` | same |
| 6 Stop | dim red `180,25,15` | focused session is thinking or working |

Approve/always/deny are held **solid**, not pulsing: these are targets to hit,
and the ring's `Alert` is already carrying the urgency. Two things shouting at
once is worse than one.

Stop sits on its own row, and never lights at the same time as Deny — they share
a hue, so overlap would be genuinely confusing. It is dimmer because it is a
standing option rather than a reply to a question.

## Where the code lives

The visual decisions are all in **one** place, `openmicro-effects`, which both
the daemon and the firmware depend on:

- `status.rs` — every mapping above. This module *is* the design.
- `ring.rs` — the ring motions, pure integer maths.
- `demo.rs` — a scripted walk through all of it.

That matters because of one asymmetry: **the two disconnected states exist
exactly when the host cannot tell the device anything.** If the device only ever
displayed what it was told, "no daemon" and "no host" would both render as dark.
So the firmware carries the same design and falls back to it locally.

## Why the wire carries meaning, not pixels

`LedFrame` sends *state* — a `Glow { colour, motion, speed }` and three booleans
— and the firmware animates. Sending pixels instead would mean roughly 1.4 kB/s
of BLE writes to say one word, it would stutter on every dropped frame, and it
could not work at all in the case where the daemon is the thing that has gone
away.

The consequence is that silence has to mean something, so the daemon resends the
current frame every `HEARTBEAT_MS` (1.5 s) and the device falls back after
`DAEMON_TIMEOUT_MS` (6 s) — comfortably more than three heartbeats, so a couple
of dropped writes do not make the display flicker between modes.

## Status

Implemented and unit-tested: all of the above.

Verified on hardware: the boot sweep, `Offline`, `NoDaemon`, and — via
`OPENMICRO_DEMO=1` — every host-driven state.

**Not reachable on hardware yet:** the real host-driven path, because
`firmware/src/ble.rs` is still a sketch, so `LED_FRAME_CHANNEL` has nothing
feeding it. Wiring the GATT server is the remaining piece; when it lands, the
demo scenes become live states with no rendering changes.
