# Oxterm Performance & Benchmarks

This document provides detailed performance measurements, profiling data, ablation studies, and architectural notes for **Oxterm**.

---

## 1. Overview & Performance Philosophy

Oxterm is designed around three performance pillars:
1. **Sub-millisecond headless core**: CLI parsing, configuration validation, frame serialization, and copy-mode motions are decoupled from GTK/VTE and operate in microseconds or nanoseconds.
2. **Display-free PTY Broker**: Decoupling the terminal session from the GTK process allows detached and background sessions to reside in **under 3 MB resident RAM**, surviving GUI restarts or crashes.
3. **Low-latency rendering & input loop**: VTE event dispatch is aligned with 60 Hz display refresh rates, and control socket communication adds less than 0.5 ms of overhead.

---

## 2. Head-to-Head Comparison

The benchmark script `scripts/bench-compare.sh` launches each terminal on the same display with an isolated `HOME` environment (using default configurations) and records the minimum across multiple runs.

### Test Environment
- **CPU**: AMD Ryzen 7 5800H (8 cores, 16 threads, Radeon Graphics)
- **OS**: CachyOS with Linux kernel 7.3.3-1-cachyos
- **Display**: X11 session at 1920x1080
- **CPU Governor**: powersave
- **Font Size**: 12
- **Build**: Release mode, warm disk cache

### Comparative Benchmark Table

| Measurement | Oxterm | Ghostty 1.4.0 | Alacritty 0.17.0 | xfce4-terminal 1.2.0 |
| :--- | :---: | :---: | :---: | :---: |
| **Spawn to first visible window** | 526 ms | 450 ms | **279 ms** | 533 ms |
| **Idle CPU over 5 s (no payload)** | 7.5 ms | 19.8 ms | **0.0 ms** | **0.0 ms** |
| **Spawn to the command inside terminal** | **102 ms** | 577 ms | 243 ms | 231 ms |
| **Drain 100 000 ANSI-coloured lines (8.8 MB)** | **89 ms** | 144 ms | 248 ms | 229 ms |
| **Drain 100 000 plain lines (8.0 MB)** | **81 ms** | 127 ms | 241 ms | 193 ms |
| **Drain plain payload in 80-byte writes (8.0 MB)** | 319 ms | **291 ms** | 295 ms | 376 ms |
| **CPU to drain ANSI (terminal only)** | 331.2 ms | 258.5 ms | 257.8 ms | **191.8 ms** |
| **CPU to drain ANSI (GUI process)** | 261.0 ms | 184.5 ms | 217.8 ms | **182.8 ms** |
| **CPU to drain plain (terminal only)** | 298.4 ms | 317.5 ms | 247.6 ms | **139.8 ms** |
| **CPU to drain 80-byte writes (terminal only)** | 421.2 ms | 380.0 ms | **263.6 ms** | 271.0 ms |
| **20 window resizes** | 138 ms | **82 ms** | 255 ms | 310 ms |
| **CPU for 20 resizes** | 31.8 ms | 24.5 ms | 20.5 ms | **12.9 ms** |
| **Close window to process exit** | 18 ms | 42 ms | 23 ms | **9 ms** |
| **Total tree RSS when idle** | 96.4 MiB | 201.1 MiB | 133.6 MiB | **78.3 MiB** |
| **Peak tree RSS under ANSI payload** | 99.6 MiB | 239.2 MiB | 148.3 MiB | **76.1 MiB** |

*Bold indicates the best value in each row.*

### Analysis of Metrics
- **CPU Time vs Wall Time**: Wall time in drain tests measures how quickly the terminal acknowledges PTY writes from the child process. CPU time (`schedstat`) represents actual compute cycles spent across all threads. In Oxterm, "terminal only" includes both the GTK GUI process and the background `oxterm-broker` PTY relay process.
- **Idle CPU**: Measures cursor blinking and timer wakeups over 5 seconds. Alacritty and xfce4-terminal disable cursor blinking by default (0.0 ms). Oxterm's blinking cursor and internal housekeeping timers consume ~7.5 ms. A hidden agent pane adds zero wakeups.
- **Drain Throughput**: Oxterm finishes both drain payloads faster than all peers (89 ms and 81 ms). Once the broker absorbs the data stream, the child command terminates without being blocked on synchronous display rendering.
- **Memory Footprint**: Total tree RSS accounts for the GUI process, the PTY broker, and the spawned shell. The actual memory attributable to Oxterm itself (PSS) is **18.7 MB** for an idle window and **21.2 MB** with 11 open tabs (~250 KB per additional tab).

---

## 3. PTY Broker Architecture & Cost Breakdown

Oxterm delegates PTY ownership to a separate helper executable, `oxterm-broker`.

### Why a Dedicated Broker Binary?
When a session is managed by the main GTK binary (`oxterm --broker`), Linux dynamic linkers map GTK3, VTE, Pango, and Cairo libraries into the process address space, costing ~20.4 MB RSS. 
`oxterm-broker` links only system libc and POSIX APIs, cutting resident memory to **2.7 MB** (with mapped file pages dropping from 17.8 MB to 2.6 MB).

### Profiling and Optimization Experiments
1. **Buffer Size Tuning (8 KiB vs 64 KiB)**: Testing 8 KiB vs 64 KiB PTY read buffers showed no significant change in throughput or CPU cycles (within ±1.7% run-to-run noise). The cost is dominated by kernel memory copy, not user-space syscall overhead.
2. **Idle Wakeup Reduction**: Lowering the broker's idle polling from 100 ms to 10 ms kept input round-trip latency at 16.3 ms, confirming that PTY polling was not the source of input latency.
3. **Signal-Driven Resizing**: The broker previously polled GUI PTY window size changes every 100 ms. This was replaced with a signal-driven architecture: the GUI sends `SIGWINCH`, and the signal handler writes to a self-pipe included in the broker's `poll` set. The broker now sleeps with zero timeout during idle periods, dropping idle CPU from 9.0 ms to ~5.0 ms over 5 seconds.

---

## 4. Input Latency

Measured using `scripts/bench-latency.py`, which types a unique sentinel into a running pane via the control socket and measures the elapsed time until the character is reflected in the terminal's screen model.

```bash
./scripts/bench-latency.py --table --runs 200
```

| Configuration Variant | ping p50 | capture p50 | echo p50 | echo p95 |
| :--- | :---: | :---: | :---: | :---: |
| **defaults** | 0.25 ms | 0.34 ms | 16.0 ms | 17.0 ms |
| **no-blink** | 0.25 ms | 0.36 ms | 16.0 ms | 17.1 ms |
| **transparency** | 0.25 ms | 0.29 ms | 16.1 ms | 17.1 ms |
| **no-shaping** | 0.24 ms | 0.34 ms | 16.1 ms | 17.1 ms |
| **fast** | 0.24 ms | 0.36 ms | 16.1 ms | 17.1 ms |

### Key Takeaway
Round-trip input latency is pegged to the 16.6 ms frame boundary (60 Hz VTE event loop cadence). Features like transparency, cursor blinking, HarfBuzz text shaping, and smooth scrolling add virtually zero measurable input delay. The control socket and broker together add less than 0.5 ms.

---

## 5. Micro-Optimizations in the Codebase

Several targeted algorithmic optimizations ensure minimal latency on critical paths:

1. **VI Copy Mode (`src/headless/vi.rs`)**:
   - Replaced `text.chars().collect::<Vec<char>>()` with iterator-based `char_indices()`.
   - Eight-hop word motion micro-benchmark dropped from **3.7 µs to 0.44 µs**.
2. **Theme Palette Lookup**:
   - Built-in palettes (such as `Dark (Default)` and `Light`) bypass JSON parsing and filesystem directory scans. First lookup dropped from **1.5 ms to 33 µs**; cached lookups take ~25 ns.
3. **Git Branch Column in Command History**:
   - Instead of spawning `git rev-parse --abbrev-ref HEAD` (costing ~2.9 ms per command), the history worker parses `.git/HEAD` directly in **1.2 µs**, with support for worktrees, submodules, and `$GIT_DIR`.
4. **History Worker Thread Consolidation**:
   - Eliminated the intermediate overflow thread; requests are queued directly via a bounded FIFO channel without blocking the GTK UI.
5. **Connection Manager Caching**:
   - Production host regexes are pre-compiled once (0.3 µs vs 191 µs per row).
   - Filtering in the connections dialog operates on in-memory models rather than re-reading configuration files from disk on every keystroke.

---

## 6. Running Benchmarks Locally

To reproduce or verify these benchmarks on your machine:

```bash
# Core headless benchmarks (CLI parse, settings validation, VI motions)
cargo run --release --example bench-core
cargo run --release --example bench-core -- --check

# GUI startup and memory benchmarks (requires an active X11/Wayland display)
./scripts/bench-startup.sh
./scripts/bench-startup.sh --check

# Comparative benchmarks vs other installed terminals
./scripts/bench-compare.sh --table

# Input latency round-trip measurements
./scripts/bench-latency.py --table
```
