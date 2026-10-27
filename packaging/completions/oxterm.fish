# fish completion for oxterm

# Global options apply until a control subcommand is chosen.
function __oxterm_global_option
    not __fish_seen_subcommand_from new split send capture wait
end

complete -c oxterm -f

complete -c oxterm -n '__oxterm_global_option' -s w -l working-directory -d 'Working directory' -r -a '(__fish_complete_directories)'
complete -c oxterm -n '__oxterm_global_option' -s T -l title -d 'Fixed window title' -r
complete -c oxterm -n '__oxterm_global_option' -s g -l geometry -d 'Initial size in cells' -r
complete -c oxterm -n '__oxterm_global_option' -s F -l fullscreen -d 'Start fullscreen'
complete -c oxterm -n '__oxterm_global_option' -s m -l maximize -d 'Start maximized'
complete -c oxterm -n '__oxterm_global_option' -l class -d 'WM_CLASS class' -r
complete -c oxterm -n '__oxterm_global_option' -l name -d 'WM_CLASS instance name' -r
complete -c oxterm -n '__oxterm_global_option' -s p -l profile -d 'Saved profile' -r
complete -c oxterm -n '__oxterm_global_option' -l config -d 'Alternative settings file' -r -F
complete -c oxterm -n '__oxterm_global_option' -s o -l option -d 'Session setting override KEY=VALUE' -r
complete -c oxterm -n '__oxterm_global_option' -l font -d 'Font family override' -r
complete -c oxterm -n '__oxterm_global_option' -l font-size -d 'Font size override' -r
complete -c oxterm -n '__oxterm_global_option' -l new-window -d 'Open a new window'
complete -c oxterm -n '__oxterm_global_option' -l no-restore -d 'Do not restore the last session'
complete -c oxterm -n '__oxterm_global_option' -l hold -d 'Keep the terminal open after the command exits'
complete -c oxterm -n '__oxterm_global_option' -s e -l execute -d 'Run a command' -r -a '(__fish_complete_command)'
complete -c oxterm -n '__oxterm_global_option' -l list -d 'List active terminals'
complete -c oxterm -n '__oxterm_global_option' -l info -d 'Show one active terminal' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -l detach -d 'Disconnect remote clients' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -l broker -d 'Internal PTY broker' -r -F
complete -c oxterm -n '__oxterm_global_option' -s a -l attach -d 'Share a live session' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -s M -l move -d 'Take a live session over' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -n '__oxterm_global_option' -s V -l version -d 'Show version'
complete -c oxterm -s h -l help -d 'Show help'

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
