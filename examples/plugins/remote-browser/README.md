# Automatic remote browser

A plugin that opens the two-pane file browser whenever an **SSH** or **Mosh**
session starts. It is the plugin side of the WinSCP-like workflow: the browser
itself is part of Oxterm, and this plugin connects it to the moment you need it.

```sh
oxterm plugin install examples/plugins/remote-browser
```

Restart Oxterm, then open an SSH tab:

```sh
ssh localhost
```

If `localhost` is a saved connection or a host in `~/.ssh/config`, a
`Files: localhost` window opens next to the terminal.

It uses the `command_started` event, extracts the destination from the command
line, and runs `oxterm browse HOST`. It needs the `events` and `open-files`
permissions. A destination that Oxterm cannot resolve is ignored; a successful
or failed browser open is rate-limited to once per minute per host.

Remove it with:

```sh
oxterm plugin remove oxterm.remote-browser --purge
```
