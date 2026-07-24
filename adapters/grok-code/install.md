# Grok Code adapter — install

> **Mechanism: confirmed (Claude-compatible hooks). Payload `session_id` field:
> unconfirmed — verify against your version.**

This adapter targets the open-source [`superagent-ai/grok-cli`](https://github.com/superagent-ai/grok-cli)
coding agent, which exposes a hook system modelled closely on Claude Code:

- Hooks live in **`~/.grok/user-settings.json`** under a `"hooks"` key.
- Hook commands receive the event details as **JSON on stdin** and may return
  JSON on stdout (exit `0` = success, `2` = block, other = non-blocking error).
- Supported events include `UserPromptSubmit`, `PreToolUse`, `Notification`,
  `Stop` (among many others) — the same names Claude Code uses.

(Source: `superagent-ai/grok-cli` hooks documentation. Note there are several
"Grok" coding tools; xAI's own *Grok Build* CLI also advertises a hook system.
This adapter is written for `superagent-ai/grok-cli`; the same
`openmicro-hook push` fallback below works for any of them.)

Because the mechanism mirrors Claude Code, this adapter reuses
`openmicro-hook claude-hook`, passing `--agent grok` so sessions group under the
`grok` name. `claude-hook` reads stdin and extracts `session_id`.

## State mapping

| Grok event         | OpenMicro state     |
| ------------------ | ------------------- |
| `UserPromptSubmit` | `thinking`          |
| `PreToolUse`       | `working`           |
| `Notification`     | `awaiting_approval` |
| `Stop`             | `idle`              |

## Caveat — the session id

Claude Code's stdin JSON includes `session_id`, and `grok-cli` mirrors Claude's
hook format, but we have **not confirmed** that grok-cli's stdin payload carries
a `session_id` field with that exact name. If it does, sessions appear as
`grok:<id>`. If it does not, `claude-hook` falls back to `grok:default` — still
correct, just a single shared slot. To confirm what your version emits,
temporarily point a hook at a dump command:

```sh
# in a hook: "command": "cat > /tmp/grok-hook-payload.json"
cat /tmp/grok-hook-payload.json   # inspect for a session id field
```

If the field has a different name, use the generic `push` form instead and
extract it yourself, e.g. with `jq`:

```json
{ "type": "command",
  "command": "openmicro-hook push --agent grok --session \"$(jq -r '.session_id // \"default\"')\" --state working" }
```

## Install

1. Ensure `openmicro-hook` is on `PATH` (`which openmicro-hook`).
2. Merge the `"hooks"` object from this directory's `hooks.json` into
   `~/.grok/user-settings.json` (create the file if absent; keep it valid JSON —
   `python3 -m json.tool ~/.grok/user-settings.json`).
3. Start the daemon (`openmicrod`) and interact with grok-cli.

## Verify

```sh
openmicrod &
echo '{"session_id":"g1","hook_event_name":"PreToolUse"}' \
  | openmicro-hook claude-hook --agent grok --state working
socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/openmicro-ctl.sock" | head -1
# -> {"sessions":[{"agent":"grok","session":"g1","state":"working",...}],...}
```

If the mapped slot does not move during real use, re-check `openmicro-hook` is on
`PATH`, that `~/.grok/user-settings.json` parses, and that `openmicrod` is
running.
