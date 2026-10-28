#!/usr/bin/env bash
#
# bench-compare.sh - head-to-head benchmark of Oxterm and another terminal.
#
# The default opponent is Ghostty. Both terminals are launched on the same
# display with an isolated HOME, the same font size and the same payload, so
# the numbers describe the products rather than the local configuration.
#
# Usage:
#   ./scripts/bench-compare.sh                  # print key=value measurements
#   ./scripts/bench-compare.sh --table          # print a Markdown table
#   ./scripts/bench-compare.sh --write-baseline # store them in bench/compare-baseline.txt
#   ./scripts/bench-compare.sh --check          # fail on a large regression
#
# What is measured (minimum of BENCH_RUNS runs):
#   startup_ms     spawn -> first visible window
#   idle_rss_kb    resident memory once the first shell is idle
#   exec_ms        spawn -> the shell command inside the terminal starts
#   drain_ms       time to consume a fixed ANSI-heavy payload (throughput)
#   peak_rss_kb    peak resident memory while consuming that payload
#
# Caveat: Oxterm delegates parsing, grid and rendering to VTE, while Ghostty
# has its own GPU renderer. The throughput numbers therefore compare VTE+GTK
# with Ghostty's core; they are a fair end-to-end product comparison, not a
# measurement of Oxterm's own code alone.
#
# Requirements: X11 (DISPLAY set) and xdotool for the window metrics. Without
# a display the script aborts; the GUI numbers need a real or virtual X server
# (CI uses xvfb-run). Wayland-only sessions are not supported yet.
#
# Environment:
#   OXTERM_BIN          binary to measure (default: target/release/oxterm)
#   GHOSTTY_BIN         opponent (default: ghostty from PATH)
#   BENCH_RUNS          repetitions (default: 3)
#   BENCH_TOLERANCE     allowed multiplier over the baseline (default: 4)
#   BENCH_BASELINE      baseline file (default: bench/compare-baseline.txt)
#   BENCH_PAYLOAD_LINES lines in the throughput payload (default: 40000)
#   BENCH_GHOSTTY_ARGS  extra arguments for Ghostty (word-split)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OXTERM_BIN="${OXTERM_BIN:-${ROOT_DIR}/target/release/oxterm}"
GHOSTTY_BIN="${GHOSTTY_BIN:-$(command -v ghostty || true)}"
BASELINE="${BENCH_BASELINE:-${ROOT_DIR}/bench/compare-baseline.txt}"
RUNS="${BENCH_RUNS:-3}"
TOLERANCE="${BENCH_TOLERANCE:-4}"
PAYLOAD_LINES="${BENCH_PAYLOAD_LINES:-40000}"
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
ghostty_supports_class() {
    if [[ -z "${GHOSTTY_CLASS_OK}" ]]; then
        if [[ -n "${GHOSTTY_BIN}" ]] \
            && "${GHOSTTY_BIN}" --help 2>&1 | grep -q -- '--class'; then
            GHOSTTY_CLASS_OK=1
        else
            GHOSTTY_CLASS_OK=0
        fi
    fi
    [[ "${GHOSTTY_CLASS_OK}" == 1 ]]
}

declare -a GHOSTTY_EXTRA=()
if [[ -n "${BENCH_GHOSTTY_ARGS:-}" ]]; then
    # shellcheck disable=SC2206
    GHOSTTY_EXTRA=(${BENCH_GHOSTTY_ARGS})
fi

# launch_term TERM CLASS HOME RUNTIME [ARGS...]
# Sets APP_PID. The window class is unique per run so a leftover window from a
# previous iteration can never be mistaken for the fresh one.
launch_term() {
    local term="$1" class="$2" home="$3" runtime="$4"
    shift 4
    local -a cmd
    mkdir -p "${home}/.config" "${runtime}"
    chmod 700 "${runtime}"
    case "${term}" in
        oxterm)
            cmd=("${OXTERM_BIN}" --no-restore --class "${class}")
            ;;
        ghostty)
            cmd=("${GHOSTTY_BIN}" --gtk-single-instance=false)
            if ghostty_supports_class; then
                cmd+=("--class=${class}")
            fi
            cmd+=("${GHOSTTY_EXTRA[@]}")
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

search_class() {
    local term="$1" class="$2"
    if [[ "${term}" == ghostty ]] && ! ghostty_supports_class; then
        printf '%s' "com.mitchellh.ghostty"
    else
        printf '%s' "${class}"
    fi
}

wait_for_window() {
    local class="$1" limit
    limit="$(deadline_ns "${2}")"
    while (( $(now_ns) < limit )); do
        if xdotool search --onlyvisible --class "${class}" >/dev/null 2>&1; then
            return 0
        fi
        # A crash must not look like a slow start.
        if [[ -n "${APP_PID}" ]] && ! kill -0 "${APP_PID}" 2>/dev/null; then
            return 1
        fi
        sleep 0.01
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

# --- measurements ----------------------------------------------------------

# measure_startup TERM -> "startup_ms idle_rss_kb"
measure_startup() {
    local term="$1" class home runtime start end ms rss
    CLASS_SEQ=$(( CLASS_SEQ + 1 ))
    class="oxterm-bench-$$-${CLASS_SEQ}"
    home="${TMP_DIR}/${term}-home-${CLASS_SEQ}"
    runtime="${TMP_DIR}/${term}-rt-${CLASS_SEQ}"

    start="$(now_ns)"
    launch_term "${term}" "${class}" "${home}" "${runtime}"
    if ! wait_for_window "$(search_class "${term}" "${class}")" "${STARTUP_TIMEOUT}"; then
        stop_term
        report_launch_failure "${term}"
        return 1
    fi
    end="$(now_ns)"
    ms=$(( (end - start) / 1000000 ))
    sleep 0.05
    rss="$(tree_sum_kb "${APP_PID}" VmRSS)"
    stop_term
    printf '%s %s\n' "${ms}" "${rss}"
}

# measure_payload TERM PAYLOAD -> "exec_ms drain_ms peak_rss_kb"
measure_payload() {
    local term="$1" payload="$2" class home runtime started done script
    local t0 t_started t_done exec_ms drain_ms peak
    CLASS_SEQ=$(( CLASS_SEQ + 1 ))
    class="oxterm-bench-$$-${CLASS_SEQ}"
    home="${TMP_DIR}/${term}-home-${CLASS_SEQ}"
    runtime="${TMP_DIR}/${term}-rt-${CLASS_SEQ}"
    started="${home}/started"
    done="${home}/done"
    mkdir -p "${home}"
    rm -f "${started}" "${done}"
    script="touch '${started}'; cat '${payload}'; touch '${done}'; sleep 1"

    t0="$(now_ns)"
    launch_term "${term}" "${class}" "${home}" "${runtime}" -e sh -c "${script}"
    if ! wait_for_file "${started}" "${STARTUP_TIMEOUT}"; then
        stop_term
        report_launch_failure "${term}"
        return 1
    fi
    t_started="$(now_ns)"
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
    stop_term

    exec_ms=$(( (t_started - t0) / 1000000 ))
    drain_ms=$(( (t_done - t_started) / 1000000 ))
    printf '%s %s %s\n' "${exec_ms}" "${drain_ms}" "${peak}"
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

# --- main ------------------------------------------------------------------

if [[ ! -x "${OXTERM_BIN}" ]]; then
    printf '%s\n' "bench-compare: ${OXTERM_BIN} is missing; run 'cargo build --release' first" >&2
    exit 1
fi

TERMS=(oxterm)
if [[ -n "${GHOSTTY_BIN}" && -x "${GHOSTTY_BIN}" ]]; then
    TERMS+=(ghostty)
else
    printf '%s\n' "bench-compare: Ghostty not found; measuring Oxterm alone." >&2
    printf '%s\n' "               Install it with 'sudo pacman -S ghostty' or set GHOSTTY_BIN." >&2
fi

PAYLOAD="${TMP_DIR}/payload.txt"
write_payload "${PAYLOAD}"
PAYLOAD_BYTES="$(stat -c %s "${PAYLOAD}")"

declare -A RESULT=()
have_failure=0

for term in "${TERMS[@]}"; do
    startup_best=""
    idle_best=""
    exec_best=""
    drain_best=""
    peak_best=""
    for _ in $(seq "${RUNS}"); do
        if out="$(measure_startup "${term}")"; then
            read -r m r <<<"${out}"
            if [[ -z "${startup_best}" || "${m}" -lt "${startup_best}" ]]; then
                startup_best="${m}"
            fi
            if [[ -z "${idle_best}" || "${r}" -lt "${idle_best}" ]]; then
                idle_best="${r}"
            fi
        else
            have_failure=1
        fi
        if out="$(measure_payload "${term}" "${PAYLOAD}")"; then
            read -r e d p <<<"${out}"
            if [[ -z "${exec_best}" || "${e}" -lt "${exec_best}" ]]; then
                exec_best="${e}"
            fi
            if [[ -z "${drain_best}" || "${d}" -lt "${drain_best}" ]]; then
                drain_best="${d}"
            fi
            if [[ -z "${peak_best}" || "${p}" -lt "${peak_best}" ]]; then
                peak_best="${p}"
            fi
        else
            have_failure=1
        fi
    done
    RESULT["${term}.startup_ms"]="${startup_best}"
    RESULT["${term}.idle_rss_kb"]="${idle_best}"
    RESULT["${term}.exec_ms"]="${exec_best}"
    RESULT["${term}.drain_ms"]="${drain_best}"
    RESULT["${term}.peak_rss_kb"]="${peak_best}"
done

printf '%s\n' "bench-compare: payload ${PAYLOAD_BYTES} bytes, ${PAYLOAD_LINES} lines" \
    "(${RUNS} runs, minimum reported)" >&2

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
    format_row "Spawn to command (ms)" ".exec_ms"
    format_row "Drain ${PAYLOAD_LINES} lines (ms)" ".drain_ms"
    format_row "Idle RSS (MiB)" ".idle_rss_kb" mib
    format_row "Peak RSS under load (MiB)" ".peak_rss_kb" mib
}

format_row() {
    local label="$1" suffix="$2" unit="${3:-}" term value cell
    printf '| %s |' "${label}"
    for term in "${TERMS[@]}"; do
        value="${RESULT[${term}${suffix}]:-}"
        if [[ -z "${value}" ]]; then
            cell="n/a"
        elif [[ "${unit}" == mib ]]; then
            cell="$(awk -v kb="${value}" 'BEGIN { printf "%.1f", kb / 1024 }')"
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
        for suffix in startup_ms idle_rss_kb exec_ms drain_ms peak_rss_kb; do
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
            printf '# Oxterm and Ghostty are measured on the same display with an\n'
            printf '# isolated HOME; see the header of the script for the caveats.\n'
            emit_results
        } >"${BASELINE}"
        emit_results
        printf '%s\n' "bench-compare: wrote ${BASELINE}" >&2
        ;;
    --check)
        failed=0
        for term in "${TERMS[@]}"; do
            for suffix in startup_ms exec_ms drain_ms idle_rss_kb peak_rss_kb; do
                key="${term}.${suffix}"
                measured="${RESULT[${key}]:-}"
                base="$(baseline_value "${key}")"
                if [[ -z "${base}" || -z "${measured}" ]]; then
                    printf 'skip %s (no baseline or not measured)\n' "${key}"
                    continue
                fi
                if [[ "${suffix}" == *rss* ]]; then
                    limit=$(( base * 3 + 65536 ))
                else
                    limit=$(( base * TOLERANCE + 50 ))
                fi
                if (( measured > limit )); then
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
