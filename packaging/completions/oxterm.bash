# bash completion for oxterm

_oxterm_sessions() {
    oxterm --list 2>/dev/null | awk 'NR > 1 && NF { print $1 }'
}

_oxterm() {
    local cur prev opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD - 1]}"

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
    else
        COMPREPLY=($(compgen -d -- "$cur"))
    fi
}

complete -o filenames -F _oxterm oxterm
