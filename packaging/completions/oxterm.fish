# fish completion for oxterm

# Global options apply until a control subcommand is chosen.
function __oxterm_global_option
    not __fish_seen_subcommand_from new split send capture wait
end

complete -c oxterm -f

# BEGIN GENERATED OPTIONS
complete -c oxterm -n '__oxterm_global_option' -s w -l working-directory -d 'Working directory for the new terminal' -r -a '(__fish_complete_directories)'
complete -c oxterm -n '__oxterm_global_option' -l new-window -d 'Open a new window instead of reusing a running instance'
complete -c oxterm -n '__oxterm_global_option' -l no-restore -d 'Do not restore the last session'
complete -c oxterm -n '__oxterm_global_option' -s T -l title -d 'Set a fixed window title (apps cannot override it)' -r
complete -c oxterm -n '__oxterm_global_option' -s g -l geometry -d 'Initial size in character cells (e.g. 120x40)' -r
complete -c oxterm -n '__oxterm_global_option' -s F -l fullscreen -d 'Start in fullscreen mode'
complete -c oxterm -n '__oxterm_global_option' -s m -l maximize -d 'Start maximized'
complete -c oxterm -n '__oxterm_global_option' -l class -d 'Set the WM_CLASS class part (window manager rules)' -r
complete -c oxterm -n '__oxterm_global_option' -l name -d 'Set the WM_CLASS instance name' -r
complete -c oxterm -n '__oxterm_global_option' -s p -l profile -d 'Start with a saved profile (session only)' -r
complete -c oxterm -n '__oxterm_global_option' -l config -d 'Use an alternative settings file' -r -F
complete -c oxterm -n '__oxterm_global_option' -s o -l option -d 'Override a setting for this session (repeatable)' -r
complete -c oxterm -n '__oxterm_global_option' -l font -d 'Override the font family for this session' -r
complete -c oxterm -n '__oxterm_global_option' -l font-size -d 'Override the font size for this session' -r
complete -c oxterm -n '__oxterm_global_option' -l hold -d 'Keep the terminal open after the command exits'
complete -c oxterm -n '__oxterm_global_option' -s e -l execute -d 'Run a command instead of the configured shell' -r -a '(__fish_complete_command)'
complete -c oxterm -n '__oxterm_global_option' -l list -d 'List active Oxterm terminals (first column is the session ID)'
complete -c oxterm -n '__oxterm_global_option' -l info -d 'Show one active terminal' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -s a -l attach -d 'Share a live session in this terminal' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -s M -l move -d 'Take a live session over in this terminal' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -l detach -d 'Disconnect every remote client of a session' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -l broker -d 'Run the persistent PTY broker (internal)' -r -F
complete -c oxterm -n '__oxterm_global_option' -l show-config -d 'Print the effective settings as JSON and exit'
complete -c oxterm -n '__oxterm_global_option' -l default -d 'With --show-config, print the built-in defaults'
complete -c oxterm -n '__oxterm_global_option' -l validate-config -d 'Validate a settings file and exit' -r -F
complete -c oxterm -n '__oxterm_global_option' -l list-settings -d 'List every setting with its type and default'
complete -c oxterm -n '__oxterm_global_option' -l list-themes -d 'List the built-in color schemes'
complete -c oxterm -n '__oxterm_global_option' -l list-keybinds -d 'List the keyboard shortcuts'
complete -c oxterm -n '__oxterm_global_option' -l list-actions -d 'List the scriptable control actions'
complete -c oxterm -n '__oxterm_global_option' -l diagnostics -d 'Print environment, paths and limits for bug reports'
complete -c oxterm -n '__oxterm_global_option' -l edit-config -d 'Open the settings file in the configured editor'
complete -c oxterm -n '__oxterm_global_option' -l list-fonts -d 'List installed font families for font_name'
complete -c oxterm -n '__oxterm_global_option' -s V -l version -d 'Show the Oxterm version'
complete -c oxterm -n '__oxterm_global_option' -s h -l help -d 'Show this help'
# END GENERATED OPTIONS

# Scriptable control subcommands.
complete -c oxterm -n '__fish_use_subcommand' -a new -d 'Open a tab and print its pane id'
complete -c oxterm -n '__fish_use_subcommand' -a split -d 'Split a single-pane window'
complete -c oxterm -n '__fish_use_subcommand' -a send -d 'Type into a pane'
complete -c oxterm -n '__fish_use_subcommand' -a capture -d 'Print a pane\'s text'
complete -c oxterm -n '__fish_use_subcommand' -a wait -d 'Wait for the next command to finish'

complete -c oxterm -n '__fish_seen_subcommand_from new split' -l cwd -d 'Working directory' -r -a '(__fish_complete_directories)'
complete -c oxterm -n '__fish_seen_subcommand_from new split' -l title -d 'Tab title' -r
complete -c oxterm -n '__fish_seen_subcommand_from new' -a '(__fish_complete_directories)'
complete -c oxterm -n '__fish_seen_subcommand_from split send capture wait' -l pane -d 'Target pane id' -r
complete -c oxterm -n '__fish_seen_subcommand_from split' -l vertical -d 'Vertical split'
complete -c oxterm -n '__fish_seen_subcommand_from split' -l horizontal -d 'Horizontal split'
complete -c oxterm -n '__fish_seen_subcommand_from split' -a '(__fish_complete_command)'
complete -c oxterm -n '__fish_seen_subcommand_from send' -l enter -d 'Append a carriage return'
complete -c oxterm -n '__fish_seen_subcommand_from capture' -l scrollback -d 'History lines to print' -r
complete -c oxterm -n '__fish_seen_subcommand_from capture' -l all -d 'Print the whole buffer'
complete -c oxterm -n '__fish_seen_subcommand_from wait' -l timeout -d 'Timeout in seconds' -r
