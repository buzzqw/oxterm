# bash completion for oxterm

_oxterm_sessions() {
    oxterm --list 2>/dev/null | awk 'NR > 1 && NF { print $1 }'
}

_oxterm_control() {
    local verb=$1 cur=$2 prev=$3 opts i
    case "$verb" in
        new) opts="--cwd --title -h --help" ;;
        split) opts="--pane --vertical --horizontal --cwd --title -h --help" ;;
        send) opts="--pane --enter -h --help" ;;
        capture) opts="--pane --scrollback --all -h --help" ;;
        wait) opts="--pane --timeout -h --help" ;;
    esac

    case "$prev" in
        --cwd)
            COMPREPLY=($(compgen -d -- "$cur"))
            return 0
            ;;
        --pane)
            COMPREPLY=($(compgen -W "${OXTERM_PANE:-}" -- "$cur"))
            return 0
            ;;
        --title | --scrollback | --timeout)
            return 0
            ;;
    esac

    # Everything after `--` is the command to run in the new pane.
    if [[ "$verb" == "new" || "$verb" == "split" ]]; then
        for ((i = 2; i < COMP_CWORD; i++)); do
            if [[ "${COMP_WORDS[i]}" == "--" ]]; then
                COMPREPLY=($(compgen -c -- "$cur"))
                return 0
            fi
        done
    fi

    if [[ "$cur" == -* ]]; then
        COMPREPLY=($(compgen -W "$opts" -- "$cur"))
    elif [[ "$verb" == "new" ]]; then
        COMPREPLY=($(compgen -d -- "$cur"))
    elif [[ "$verb" == "send" && -z "$cur" ]]; then
        COMPREPLY=($(compgen -W "-" -- "$cur"))
    fi
    return 0
}

_oxterm() {
    local cur prev opts control
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD - 1]}"
    control="new split send capture wait"

    case "${COMP_WORDS[1]}" in
        new | split | send | capture | wait)
            _oxterm_control "${COMP_WORDS[1]}" "$cur" "$prev"
            return 0
            ;;
    esac

    opts="-w --working-directory -T --title -g --geometry -F --fullscreen
        -m --maximize --class --name -p --profile --config -o --option
        --font --font-size --new-window --no-restore --hold -e --execute
        --list --info --detach -a --attach -M --move -V --version -h --help
        --broker --"

    case "$prev" in
        -w | --working-directory | --config)
            COMPREPLY=($(compgen -f -- "$cur"))
            return 0
            ;;
        -a | --attach | -M | --move | --detach | --info)
            COMPREPLY=($(compgen -W "$(_oxterm_sessions)" -- "$cur"))
            return 0
            ;;
        -e | --execute)
            COMPREPLY=($(compgen -c -- "$cur"))
            return 0
            ;;
        --font)
            COMPREPLY=($(compgen -W "$(fc-list : family 2>/dev/null | tr ',' '\n' | sort -u)" -- "$cur"))
            return 0
            ;;
        -p | --profile | -T | --title | -o | --option | -g | --geometry | --class | --name | --font-size | --broker)
            return 0
            ;;
    esac

    if [[ "$cur" == -* ]]; then
        COMPREPLY=($(compgen -W "$opts" -- "$cur"))
    elif [[ "$COMP_CWORD" -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$control" -- "$cur") $(compgen -d -- "$cur"))
    else
        COMPREPLY=($(compgen -d -- "$cur"))
    fi
}

complete -o filenames -F _oxterm oxterm
