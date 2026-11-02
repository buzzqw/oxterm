# Example plugins

Two small plugins that exercise the real plugin protocol. Both are installed
with `oxterm plugin install <directory>` and start with the next Oxterm window.

| Plugin | Language | Permissions | What it shows |
| --- | --- | --- | --- |
| [`tab-status`](tab-status/) | Python 3 | `events`, `workspace` | Colour the tab amber/green/red from command events and clear it with `oxterm action clear-tab-color` |
| [`command-log`](command-log/) | POSIX shell | `events` | Append every event as JSON Lines to the plugin's state directory |

```sh
oxterm plugin install examples/plugins/tab-status
oxterm plugin install examples/plugins/command-log
oxterm plugin list
```

A plugin is a directory with `plugin.json`:

```json
{
  "id": "acme.my-plugin",
  "version": "1.0.0",
  "api_version": 1,
  "permissions": ["events", "workspace"],
  "events": ["command_finished"],
  "command": ["./run.sh"]
}
```

Oxterm starts `command` with:

- `OXTERM_CONTROL` — the private control socket;
- `OXTERM` — the Oxterm binary to call for actions;
- `OXTERM_PLUGIN_ID`, `OXTERM_PLUGIN_DIR`, `OXTERM_PLUGIN_STATE`;
- `OXTERM_PLUGIN_PERMISSIONS`;
- one JSON event per line on standard input, filtered by `events`.

The plugin can call `oxterm action …`, `oxterm new …`, `oxterm send …` and the
other client verbs. Standard output is discarded; write diagnostics to standard
error (kept in `$OXTERM_PLUGIN_STATE/plugin.log`) or to a file in the state
directory.

Permissions are checked for requests that carry `OXTERM_PLUGIN_ID`, but they
are a safety rail, not a sandbox. Install only plugins you trust. See the
[manual](../../manual.md#plugins) for the full manifest and permission list.
