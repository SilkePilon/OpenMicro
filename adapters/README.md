# OpenMicro agent adapters

An adapter connects a coding agent's lifecycle events to OpenMicro so your
macropad reflects what the agent is doing. Every adapter ultimately does one
thing: call `openmicro-hook` on state transitions.

## Installing them

You do not have to follow these documents by hand. Run `openmicro`, pick
**Coding agents**, and tick the ones you want; the guided setup does the same
thing as its last step.

Installing is merge-only and idempotent: your existing keys and hooks survive
(key order included), the previous file is copied to `<name>.openmicro.bak`, the
replacement is written atomically, and re-running changes nothing. A config that
cannot be merged safely — invalid JSON, or a Codex `notify` already pointing at
another program — is reported and left untouched.

The per-agent `install.md` files below stay the reference for what is written
and why.

## The universal contract

Any integration just needs to run, on a lifecycle transition:

```sh
openmicro-hook push --agent <name> --session <id> --state <state>
```

- `--agent` — a short stable name for the agent (`claude`, `codex`, `grok`, …).
  It groups a tool's sessions on the macropad.
- `--session` — a stable id for the conversation/session. Reuse the same id for
  the life of a session so its macropad slot stays put. If the agent gives you
  no id, use any constant (e.g. `default`).
- `--state` — one of the four states below.

The daemon accepts exactly these states:

| State               | Meaning                                             |
| ------------------- | --------------------------------------------------- |
| `idle`              | Not working; turn finished, waiting for you.        |
| `thinking`          | Prompt received; reasoning before acting.           |
| `working`           | Actively running tools / editing / executing.       |
| `awaiting_approval` | Blocked on you (approval, notification, question).  |

### Best-effort, exit-0 guarantee

`openmicro-hook` **always exits 0**, even if `openmicrod` is not running (it
silently no-ops when the socket is absent). This is deliberate: an adapter hook
must never block or fail the agent. Wire adapters freely — a stopped daemon
costs nothing.

### Ready-made subcommands

Some agents deliver their event as JSON rather than as ready CLI args, so
`openmicro-hook` ships helpers that parse the payload for you:

- `openmicro-hook claude-hook --state <state> [--agent <name>]` — reads a Claude
  Code (or Claude-compatible) hook JSON from **stdin**, extracts `session_id`,
  and pushes it. `--agent` defaults to `claude`; set it to reuse the same stdin
  mechanism for any agent whose hooks mirror Claude Code's (e.g. Grok Code).
- `openmicro-hook codex-notify [<payload-json>]` — reads a Codex CLI `notify`
  payload (first positional arg, else stdin), maps its event `type` to a state,
  and pushes it as agent `codex`.

Any agent that can run a shell command with your session id available can skip
these and call `push` directly.

## Adapters in this directory

| Agent                       | Mechanism                              | Status                             |
| --------------------------- | -------------------------------------- | ---------------------------------- |
| [Claude Code](./claude-code/install.md) | Hooks, JSON on stdin (`claude-hook`)   | Confirmed                          |
| [Codex CLI](./codex/install.md)         | `notify` program (`codex-notify`)      | Confirmed                          |
| [Grok Code](./grok-code/install.md)     | Claude-compatible hooks, JSON on stdin | Confirmed mechanism; see caveats   |

## Verifying any adapter

Start the daemon and read its control socket (a JSON snapshot per second):

```sh
openmicrod &
socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/openmicro-ctl.sock" | head -1
```

Trigger the agent (or simulate a `push`) and confirm the `<agent>:<session>`
entry shows the expected `state`.
