# Command log

The smallest useful Oxterm plugin: a POSIX shell script that appends every
event it receives to a JSON Lines file in its private state directory.

```sh
oxterm plugin install examples/plugins/command-log
```

Restart Oxterm, run a couple of commands, then:

```sh
tail -f "${XDG_DATA_HOME:-$HOME/.local/share}/oxterm/state/oxterm.command-log/commands.jsonl"
```

Each line is one event:

```json
{"event":"command_started","pane":"12345-2","command":"cargo test","cwd":"/src/app","time":"..."}
{"event":"command_finished","pane":"12345-2","command":"cargo test","exit":0,"duration_ms":4213,"cwd":"/src/app","time":"..."}
```

Change the `events` array in `plugin.json` to record other events, or add
`"read-screen"` to `permissions` and `"output_line"` to `events` to include the
output text (which may contain secrets).

## Remove

```sh
oxterm plugin remove oxterm.command-log --purge
```
