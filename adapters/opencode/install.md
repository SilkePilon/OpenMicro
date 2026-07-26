# opencode adapter — install

This adapter wires [opencode](https://opencode.ai) to OpenMicro so your macropad
reflects what the agent is doing.

Unlike the other adapters, nothing is merged into a config file. opencode
auto-loads every file in its plugin directory, so the adapter is a plugin file
that OpenMicro owns outright: installing writes it, uninstalling deletes it, and
no file the user wrote is ever touched.

## Installing

Run `openmicro`, pick **Coding agents**, tick **opencode**. That writes:

    ~/.config/opencode/plugin/openmicro.js

Nothing else changes — `opencode.json` is not edited.

To do it by hand, copy `plugin.js` from this directory to that path.

## How it works

opencode plugins are ES modules that export an async function returning a set of
hooks. The adapter subscribes to four of them and shells out to `openmicro-hook`
on each:

| opencode hook / event  | OpenMicro state     | Fires when                        |
| ---------------------- | ------------------- | --------------------------------- |
| `chat.message`         | `thinking`          | a prompt is submitted             |
| `tool.execute.before`  | `working`           | the agent runs a tool             |
| `permission.ask`       | `awaiting_approval` | the agent needs your decision     |
| `permission.replied`   | `working`           | you answered, and it carries on   |
| `session.idle`         | `idle`              | the turn is finished              |

Every one of them carries a `sessionID`, so opencode sessions get their own
macropad slots and several running at once stay distinguishable.

The call is:

```sh
openmicro-hook push --agent opencode --session <sessionID> --state <state>
```

spawned detached with stdio discarded and immediately `unref()`d, so the agent
never waits on it. `openmicro-hook` always exits 0 and silently no-ops when the
daemon is down, so a stopped daemon costs nothing.

## Requirements

`openmicro-hook` must be on the `PATH` of the process running opencode. If it is
not, the plugin loads and does nothing — no errors, and no lights.

The plugin uses `Bun.spawn`, which is available because opencode runs its plugins
on Bun.

## Notes

- The plugin directory is read at startup, so opencode must be restarted after
  installing.
- `~/.config/opencode/plugins/` (plural) is also honoured by opencode. OpenMicro
  writes the singular `plugin/`; if you have a copy in both, both will load and
  every state will be pushed twice. Harmless, but pick one.
- Reinstalling overwrites the file, which is how an older adapter gets refreshed.
  A file in that path that OpenMicro did not write is reported as blocked and
  left alone rather than clobbered.
