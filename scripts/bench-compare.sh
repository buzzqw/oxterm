#!/usr/bin/env bash
#
# bench-compare.sh - head-to-head benchmark of Oxterm and other terminals.
#
# The default opponents are Ghostty, Alacritty and xfce4-terminal, whichever
# are installed. Every terminal is launched on the same display with an
# isolated HOME, the same font size and the same payload, so the numbers
# describe the products rather than the local configuration.
#
# Usage:
#   ./scripts/bench-compare.sh                  # print key=value measurements
#   ./scripts/bench-compare.sh --table          # print a Markdown table
#   ./scripts/bench-compare.sh --write-baseline # store them in bench/compare-baseline.txt
#   ./scripts/bench-compare.sh --check          # fail on a large regression
#
# What is measured (minimum of BENCH_RUNS runs):
#   startup_ms       spawn -> first visible window
#   idle_cpu_ms      CPU spent by the idle window over BENCH_IDLE_SECONDS
#   idle_rss_kb      resident memory once the first shell is idle
#   exec_ms          spawn -> the shell command inside the terminal starts
#   drain_ms         time to consume a fixed ANSI-heavy payload (throughput)
#   drain_plain_ms   same payload as plain text, without SGR sequences
#   chunks_ms        the plain payload written 80 bytes at a time by dd
#   peak_rss_kb      peak resident memory while consuming the ANSI payload
#   resize_ms        wall time and CPU for BENCH_RESIZE_STEPS window resizes
#   close_ms         WM_DELETE_WINDOW -> process exit (graceful shutdown)
#
# The ANSI payload costs more to parse, the plain one isolates the parser from
# the colour handling, and the chunked one keeps bytes and glyphs constant
# while trading large writes for one write per line.
#
# Caveat: Oxterm and xfce4-terminal both delegate parsing, grid and rendering to
# VTE, while Ghostty and Alacritty drive their own parsers and GPU renderers.
# The throughput numbers therefore compare VTE+GTK (plus Oxterm's broker)
# against their cores; they are a fair end-to-end product comparison, not a
# measurement of Oxterm's own code alone.
#
# Requirements: X11 (DISPLAY set) and xdotool for the window metrics. Without
# a display the script aborts; the GUI numbers need a real or virtual X server
# (CI uses xvfb-run). Wayland-only sessions are not supported yet.
#
# Environment:
#   OXTERM_BIN          binary to measure (default: target/release/oxterm)
#   GHOSTTY_BIN         Ghostty binary (default: ghostty from PATH)
#   ALACRITTY_BIN       Alacritty binary (default: alacritty from PATH)
#   XFCE4_TERMINAL_BIN  xfce4-terminal binary (default: xfce4-terminal from PATH)
#   BENCH_OPPONENTS     opponents to measure
#                       (default: "ghostty alacritty xfce4-terminal")
#   BENCH_RUNS          repetitions (default: 3)
#   BENCH_TOLERANCE     allowed multiplier over the baseline (default: 4)
#   BENCH_BASELINE      baseline file (default: bench/compare-baseline.txt)
#   BENCH_PAYLOAD_LINES lines in the throughput payload (default: 100000)
#   BENCH_IDLE_SECONDS  idle-CPU sampling window (default: 5)
#   BENCH_RESIZE_STEPS  resizes in the resize storm (default: 20)
#   BENCH_GHOSTTY_ARGS  extra arguments for Ghostty (word-split)
#   BENCH_ALACRITTY_ARGS extra arguments for Alacritty (word-split)
#   BENCH_XFCE4_TERMINAL_ARGS extra arguments for xfce4-terminal (word-split)
#   BENCH_FONT_SIZE     font size used by all terminals (default: 12)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OXTERM_BIN="${OXTERM_BIN:-${ROOT_DIR}/target/release/oxterm}"
GHOSTTY_BIN="${GHOSTTY_BIN:-$(command -v ghostty || true)}"
ALACRITTY_BIN="${ALACRITTY_BIN:-$(command -v alacritty || true)}"
XFCE4_TERMINAL_BIN="${XFCE4_TERMINAL_BIN:-$(command -v xfce4-terminal || true)}"
OPPONENTS="${BENCH_OPPONENTS:-ghostty alacritty xfce4-terminal}"
BASELINE="${BENCH_BASELINE:-${ROOT_DIR}/bench/compare-baseline.txt}"
RUNS="${BENCH_RUNS:-3}"
TOLERANCE="${BENCH_TOLERANCE:-4}"
PAYLOAD_LINES="${BENCH_PAYLOAD_LINES:-100000}"
IDLE_SECONDS="${BENCH_IDLE_SECONDS:-5}"
RESIZE_STEPS="${BENCH_RESIZE_STEPS:-20}"
FONT_SIZE="${BENCH_FONT_SIZE:-12}"
STARTUP_TIMEOUT=25
PAYLOAD_TIMEOUT=120
MODE="${1:-run}"

case "${MODE}" in
    run | --table | --write-baseline | --check) ;;
    *)
        printf '%s\n' "bench-compare: unknown option: ${MODE}" >&2
        exit 2
        ;;
esac

if [[ -z "${DISPLAY:-}" ]]; then
    printf '%s\n' "bench-compare: DISPLAY is not set; the GUI needs an X server" >&2
    printf '%s\n' "               (use xvfb-run -a ./scripts/bench-compare.sh)" >&2
    exit 1
fi

if ! command -v xdotool >/dev/null 2>&1; then
    printf '%s\n' "bench-compare: xdotool is required to wait for the window" >&2
    printf '%s\n' "               install it with 'sudo pacman -S xdotool'" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oxc.XXXXXX")"
APP_PID=""
CLASS_SEQ=0

cleanup() {
    stop_term
    rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

now_ns() {
    date +%s%N
}

deadline_ns() {
    printf '%s' "$(( $(now_ns) + $1 * 1000000000 ))"
}

# --- process helpers -------------------------------------------------------

children_of() {
    ps -o pid= --ppid "$1" 2>/dev/null || true
}

proc_kb() {
    awk -v key="$2" '$1 == key ":" { print $2; exit }' "/proc/$1/status" 2>/dev/null \
        || printf '0'
}

tree_sum_kb() {
    local pid="$1" key="$2" total=0 child
    [[ -r "/proc/${pid}/status" ]] || { printf '0'; return 0; }
    total="$(proc_kb "${pid}" "${key}")"
    for child in $(children_of "${pid}"); do
        total=$(( total + $(tree_sum_kb "${child}" "${key}") ))
    done
    printf '%s' "${total}"
}

# CPU time is the honest efficiency metric: wall time in the drain test is
# bounded by how each terminal acknowledges PTY output, while CPU time shows
# what the machine actually spends to consume the same bytes.
CLK_TCK="$(getconf CLK_TCK 2>/dev/null || printf '100')"

proc_ticks() {
    local stat rest
    stat="$(cat "/proc/$1/stat" 2>/dev/null)" || { printf '0'; return 0; }
    rest="${stat##*) }"
    # shellcheck disable=SC2086
    set -- ${rest}
    printf '%s' "$(( ${12:-0} + ${13:-0} ))"
}

# /proc/<pid>/stat counts in 10 ms clock ticks, which is too coarse to see the
# relay's per-byte cost. schedstat reports nanoseconds, but it is per *task*:
# summing the threads is required, otherwise a terminal that parses on worker
# threads (Ghostty) would look ten times cheaper than it is. Fall back to ticks
# when the kernel has no schedstats.
proc_ns() {
    local value
    if value="$(awk 'FNR == 1 { total += $1 } END { printf "%d", total }' \
        "/proc/$1/task/"*/schedstat 2>/dev/null)" && [[ -n "${value}" ]]; then
        printf '%s' "${value}"
        return 0
    fi
    printf '%s' "$(( $(proc_ticks "$1") * 1000000000 / CLK_TCK ))"
}

# CPU of the terminal itself: the process we started plus the helpers that run
# the same executable (Oxterm's PTY broker). The payload command and the shell
# are excluded on purpose: they would disappear before the final sample and
# their cost is not the terminal's.
term_ns() {
    local pid="$1" exe="$2" total child target
    total="$(proc_ns "${pid}")"
    for child in $(children_of "${pid}"); do
        target="$(readlink "/proc/${child}/exe" 2>/dev/null || true)"
        if [[ "${target}" == "${exe}" ]]; then
            total=$(( total + $(term_ns "${child}" "${exe}") ))
        fi
    done
    printf '%s' "${total}"
}

ns_to_ms() {
    awk -v ns="$1" 'BEGIN { printf "%.1f", ns / 1000000 }'
}

stop_term() {
    local pid="${APP_PID}" child waited=0
    APP_PID=""
    [[ -n "${pid}" ]] || return 0
    for child in $(children_of "${pid}"); do
        kill -TERM "${child}" 2>/dev/null || true
    done
    kill -TERM "${pid}" 2>/dev/null || true
    while kill -0 "${pid}" 2>/dev/null && (( waited < 100 )); do
        sleep 0.02
        waited=$(( waited + 1 ))
    done
    kill -KILL "${pid}" 2>/dev/null || true
    wait "${pid}" 2>/dev/null || true
}

# --- launching -------------------------------------------------------------

GHOSTTY_CLASS_OK=""
GHOSTTY_SINGLE_OK=""
# Ghostty exposes every configuration key as a command line option, but its
# --help does not list them. Ask for the defaults instead.
ghostty_option_exists() {
    local name="$1" cache key
    "${GHOSTTY_BIN}" +show-config --default 2>/dev/null \
        | grep -qE "^${name} = "
}

ghostty_supports_class() {
    if [[ -z "${GHOSTTY_CLASS_OK}" ]]; then
        if [[ -n "${GHOSTTY_BIN}" ]] && ghostty_option_exists class; then
            GHOSTTY_CLASS_OK=1
        else
            GHOSTTY_CLASS_OK=0
        fi
    fi
    [[ "${GHOSTTY_CLASS_OK}" == 1 ]]
}

ghostty_supports_single_instance() {
    if [[ -z "${GHOSTTY_SINGLE_OK}" ]]; then
        if [[ -n "${GHOSTTY_BIN}" ]] && ghostty_option_exists gtk-single-instance; then
            GHOSTTY_SINGLE_OK=1
        else
            GHOSTTY_SINGLE_OK=0
        fi
    fi
    [[ "${GHOSTTY_SINGLE_OK}" == 1 ]]
}

declare -a GHOSTTY_EXTRA=()
if [[ -n "${BENCH_GHOSTTY_ARGS:-}" ]]; then
    # shellcheck disable=SC2206
    GHOSTTY_EXTRA=(${BENCH_GHOSTTY_ARGS})
fi

declare -a ALACRITTY_EXTRA=()
if [[ -n "${BENCH_ALACRITTY_ARGS:-}" ]]; then
    # shellcheck disable=SC2206
    ALACRITTY_EXTRA=(${BENCH_ALACRITTY_ARGS})
fi

declare -a XFCE4_EXTRA=()
if [[ -n "${BENCH_XFCE4_TERMINAL_ARGS:-}" ]]; then
    # shellcheck disable=SC2206
    XFCE4_EXTRA=(${BENCH_XFCE4_TERMINAL_ARGS})
fi

# launch_term TERM CLASS HOME RUNTIME [ARGS...]
# Sets APP_PID. The class is still set for isolation and for future Wayland
# support, but windows are matched by _NET_WM_PID (see wait_for_window).
launch_term() {
    local term="$1" class="$2" home="$3" runtime="$4"
    shift 4
    local -a cmd
    mkdir -p "${home}/.config" "${runtime}"
    chmod 700 "${runtime}"
    case "${term}" in
        oxterm)
            cmd=("${OXTERM_BIN}" --no-restore --class "${class}" -o "font_size=${FONT_SIZE}")
            ;;
        ghostty)
            cmd=("${GHOSTTY_BIN}")
            if ghostty_supports_single_instance; then
                cmd+=("--gtk-single-instance=false")
            fi
            if ghostty_supports_class; then
                cmd+=("--class=${class}")
            fi
            cmd+=("--font-size=${FONT_SIZE}")
            cmd+=("${GHOSTTY_EXTRA[@]}")
            ;;
        alacritty)
            # --class still sets WM_CLASS for a desktop that cares; the window
            # is matched by _NET_WM_PID (see wait_for_window).
            cmd=("${ALACRITTY_BIN}" --class "${class}" -o "font.size=${FONT_SIZE}")
            cmd+=("${ALACRITTY_EXTRA[@]}")
            ;;
        xfce4-terminal)
            # --disable-server keeps every launch in its own process, which is
            # required for the isolated HOME and for the CPU/RSS figures.
            # --font takes a full Pango name, hence the generic family.
            cmd=("${XFCE4_TERMINAL_BIN}" --disable-server --class "${class}"
                "--font=Monospace ${FONT_SIZE}")
            cmd+=("${XFCE4_EXTRA[@]}")
            # xfce4-terminal's -e takes one command string; -x takes the rest
            # of the argv and is what the callers' "-e sh -c SCRIPT" means.
            if [[ "${1:-}" == -e ]]; then
                shift
                cmd+=(-x)
            fi
            ;;
    esac
    env -u OXTERM_SESSION \
        HOME="${home}" \
        XDG_RUNTIME_DIR="${runtime}" \
        XDG_CONFIG_HOME="${home}/.config" \
        "${cmd[@]}" "$@" >/dev/null 2>"${TMP_DIR}/launch.log" &
    APP_PID=$!
}

report_launch_failure() {
    local term="$1"
    printf '%s\n' "bench-compare: ${term} did not start" >&2
    if [[ -s "${TMP_DIR}/launch.log" ]]; then
        sed -n '1,10p' "${TMP_DIR}/launch.log" | sed 's/^/  /' >&2
    fi
}

# Ghostty ignores --class on X11 (its WM_CLASS stays "ghostty"), and even
# Oxterm's class would only be unique per run if the terminal honoured it. Match
# the window by _NET_WM_PID instead: it is exact and needs no per-terminal flag.
wait_for_window() {
    local pid="$1" limit child
    limit="$(deadline_ns "${2}")"
    while (( $(now_ns) < limit )); do
        if xdotool search --onlyvisible --pid "${pid}" >/dev/null 2>&1; then
            return 0
        fi
        # A wrapper may exec or fork, so also accept a window owned by a child.
        for child in $(children_of "${pid}"); do
            if xdotool search --onlyvisible --pid "${child}" >/dev/null 2>&1; then
                return 0
            fi
        done
        # A crash must not look like a slow start.
        kill -0 "${pid}" 2>/dev/null || [[ -n "$(children_of "${pid}")" ]] || return 1
        sleep 0.02
    done
    return 1
}

wait_for_file() {
    local file="$1" limit
    limit="$(deadline_ns "${2}")"
    while (( $(now_ns) < limit )); do
        [[ -e "${file}" ]] && return 0
        if [[ -n "${APP_PID}" ]] && ! kill -0 "${APP_PID}" 2>/dev/null; then
            [[ -e "${file}" ]] && return 0
            return 1
        fi
        sleep 0.005
    done
    return 1
}

# window_id_for PID -> first visible window of the process or a child.
window_id_for() {
    local pid="$1" id child
    id="$(xdotool search --onlyvisible --pid "${pid}" 2>/dev/null | head -n 1 || true)"
    if [[ -z "${id}" ]]; then
        for child in $(children_of "${pid}"); do
            id="$(xdotool search --onlyvisible --pid "${child}" 2>/dev/null | head -n 1 || true)"
            [[ -n "${id}" ]] && break
        done
    fi
    printf '%s' "${id}"
}

# A zombie still answers kill -0, so read the state from /proc instead.
proc_alive() {
    local state
    [[ -r "/proc/$1/stat" ]] || return 1
    state="$(awk '{ print $3 }' "/proc/$1/stat" 2>/dev/null || true)"
    [[ -n "${state}" && "${state}" != Z ]]
}

# Summing schedstat across live tasks is not monotonic: a short-lived thread
# takes its CPU time with it when it exits, which can make an idle window look
# negative. Snapshot per-task values instead and only add the delta of tasks
# that survived, so the result cannot go backwards.
task_snapshot() {
    local pid="$1" file tid
    for file in "/proc/${pid}/task/"*/schedstat; do
        [[ -r "${file}" ]] || continue
        tid="${file#/proc/${pid}/task/}"
        tid="${tid%/schedstat}"
        printf '%s:%s %s\n' "${pid}" "${tid}" \
            "$(awk 'NR == 1 { print $1 }' "${file}" 2>/dev/null || printf '0')"
    done
}

tree_snapshot() {
    local pid="$1" exe="$2" child
    task_snapshot "${pid}"
    for child in $(children_of "${pid}"); do
        if [[ "$(readlink "/proc/${child}/exe" 2>/dev/null || true)" == "${exe}" ]]; then
            tree_snapshot "${child}" "${exe}"
        fi
    done
}

snapshot_delta_ms() {
    awk 'FNR == NR { before[$1] = $2; next }
         { if ($1 in before) total += $2 - before[$1] }
         END { printf "%.1f", total / 1000000 }' "$1" "$2"
}

# --- measurements ----------------------------------------------------------

# measure_startup TERM -> "startup_ms idle_rss_kb idle_cpu_ms"
measure_startup() {
    local term="$1" class home runtime start end ms rss exe cpu_ms
    CLASS_SEQ=$(( CLASS_SEQ + 1 ))
    class="oxterm-bench-$$-${CLASS_SEQ}"
    home="${TMP_DIR}/${term}-home-${CLASS_SEQ}"
    runtime="${TMP_DIR}/${term}-rt-${CLASS_SEQ}"

    start="$(now_ns)"
    launch_term "${term}" "${class}" "${home}" "${runtime}"
    if ! wait_for_window "${APP_PID}" "${STARTUP_TIMEOUT}"; then
        stop_term
        report_launch_failure "${term}"
        return 1
    fi
    end="$(now_ns)"
    ms=$(( (end - start) / 1000000 ))
    sleep 0.05
    rss="$(tree_sum_kb "${APP_PID}" VmRSS)"
    # Let the first render and font setup finish before sampling the idle cost:
    # redraws, cursor blink and helper timers with no payload.
    # The shell is excluded like everywhere else, so only the terminal counts.
    sleep 0.5
    exe="$(readlink "/proc/${APP_PID}/exe" 2>/dev/null || true)"
    tree_snapshot "${APP_PID}" "${exe}" >"${home}/idle-before"
    sleep "${IDLE_SECONDS}"
    tree_snapshot "${APP_PID}" "${exe}" >"${home}/idle-after"
    cpu_ms="$(snapshot_delta_ms "${home}/idle-before" "${home}/idle-after")"
    stop_term
    printf '%s %s %s\n' "${ms}" "${rss}" "${cpu_ms}"
}

# measure_payload TERM COMMAND -> "exec_ms drain_ms peak_rss_kb cpu_ms gui_cpu_ms"
# COMMAND runs inside the terminal; it must touch $started before and $done
# after the workload so the script can time the part the terminal controls.
measure_payload() {
    local term="$1" command="$2" class home runtime started done script
    local t0 t_started t_done exec_ms drain_ms peak cpu gui_cpu cpu_start gui_cpu_start
    CLASS_SEQ=$(( CLASS_SEQ + 1 ))
    class="oxterm-bench-$$-${CLASS_SEQ}"
    home="${TMP_DIR}/${term}-home-${CLASS_SEQ}"
    runtime="${TMP_DIR}/${term}-rt-${CLASS_SEQ}"
    started="${home}/started"
    done="${home}/done"
    mkdir -p "${home}"
    rm -f "${started}" "${done}"
    script="touch '${started}'; ${command}; touch '${done}'; sleep 1"

    t0="$(now_ns)"
    launch_term "${term}" "${class}" "${home}" "${runtime}" -e sh -c "${script}"
    if ! wait_for_file "${started}" "${STARTUP_TIMEOUT}"; then
        stop_term
        report_launch_failure "${term}"
        return 1
    fi
    t_started="$(now_ns)"
    local exe
    exe="$(readlink "/proc/${APP_PID}/exe" 2>/dev/null || true)"
    cpu_start="$(term_ns "${APP_PID}" "${exe}")"
    gui_cpu_start="$(proc_ns "${APP_PID}")"
    if ! wait_for_file "${done}" "${PAYLOAD_TIMEOUT}"; then
        stop_term
        printf '%s\n' "bench-compare: ${term} did not finish the payload" >&2
        return 1
    fi
    t_done="$(now_ns)"
    # VTE parses the pty output on idle; let the backlog settle so VmHWM
    # reflects the scrollback the payload actually allocated.
    sleep 0.3
    peak="$(tree_sum_kb "${APP_PID}" VmHWM)"
    cpu="$(ns_to_ms "$(( $(term_ns "${APP_PID}" "${exe}") - cpu_start ))" )"
    gui_cpu="$(ns_to_ms "$(( $(proc_ns "${APP_PID}") - gui_cpu_start ))" )"
    stop_term

    exec_ms=$(( (t_started - t0) / 1000000 ))
    drain_ms=$(( (t_done - t_started) / 1000000 ))
    printf '%s %s %s %s %s\n' "${exec_ms}" "${drain_ms}" "${peak}" "${cpu}" "${gui_cpu}"
}

# measure_resize TERM -> "resize_ms resize_cpu_ms"
# A storm of window resizes on a fresh, empty terminal. The two sizes stay
# small enough that the whole animation fits on a 1920x1080 screen.
measure_resize() {
    local term="$1" class home runtime started id t0 t1 step width height exe cpu_ms
    CLASS_SEQ=$(( CLASS_SEQ + 1 ))
    class="oxterm-bench-$$-${CLASS_SEQ}"
    home="${TMP_DIR}/${term}-home-${CLASS_SEQ}"
    runtime="${TMP_DIR}/${term}-rt-${CLASS_SEQ}"
    started="${home}/started"
    mkdir -p "${home}"
    rm -f "${started}"

    launch_term "${term}" "${class}" "${home}" "${runtime}" \
        -e sh -c "touch '${started}'; sleep 120"
    if ! wait_for_window "${APP_PID}" "${STARTUP_TIMEOUT}"; then
        stop_term
        report_launch_failure "${term}"
        return 1
    fi
    wait_for_file "${started}" "${STARTUP_TIMEOUT}" || true
    sleep 0.2
    id="$(window_id_for "${APP_PID}")"
    if [[ -z "${id}" ]]; then
        stop_term
        printf '%s\n' "bench-compare: ${term} exposed no window id" >&2
        return 1
    fi
    read -r width height <<<"$(xdotool getwindowgeometry --shell "${id}" \
        | awk -F= '$1 == "WIDTH" { w = $2 } $1 == "HEIGHT" { h = $2 } END { print w, h }')"
    exe="$(readlink "/proc/${APP_PID}/exe" 2>/dev/null || true)"
    tree_snapshot "${APP_PID}" "${exe}" >"${home}/resize-before"
    t0="$(now_ns)"
    for step in $(seq "${RESIZE_STEPS}"); do
        if (( step % 2 )); then
            xdotool windowsize "${id}" $(( width + 120 )) $(( height + 60 ))
        else
            xdotool windowsize "${id}" "${width}" "${height}"
        fi
    done
    t1="$(now_ns)"
    # Give the terminal a fixed moment to apply the last request before the
    # CPU sample; the same 200 ms applies to every opponent.
    sleep 0.2
    tree_snapshot "${APP_PID}" "${exe}" >"${home}/resize-after"
    cpu_ms="$(snapshot_delta_ms "${home}/resize-before" "${home}/resize-after")"
    stop_term
    printf '%s %s\n' "$(( (t1 - t0) / 1000000 ))" "${cpu_ms}"
}

# measure_close TERM -> "close_ms"
# Graceful close: WM_DELETE_WINDOW through xdotool windowquit, then wait for
# the process to leave the process table (a zombie is not an exit).
measure_close() {
    local term="$1" class home runtime id t0 t1 limit
    CLASS_SEQ=$(( CLASS_SEQ + 1 ))
    class="oxterm-bench-$$-${CLASS_SEQ}"
    home="${TMP_DIR}/${term}-home-${CLASS_SEQ}"
    runtime="${TMP_DIR}/${term}-rt-${CLASS_SEQ}"

    launch_term "${term}" "${class}" "${home}" "${runtime}"
    if ! wait_for_window "${APP_PID}" "${STARTUP_TIMEOUT}"; then
        stop_term
        report_launch_failure "${term}"
        return 1
    fi
    # Let the first shell finish starting so the close is not racing exec.
    sleep 0.3
    id="$(window_id_for "${APP_PID}")"
    if [[ -z "${id}" ]]; then
        stop_term
        printf '%s\n' "bench-compare: ${term} exposed no window id" >&2
        return 1
    fi
    t0="$(now_ns)"
    xdotool windowquit "${id}" >/dev/null 2>&1 || true
    limit="$(deadline_ns 10)"
    while proc_alive "${APP_PID}" && (( $(now_ns) < limit )); do
        sleep 0.005
    done
    t1="$(now_ns)"
    if proc_alive "${APP_PID}"; then
        stop_term
        printf '%s\n' "bench-compare: ${term} did not close on WM_DELETE_WINDOW" >&2
        return 1
    fi
    wait "${APP_PID}" 2>/dev/null || true
    APP_PID=""
    printf '%s\n' "$(( (t1 - t0) / 1000000 ))"
}

# --- payload ---------------------------------------------------------------

# A fixed ANSI-heavy workload: coloured lines with SGR sequences, which is what
# `ls --color`, log tails and TUIs actually push through the terminal.
write_payload() {
    local path="$1"
    awk -v lines="${PAYLOAD_LINES}" 'BEGIN {
        text = "the quick brown fox jumps over the lazy dog 0123456789"
        for (i = 0; i < lines; i++) {
            printf "\033[3%dm%s %07d\033[0m \033[1;4m%s\033[0m\r\n",
                   i % 8, "line", i, text
        }
    }' >"${path}"
}

# The same workload as plain text, 80 bytes per line exactly, so the chunked
# variant can feed the very same bytes with one write per line via dd bs=80.
write_plain_payload() {
    local path="$1"
    awk -v lines="${PAYLOAD_LINES}" 'BEGIN {
        text = "the quick brown fox jumps over the lazy dog 0123456789 abcdefghij"
        for (i = 0; i < lines; i++) {
            printf "%-79s\n", substr(sprintf("line %07d %s", i, text), 1, 79)
        }
    }' >"${path}"
}

# --- main ------------------------------------------------------------------

if [[ ! -x "${OXTERM_BIN}" ]]; then
    printf '%s\n' "bench-compare: ${OXTERM_BIN} is missing; run 'cargo build --release' first" >&2
    exit 1
fi

TERMS=(oxterm)
for opponent in ${OPPONENTS}; do
    case "${opponent}" in
        ghostty)
            if [[ -n "${GHOSTTY_BIN}" && -x "${GHOSTTY_BIN}" ]]; then
                TERMS+=(ghostty)
            else
                printf '%s\n' "bench-compare: Ghostty not found; skipping it." >&2
                printf '%s\n' "               Install it with 'sudo pacman -S ghostty' or set GHOSTTY_BIN." >&2
            fi
            ;;
        alacritty)
            if [[ -n "${ALACRITTY_BIN}" && -x "${ALACRITTY_BIN}" ]]; then
                TERMS+=(alacritty)
            else
                printf '%s\n' "bench-compare: Alacritty not found; skipping it." >&2
                printf '%s\n' "               Install it with 'sudo pacman -S alacritty' or set ALACRITTY_BIN." >&2
            fi
            ;;
        xfce4-terminal)
            if [[ -n "${XFCE4_TERMINAL_BIN}" && -x "${XFCE4_TERMINAL_BIN}" ]]; then
                TERMS+=(xfce4-terminal)
            else
                printf '%s\n' "bench-compare: xfce4-terminal not found; skipping it." >&2
                printf '%s\n' "               Install it with 'sudo pacman -S xfce4-terminal' or set XFCE4_TERMINAL_BIN." >&2
            fi
            ;;
        *)
            printf '%s\n' "bench-compare: unknown opponent: ${opponent}" >&2
            printf '%s\n' "               supported: ghostty, alacritty, xfce4-terminal (see BENCH_OPPONENTS)" >&2
            exit 2
            ;;
    esac
done
if (( ${#TERMS[@]} == 1 )); then
    printf '%s\n' "bench-compare: no opponent found; measuring Oxterm alone." >&2
fi

PAYLOAD="${TMP_DIR}/payload.txt"
PLAIN_PAYLOAD="${TMP_DIR}/plain.txt"
write_payload "${PAYLOAD}"
write_plain_payload "${PLAIN_PAYLOAD}"
PAYLOAD_BYTES="$(stat -c %s "${PAYLOAD}")"
PLAIN_BYTES="$(stat -c %s "${PLAIN_PAYLOAD}")"

declare -A RESULT=()
METRIC_KEYS=(startup_ms idle_cpu_ms idle_rss_kb exec_ms drain_ms drain_cpu_ms
    drain_gui_cpu_ms drain_plain_ms drain_plain_cpu_ms chunks_ms chunks_cpu_ms
    resize_ms resize_cpu_ms close_ms peak_rss_kb)
have_failure=0

# Keep the fastest value per key: the minimum is far more stable than the mean
# when a run is interrupted by the desktop.
update_min() {
    local key="$1" value="$2"
    [[ -n "${value}" ]] || return 0
    if [[ -z "${RESULT[${key}]:-}" ]] \
        || awk -v a="${value}" -v b="${RESULT[${key}]}" 'BEGIN { exit !(a < b) }'; then
        RESULT["${key}"]="${value}"
    fi
}

for term in "${TERMS[@]}"; do
    for _ in $(seq "${RUNS}"); do
        if out="$(measure_startup "${term}")"; then
            read -r m r c <<<"${out}"
            update_min "${term}.startup_ms" "${m}"
            update_min "${term}.idle_rss_kb" "${r}"
            update_min "${term}.idle_cpu_ms" "${c}"
        else
            have_failure=1
        fi
        if out="$(measure_payload "${term}" "cat '${PAYLOAD}'")"; then
            read -r e d p c g <<<"${out}"
            update_min "${term}.exec_ms" "${e}"
            update_min "${term}.drain_ms" "${d}"
            update_min "${term}.peak_rss_kb" "${p}"
            update_min "${term}.drain_cpu_ms" "${c}"
            update_min "${term}.drain_gui_cpu_ms" "${g}"
        else
            have_failure=1
        fi
        if out="$(measure_payload "${term}" "cat '${PLAIN_PAYLOAD}'")"; then
            read -r _ d _ c _ <<<"${out}"
            update_min "${term}.drain_plain_ms" "${d}"
            update_min "${term}.drain_plain_cpu_ms" "${c}"
        else
            have_failure=1
        fi
        if out="$(measure_payload "${term}" "dd if='${PLAIN_PAYLOAD}' bs=80 status=none")"; then
            read -r _ d _ c _ <<<"${out}"
            update_min "${term}.chunks_ms" "${d}"
            update_min "${term}.chunks_cpu_ms" "${c}"
        else
            have_failure=1
        fi
        if out="$(measure_resize "${term}")"; then
            read -r rt rc <<<"${out}"
            update_min "${term}.resize_ms" "${rt}"
            update_min "${term}.resize_cpu_ms" "${rc}"
        else
            have_failure=1
        fi
        if out="$(measure_close "${term}")"; then
            update_min "${term}.close_ms" "${out}"
        else
            have_failure=1
        fi
    done
done

printf '%s\n' "bench-compare: payload ${PAYLOAD_BYTES} bytes ANSI, ${PLAIN_BYTES} bytes plain," \
    "${PAYLOAD_LINES} lines (${RUNS} runs, minimum reported)" >&2

print_table() {
    local term
    printf '| Measurement |'
    for term in "${TERMS[@]}"; do
        printf ' %s |' "${term}"
    done
    printf '\n| --- |'
    for _ in "${TERMS[@]}"; do
        printf ' --- |'
    done
    printf '\n'
    format_row "Startup to window (ms)" ".startup_ms"
    format_row "Idle CPU over ${IDLE_SECONDS} s (ms)" ".idle_cpu_ms"
    format_row "Spawn to command (ms)" ".exec_ms"
    format_row "Drain ${PAYLOAD_LINES} ANSI lines (ms)" ".drain_ms"
    format_row "ANSI throughput (MB/s)" ".drain_ms" mbs "${PAYLOAD_BYTES}"
    format_row "Drain ${PAYLOAD_LINES} plain lines (ms)" ".drain_plain_ms"
    format_row "Plain throughput (MB/s)" ".drain_plain_ms" mbs "${PLAIN_BYTES}"
    format_row "Drain in 80-byte writes (ms)" ".chunks_ms"
    format_row "Drain CPU ANSI, terminal (ms)" ".drain_cpu_ms"
    format_row "Drain CPU ANSI, GUI process (ms)" ".drain_gui_cpu_ms"
    format_row "Drain CPU plain, terminal (ms)" ".drain_plain_cpu_ms"
    format_row "Drain CPU in 80-byte writes (ms)" ".chunks_cpu_ms"
    format_row "${RESIZE_STEPS} resizes (ms)" ".resize_ms"
    format_row "Resize CPU (ms)" ".resize_cpu_ms"
    format_row "Close window to exit (ms)" ".close_ms"
    format_row "Idle RSS (MiB)" ".idle_rss_kb" mib
    format_row "Peak RSS under ANSI load (MiB)" ".peak_rss_kb" mib
}

format_row() {
    local label="$1" suffix="$2" unit="${3:-}" bytes="${4:-${PAYLOAD_BYTES}}" term value cell
    printf '| %s |' "${label}"
    for term in "${TERMS[@]}"; do
        value="${RESULT[${term}${suffix}]:-}"
        if [[ -z "${value}" ]]; then
            cell="n/a"
        elif [[ "${unit}" == mib ]]; then
            cell="$(awk -v kb="${value}" 'BEGIN { printf "%.1f", kb / 1024 }')"
        elif [[ "${unit}" == mbs ]]; then
            cell="$(awk -v ms="${value}" -v bytes="${bytes}" \
                'BEGIN { printf "%.1f", bytes / 1048576 / (ms / 1000) }')"
        else
            cell="${value}"
        fi
        printf ' %s |' "${cell}"
    done
    printf '\n'
}

baseline_value() {
    [[ -f "${BASELINE}" ]] || return 0
    awk -F= -v key="$1" '$1 == key { print $2; exit }' "${BASELINE}"
}

emit_results() {
    local term
    for term in "${TERMS[@]}"; do
        for suffix in "${METRIC_KEYS[@]}"; do
            local value="${RESULT[${term}.${suffix}]:-}"
            [[ -n "${value}" ]] || continue
            printf '%s.%s=%s\n' "${term}" "${suffix}" "${value}"
        done
    done
}

case "${MODE}" in
    run)
        emit_results
        ;;
    --table)
        print_table
        ;;
    --write-baseline)
        if (( have_failure )); then
            printf '%s\n' "bench-compare: not writing a baseline from failed runs" >&2
            exit 1
        fi
        {
            printf '# Generated by scripts/bench-compare.sh --write-baseline.\n'
            printf '# Times are milliseconds (minimum of %s runs); RSS is KiB.\n' "${RUNS}"
            printf '# Oxterm and its opponents are measured on the same display with an\n'
            printf '# isolated HOME; see the header of the script for the caveats.\n'
            emit_results
        } >"${BASELINE}"
        emit_results
        printf '%s\n' "bench-compare: wrote ${BASELINE}" >&2
        ;;
    --check)
        failed=0
        for term in "${TERMS[@]}"; do
            for suffix in "${METRIC_KEYS[@]}"; do
                key="${term}.${suffix}"
                measured="${RESULT[${key}]:-}"
                base="$(baseline_value "${key}")"
                if [[ -z "${base}" || -z "${measured}" ]]; then
                    printf 'skip %s (no baseline or not measured)\n' "${key}"
                    continue
                fi
                # CPU values carry one decimal, so compare with awk instead of
                # shell arithmetic (which only handles integers).
                if [[ "${suffix}" == *rss* ]]; then
                    limit="$(awk -v base="${base}" 'BEGIN { printf "%d", base * 3 + 65536 }')"
                else
                    limit="$(awk -v base="${base}" -v tol="${TOLERANCE}" \
                        'BEGIN { printf "%.1f", base * tol + 50 }')"
                fi
                if awk -v measured="${measured}" -v limit="${limit}" \
                    'BEGIN { exit !(measured > limit) }'; then
                    printf 'REGRESSION %s: %s > %s (baseline %s)\n' \
                        "${key}" "${measured}" "${limit}" "${base}"
                    failed=1
                else
                    printf 'ok %s: %s <= %s (baseline %s)\n' \
                        "${key}" "${measured}" "${limit}" "${base}"
                fi
            done
        done
        exit "${failed}"
        ;;
esac

if (( have_failure )); then
    printf '%s\n' "bench-compare: some runs failed; the numbers are incomplete" >&2
    exit 1
fi
