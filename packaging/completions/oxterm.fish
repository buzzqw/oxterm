# fish completion for oxterm

complete -c oxterm -f

complete -c oxterm -s w -l working-directory -d 'Working directory' -r -a '(__fish_complete_directories)'
complete -c oxterm -s T -l title -d 'Fixed window title' -r
complete -c oxterm -s g -l geometry -d 'Initial size in cells' -r
complete -c oxterm -s F -l fullscreen -d 'Start fullscreen'
complete -c oxterm -s m -l maximize -d 'Start maximized'
complete -c oxterm -l class -d 'WM_CLASS class' -r
complete -c oxterm -l name -d 'WM_CLASS instance name' -r
complete -c oxterm -s p -l profile -d 'Saved profile' -r
complete -c oxterm -l config -d 'Alternative settings file' -r -F
complete -c oxterm -s o -l option -d 'Session setting override KEY=VALUE' -r
complete -c oxterm -l font -d 'Font family override' -r
complete -c oxterm -l font-size -d 'Font size override' -r
complete -c oxterm -l new-window -d 'Open a new window'
complete -c oxterm -l no-restore -d 'Do not restore the last session'
complete -c oxterm -l hold -d 'Keep the terminal open after the command exits'
complete -c oxterm -s e -l execute -d 'Run a command' -r -a '(__fish_complete_command)'
complete -c oxterm -l list -d 'List active terminals'
complete -c oxterm -l info -d 'Show one active terminal' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -l detach -d 'Disconnect remote clients' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -l broker -d 'Internal PTY broker' -r -F
complete -c oxterm -s a -l attach -d 'Share a live session' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -s M -l move -d 'Take a live session over' -r -a '(oxterm --list 2>/dev/null | tail -n +2 | string split -f1 \t)'
complete -c oxterm -s V -l version -d 'Show version'
complete -c oxterm -s h -l help -d 'Show help'
