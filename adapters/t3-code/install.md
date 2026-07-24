# T3 Code adapter — install

> **Mechanism unconfirmed — verify against your version.**
>
> As of this writing, [T3 Code](https://github.com/pingdotgg/t3code) is a web
> GUI that *manages* other coding agents (Codex, Claude, Cursor, OpenCode, …). It
> is a CLI-to-GUI bridge and does **not** appear to expose its own hook / notify
> / plugin mechanism for external state reporting; comparisons note it has "no
> notifications built-in." We could not source a T3 Code hook API, so we do not
> invent one here.

## Recommended: use the underlying agent's adapter

Because T3 Code runs an underlying agent, the reliable way to light up your
macropad is to install the adapter for whichever agent T3 Code is driving:

- Codex → [`../codex/install.md`](../codex/install.md)
- Claude Code → [`../claude-code/install.md`](../claude-code/install.md)

Those hooks/notify integrations fire regardless of whether the agent is launched
from a terminal or from T3 Code.

## If a T3 Code hook mechanism exists in your version

If your T3 Code build *does* expose a way to run a shell command on lifecycle
transitions (check its docs/settings for "hooks", "notify", "events", or
"commands"), wire it to the universal contract — call `openmicro-hook push` on
each transition:

```sh
# thinking | working | awaiting_approval | idle
openmicro-hook push --agent t3 --session "<session-id>" --state working
```

Map T3 Code's events to the four OpenMicro states:

| Lifecycle moment                    | OpenMicro state     |
| ----------------------------------- | ------------------- |
| user submits a prompt               | `thinking`          |
| a tool/command starts running       | `working`           |
| the agent asks for approval/input   | `awaiting_approval` |
| the turn finishes                   | `idle`              |

Use a stable `--session` id per T3 Code project/conversation (or `default` if
none is available). `openmicro-hook` always exits `0`, so wiring it can never
break T3 Code even when `openmicrod` is not running.

## Verify

```sh
openmicrod &
openmicro-hook push --agent t3 --session s1 --state working
socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/openmicro-ctl.sock" | head -1
# -> {"sessions":[{"agent":"t3","session":"s1","state":"working",...}],...}
```

If you confirm a concrete T3 Code hook mechanism, please replace this template
with the real configuration and drop the "unconfirmed" banner.
