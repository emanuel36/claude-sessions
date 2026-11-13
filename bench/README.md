# Performance check

`sh bench/run.sh` builds the release binary and checks it against the Goals of the design
(`docs/superpowers/specs/2026-11-08-rust-rewrite-design.md`): hook and status line time and CPU
over a `/bin/true` floor and their peak RSS, then the CPU, memory and tmux clients of a real
sidebar, shown and hidden, in a private tmux server. Each result line ends in `PASS` or `FAIL`,
and the script exits 1 when one fails; it takes about four minutes. It needs
[hyperfine](https://github.com/sharkdp/hyperfine) (or its path in `HYPERFINE`), tmux 3.2 or
newer, GNU time at `/usr/bin/time` and cargo; `CST_BENCH_SECONDS` shortens the 60 s sidebar
samples for a quick try. The hook and status line are timed from a copy of the binary on tmpfs
(`$XDG_RUNTIME_DIR` or `/dev/shm`): on a disk where something keeps evicting a freshly built
file from the page cache (an endpoint security agent, for one), each run of the build output
waits on a few dozen page faults that have nothing to do with the code. A `note:` line shows
the hook run from the build directory, with its page faults, for comparison; it reads `WARN:`
when that run is over the +3 ms target, which a copy installed on such a disk may be too. Each
"+" figure is the median, over the rounds, of the round's difference from the floor.
