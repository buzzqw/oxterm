# Tab status colours

An example Oxterm plugin that colours the tab label from command events:

- **amber** while a command is running;
- **green** when it succeeds, cleared after three seconds;
- **red** when it fails, kept until the next command starts.

It is a small Python 3 program using only the standard library. It demonstrates
the plugin protocol: JSON events on standard input, actions through
`oxterm action`, and the `events` / `workspace` permissions.

## Install

From a clone of the repository:

```sh
oxterm plugin install examples/plugins/tab-status
oxterm plugin list
```

Restart Oxterm. The plugin starts with the next window. Requirements: `python3`
and an `oxterm` binary in `PATH` (or the `OXTERM` environment variable set by
the plugin host).

## Try it

```sh
true          # tab turns green for three seconds
sleep 1; false  # tab turns red and stays red
```

The colours are the constants at the top of `plugin.py`: edit them and run
`oxterm plugin install examples/plugins/tab-status --force`, then restart.

## Remove

```sh
oxterm plugin remove oxterm.tab-status --purge
```

## Notes

- The plugin runs for every pane; it keeps state per pane id.
- A command whose shell integration reports no exit status is treated as a
  failure (red), never as a success.
- It does not read terminal output; the `events` permission is enough. Add
  `read-screen` and the `output_line` event only if a plugin needs the text.
