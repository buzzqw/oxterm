//! Shell integration scripts installed for bash/zsh and fish. They report
//! OSC 133 prompt/command events to Oxterm through a FIFO.

pub const OSC133_SCRIPT: &str = r#"# Oxterm OSC 133 Shell Integration
# Oxterm loads this file only in shells started by Oxterm.

# Let the shell pick a free high descriptor instead of clobbering fd 3,
# which user rc files and child programs may rely on.
__OXTERM_OSC133_FD=
if [ -n "$OXTERM_OSC133_FIFO" ] && [ -p "$OXTERM_OSC133_FIFO" ]; then
    exec {__OXTERM_OSC133_FD}>>"$OXTERM_OSC133_FIFO"
fi

__OXTERM_OSC133_READY=0

__oxterm_osc133_notify() {
    [ -n "$__OXTERM_OSC133_FD" ] && printf '%s\n' "$1" >&"$__OXTERM_OSC133_FD" 2>/dev/null
    return 0
}

__oxterm_reattach_replaced_cwd() {
    if [ -n "$PWD" ] && [ -d "$PWD" ] && [ ! . -ef "$PWD" ]; then
        builtin cd -- "$PWD"
    fi
}

__oxterm_osc133_preexec() {
    __oxterm_reattach_replaced_cwd
    [ "$__OXTERM_OSC133_READY" = "1" ] || return
    case "$BASH_COMMAND" in
        __oxterm_osc133_*) return ;;
    esac
    __OXTERM_OSC133_READY=0
    printf '\033]133;C\007'
    __oxterm_osc133_notify "C${BASH_COMMAND//$'\n'/ }"
}
__oxterm_osc133_precmd() {
    local _exit=$?
    __oxterm_reattach_replaced_cwd
    __OXTERM_OSC133_READY=1
    printf '\033]133;D;%s\007' "$_exit"
    __oxterm_osc133_notify "D$_exit"
    printf '\033]133;A\007'
    __oxterm_osc133_notify A
    printf '\033]7;%s\007' "file://$PWD"
}

# ssh wrapper: share one authenticated connection per host/port/user across
# all tabs (instant new sessions, cheap stats probes) unless the user's own
# ssh config already sets ControlPath, and enable keepalives when none are
# configured so dead connections are detected instead of freezing the tab.
# __oxterm_ssh_options fills __oxterm_ssh_opts for the given ssh arguments.
__oxterm_ssh_options() {
    __oxterm_ssh_opts=()
    local socket_dir="${OXTERM_CONFIG_DIR:-${HOME}/.config/oxterm}"
    local cfg
    cfg=$'\n'"$(command ssh -G "$@" 2>/dev/null)"$'\n'
    if [ "${OXTERM_SSH_SHARING:-1}" = "1" ] && [[ $cfg != *$'\ncontrolpath '* ]]; then
        if [ ! -d "$socket_dir" ]; then
            mkdir -p -- "$socket_dir" 2>/dev/null
            chmod 700 -- "$socket_dir" 2>/dev/null
        fi
        if [ -d "$socket_dir" ]; then
            __oxterm_ssh_opts+=(-o ControlMaster=auto -o "ControlPath=$socket_dir/cm-%C" -o ControlPersist=10m)
        fi
    fi
    if [[ $cfg == *$'\nserveraliveinterval 0\n'* ]]; then
        __oxterm_ssh_opts+=(-o ServerAliveInterval=30 -o ServerAliveCountMax=4)
    fi
}

__oxterm_ssh() {
    __oxterm_ssh_options "$@"
    command ssh "${__oxterm_ssh_opts[@]}" "$@"
}

# autossh: same options for the ssh it supervises. Its own leading options
# (-M port, -f, -V) are not ssh options and are kept in front.
__oxterm_autossh() {
    local -a pre=()
    while [ $# -gt 0 ]; do
        case "$1" in
            -M) pre+=("$1" "$2"); shift 2 || break ;;
            -M*|-f|-V) pre+=("$1"); shift ;;
            *) break ;;
        esac
    done
    __oxterm_ssh_options "$@"
    command autossh "${pre[@]}" "${__oxterm_ssh_opts[@]}" "$@"
}

if [ -n "$BASH_VERSION" ]; then
    alias ssh='__oxterm_ssh' autossh='__oxterm_autossh'
    trap '__oxterm_osc133_preexec' DEBUG
    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND+=(__oxterm_osc133_precmd)
    else
        PROMPT_COMMAND="${PROMPT_COMMAND}${PROMPT_COMMAND:+;}__oxterm_osc133_precmd"
    fi
    printf '\033]133;A\007'
    __oxterm_osc133_notify A
    printf '\033]7;%s\007' "file://$PWD"
elif [ -n "$ZSH_VERSION" ]; then
    autoload -Uz add-zsh-hook
    __oxterm_zsh_preexec() {
        __oxterm_reattach_replaced_cwd
        printf '\033]133;C\007'
        __oxterm_osc133_notify "C${1//$'\n'/ }"
    }
    __oxterm_zsh_precmd() {
        local _exit=$?
        __oxterm_reattach_replaced_cwd
        printf '\033]133;D;%s\007' "$_exit"
        __oxterm_osc133_notify "D$_exit"
        printf '\033]133;A\007'
        __oxterm_osc133_notify A
        printf '\033]7;%s\007' "file://$PWD"
    }
    add-zsh-hook preexec __oxterm_zsh_preexec
    add-zsh-hook precmd __oxterm_zsh_precmd
    alias ssh='__oxterm_ssh' autossh='__oxterm_autossh'
    printf '\033]133;A\007'
    __oxterm_osc133_notify A
    printf '\033]7;%s\007' "file://$PWD"
fi
"#;

pub const OSC133_FISH_SCRIPT: &str = r#"# Oxterm OSC 133 Fish Integration

# fish cannot keep a descriptor open with a bare `exec 3>>file` (it prints
# the exec help instead), so open the FIFO for each event. Oxterm holds the
# read end open for the shell's lifetime, so the open never blocks.
function __oxterm_osc133_notify
    if set -q OXTERM_OSC133_FIFO; and test -p "$OXTERM_OSC133_FIFO"
        printf '%s\n' "$argv[1]" >>"$OXTERM_OSC133_FIFO" 2>/dev/null
    end
end

function __oxterm_osc133_preexec --on-event fish_preexec
    printf '\e]133;C\a'
    __oxterm_osc133_notify "C$argv[1]"
end

function __oxterm_osc133_postexec --on-event fish_postexec
    set -l _exit $status
    printf '\e]133;D;%s\a' "$_exit"
    __oxterm_osc133_notify "D$_exit"
end

function __oxterm_osc133_prompt --on-event fish_prompt
    printf '\e]133;A\a'
    __oxterm_osc133_notify A
    printf '\e]7;file://%s\a' "$PWD"
end

# See the bash/zsh wrapper: shared connections and keepalives, unless the
# user's ssh config already configures them.
function __oxterm_ssh_options
    set -l opts
    set -l cfg (command ssh -G $argv 2>/dev/null)
    if test "$OXTERM_SSH_SHARING" != 0; and not string match -qr '^controlpath ' -- $cfg
        set -l socket_dir "$OXTERM_CONFIG_DIR"
        if test -z "$socket_dir"
            set socket_dir "$HOME/.config/oxterm"
        end
        if not test -d "$socket_dir"
            mkdir -p "$socket_dir" 2>/dev/null
            chmod 700 "$socket_dir" 2>/dev/null
        end
        if test -d "$socket_dir"
            set opts -o ControlMaster=auto -o "ControlPath=$socket_dir/cm-%C" -o ControlPersist=10m
        end
    end
    if contains -- "serveraliveinterval 0" $cfg
        set -a opts -o ServerAliveInterval=30 -o ServerAliveCountMax=4
    end
    string join \n -- $opts
end

function __oxterm_ssh
    command ssh (__oxterm_ssh_options $argv) $argv
end

function ssh
    __oxterm_ssh $argv
end

function autossh
    set -l pre
    while set -q argv[1]
        switch $argv[1]
            case -M
                set -a pre $argv[1..2]
                set -e argv[1..2]
            case '-M*' -f -V
                set -a pre $argv[1]
                set -e argv[1]
            case '*'
                break
        end
    end
    command autossh $pre (__oxterm_ssh_options $argv) $argv
end
"#;
