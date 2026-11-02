#!/bin/sh
# The simplest useful plugin: every event Oxterm sends is one JSON line on
# standard input. Append them to the private state directory.
exec cat >> "$OXTERM_PLUGIN_STATE/commands.jsonl"
