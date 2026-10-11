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

__oxterm_ssh() {
    local socket_dir="${OXTERM_CONFIG_DIR:-${HOME}/.config/oxterm}"
    if [ ! -d "$socket_dir" ]; then
        mkdir -p -- "$socket_dir" 2>/dev/null
        chmod 700 -- "$socket_dir" 2>/dev/null
    fi
    if [ -d "$socket_dir" ]; then
        command ssh -o ControlMaster=auto -o "ControlPath=$socket_dir/oxterm-ssh-$$" "$@"
    else
        command ssh "$@"
    fi
}

if [ -n "$BASH_VERSION" ]; then
    alias ssh='__oxterm_ssh'
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
    alias ssh='__oxterm_ssh'
    printf '\033]133;A\007'
    __oxterm_osc133_notify A
    printf '\033]7;%s\007' "file://$PWD"
fi
"#;

pub const OSC133_FISH_SCRIPT: &str = r#"# Oxterm OSC 133 Fish Integration

if set -q OXTERM_OSC133_FIFO; and test -p "$OXTERM_OSC133_FIFO"
    exec 3>>"$OXTERM_OSC133_FIFO"
end

function __oxterm_osc133_notify
    if set -q OXTERM_OSC133_FIFO
        printf '%s\n' "$argv[1]" >&3 2>/dev/null
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

function __oxterm_ssh
    set -l socket_dir "$OXTERM_CONFIG_DIR"
    if test -z "$socket_dir"
        set socket_dir "$HOME/.config/oxterm"
    end
    if not test -d "$socket_dir"
        mkdir -p "$socket_dir" 2>/dev/null
        chmod 700 "$socket_dir" 2>/dev/null
    end
    if test -d "$socket_dir"
        command ssh -o ControlMaster=auto -o "ControlPath=$socket_dir/oxterm-ssh-$fish_pid" $argv
    else
        command ssh $argv
    end
end

function ssh
    __oxterm_ssh $argv
end
"#;
