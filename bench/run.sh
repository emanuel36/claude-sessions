#!/bin/sh
# Performance check of the release `cst` against the Goals of
# docs/superpowers/specs/2026-11-08-rust-rewrite-design.md, by the 2026-11-08 method:
#
# - hook and status line: hyperfine (-N, 3 interleaved rounds of 200 runs, 30 warm-ups), each
#   round timing `/bin/true` right before `cst` through the same `env -i` wrapper; a result is
#   the median over the rounds of (cst - floor), so a busy machine shifts both alike;
#   both run from a copy of the binary on tmpfs, so the time is the program's and not how the
#   disk's page cache treats a freshly built file (see bench/README.md); a `note:` line gives
#   the hook run from the build directory too, `WARN:` when it is over the target;
# - peak RSS of both, with GNU time;
# - the real `cst sidebar` in a private tmux server, 10 records with 4 working, sampled from
#   /proc while shown, then while hidden (F11). A hook event every 5 s keeps the records and a
#   transcript changing, as a working session would.
#
# Every result line ends in PASS or FAIL; the script exits 1 when one fails. MB is 1024 KiB.
# Environment: HYPERFINE (default `hyperfine`), CST_BENCH_SECONDS (sample length, default 60).
# Never touches `tmux -L claude`: the server is `-L cst-bench-<pid>`, killed at the end.
set -eu

HYPERFINE=${HYPERFINE:-hyperfine}
SAMPLE=${CST_BENCH_SECONDS:-60}
ROUNDS=3
RUNS=200
WARMUP=30
# Targets, from the spec's Goals
HOT_MS=3
RSS_KIB=5120
SIDEBAR_CPU_PCT=1

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CST=$ROOT/target/release/cst

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "bench: $1 not found (see bench/README.md)" >&2
        exit 2
    }
}
need "$HYPERFINE"
need tmux
need cargo
need /usr/bin/time
TMUX_BIN=$(command -v tmux)
TICK=$(getconf CLK_TCK)

cargo build --release --quiet --manifest-path "$ROOT/Cargo.toml"

WORK=$(mktemp -d)
SOCKET=cst-bench-$$
FAST=
SERVER=
cleanup() {
    "$TMUX_BIN" -L "$SOCKET" kill-server 2>/dev/null || true
    # tmux leaves the socket file behind
    rm -rf "$WORK" ${FAST:+"$FAST"} ${SERVER:+"$SERVER"}
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# The copy the hot path is timed from, on the first tmpfs found
for dir in "${XDG_RUNTIME_DIR:-}" /dev/shm; do
    if [ -n "$dir" ] && [ -d "$dir" ] && [ "$(stat -f -c %T "$dir")" = tmpfs ]; then
        FAST=$(mktemp -d "$dir/cst-bench.XXXXXX")
        break
    fi
done
if [ -n "$FAST" ]; then
    cp "$CST" "$FAST/cst"
    HOT_CST=$FAST/cst
else
    echo "bench: no tmpfs found; timing the binary in place" >&2
    HOT_CST=$CST
fi

FAILED=0
# verdict OK TEXT...: prints TEXT ending in PASS when OK is 1, FAIL otherwise
verdict() {
    ok=$1
    shift
    if [ "$ok" = 1 ]; then
        printf '%s  PASS\n' "$*"
    else
        printf '%s  FAIL\n' "$*"
        FAILED=1
    fi
}
# le A B: whether A <= B, as numbers
le() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a <= b) }'; }
# median of the numbers on stdin
median() {
    sort -g | awk '{ v[NR] = $1 }
        END { print (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2 }'
}
# calc EXPR: evaluates an awk expression
calc() { awk "BEGIN { printf \"%.2f\", $1 }"; }

echo "cst performance check, $(date '+%Y-%m-%d %H:%M')"
echo "$("$CST" --version), $("$HYPERFINE" --version), $("$TMUX_BIN" -V), $(nproc) CPUs," \
    "load $(cut -d' ' -f1-3 /proc/loadavg)," \
    "governor $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo ?)"
echo

# --- Hook and status line -------------------------------------------------------------------
HOT=$WORK/hot
mkdir -p "$HOT/state" "$HOT/claude/projects/api" "$HOT/home"
mkdir -m 700 "$HOT/run"
: >"$HOT/claude/projects/api/bench.jsonl"
# The same wrapper for the floor and for cst; TMUX is unset, as are Claude Code's variables
ENV="env -i HOME=$HOT/home PATH=/usr/bin:/bin CST_RUNTIME_DIR=$HOT/run CST_STATE_DIR=$HOT/state"
ENV="$ENV CLAUDE_CONFIG_DIR=$HOT/claude"
event() {
    printf '{"session_id":"bench","hook_event_name":"%s","cwd":"/work/api",' "$1"
    printf '"transcript_path":"%s","tool_name":"Bash","tool_input":{"command":"ls"}}\n' \
        "$HOT/claude/projects/api/bench.jsonl"
}
event SessionStart >"$WORK/start.json"
event PreToolUse >"$WORK/hook.json"
cat >"$WORK/statusline.json" <<'EOF'
{"session_id": "bench", "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
 "cost": {"total_cost_usd": 1.5, "total_api_duration_ms": 60000, "total_lines_added": 3},
 "rate_limits": {"five_hour": {"used_percentage": 40, "resets_at": 4000000000}}}
EOF
# The session exists, as it does once Claude Code has started it
$ENV "$CST" hook <"$WORK/start.json"

# time CMD INPUT ROUND [ARG...]: one hyperfine round of the floor, `cst CMD` and any other
# commands ARG names, to CSV
time_round() {
    cmd=$1 input=$2 csv=$WORK/$1-$3.csv
    shift 3
    "$HYPERFINE" -N --style none --warmup "$WARMUP" --runs "$RUNS" --input "$input" \
        --export-csv "$csv" -n floor "$ENV /bin/true" -n cst "$ENV $HOT_CST $cmd" "$@" \
        >"$WORK/hyperfine.log" 2>&1 || {
        cat "$WORK/hyperfine.log" >&2
        exit 2
    }
}
round=1
while [ "$round" -le "$ROUNDS" ]; do
    time_round hook "$WORK/hook.json" "$round" -n in-place "$ENV $CST hook"
    time_round statusline "$WORK/statusline.json" "$round"
    round=$((round + 1))
done

# CSV columns: command,mean,stddev,median,user,system,min,max (seconds)
# column CSV NAME FIELD-EXPR: the value of an awk expression over a command's row, in ms
column() { awk -F, -v n="$2" "\$1 == n { printf \"%.4f\n\", ($3) * 1000 }" "$1"; }
# per_round CMD FIELD-EXPR [NAME]: NAME (default cst) minus floor, one line per round
per_round() {
    for csv in "$WORK/$1"-*.csv; do
        awk -v c="$(column "$csv" "${3:-cst}" "$2")" -v f="$(column "$csv" floor "$2")" \
            'BEGIN { printf "%.4f\n", c - f }'
    done
}
# faults BINARY CMD INPUT: major page faults of one run
faults() {
    /usr/bin/time -f %F -o "$WORK/faults" $ENV "$1" "$2" <"$3" >/dev/null
    cat "$WORK/faults"
}
# all_rounds CMD NAME FIELD-EXPR: the median over the rounds of one command's value
all_rounds() {
    for csv in "$WORK/$1"-*.csv; do column "$csv" "$2" "$3"; done | median
}
peak_rss() {
    max=0
    i=0
    while [ "$i" -lt 20 ]; do
        /usr/bin/time -f %M -o "$WORK/rss" $ENV "$HOT_CST" "$1" <"$2" >/dev/null
        rss=$(cat "$WORK/rss")
        [ "$rss" -gt "$max" ] && max=$rss
        i=$((i + 1))
    done
    echo "$max"
}
hot_path() {
    name=$1
    over=$(per_round "$name" '$4' | median)
    cst=$(all_rounds "$name" cst '$4')
    floor=$(all_rounds "$name" floor '$4')
    ok=0
    le "$over" "$HOT_MS" && ok=1
    verdict $ok "$name: median $(calc "$cst") ms, floor $(calc "$floor") ms," \
        "+$(calc "$over") ms median of per-round differences (target <= +$HOT_MS ms)"
    over=$(per_round "$name" '$5 + $6' | median)
    cst=$(all_rounds "$name" cst '$5 + $6')
    floor=$(all_rounds "$name" floor '$5 + $6')
    ok=0
    le "$over" "$HOT_MS" && ok=1
    verdict $ok "$name: CPU $(calc "$cst") ms, floor $(calc "$floor") ms," \
        "+$(calc "$over") ms median of per-round differences (target <= +$HOT_MS ms)"
    rss=$(peak_rss "$name" "$2")
    ok=0
    [ "$rss" -lt "$RSS_KIB" ] && ok=1
    verdict $ok "$name: peak RSS $(calc "$rss / 1024") MB (target < 5 MB)"
}
hot_path hook "$WORK/hook.json"
hot_path statusline "$WORK/statusline.json"
# in_place_note OVER CPU FAULTS TMPFS_FAULTS: the hook timed from the build directory; WARN
# when it misses the target, which an installed copy on disk may miss too
in_place_note() {
    level=note
    le "$1" "$HOT_MS" || level=WARN
    echo "$level: hook run from the build directory instead:" \
        "+$(calc "$1") ms median of per-round differences, CPU +$(calc "$2") ms over the floor;" \
        "$3 major page faults a run, against $4 from tmpfs"
    if [ "$level" = WARN ]; then
        echo "WARN: that is above the +$HOT_MS ms target; an installed copy (~/.local/bin) may" \
            "pay it too"
    fi
}
if [ "$HOT_CST" != "$CST" ]; then
    in_place_note "$(per_round hook '$4' in-place | median)" \
        "$(per_round hook '$5 + $6' in-place | median)" \
        "$(faults "$CST" hook "$WORK/hook.json")" "$(faults "$HOT_CST" hook "$WORK/hook.json")"
fi

# --- Sidebar --------------------------------------------------------------------------------
SIDE=$WORK/side
mkdir -p "$SIDE/state" "$SIDE/claude" "$SIDE/home" "$SIDE/bin"
mkdir -m 700 "$SIDE/run"
# Every tmux client the sidebar starts goes through this wrapper, which counts it
CALLS=$SIDE/tmux-calls
: >"$CALLS"
cat >"$SIDE/bin/tmux" <<EOF
#!/bin/sh
echo "\$*" >>"$CALLS"
exec "$TMUX_BIN" "\$@"
EOF
chmod +x "$SIDE/bin/tmux"
T() { "$TMUX_BIN" -L "$SOCKET" "$@"; }
# A bare server (no conf, so no hooks run `cst`), started with the environment its panes get
env -i HOME="$SIDE/home" PATH="$SIDE/bin:/usr/bin:/bin" TERM=xterm-256color \
    LANG="${LANG:-C.UTF-8}" CST_RUNTIME_DIR="$SIDE/run" CST_STATE_DIR="$SIDE/state" \
    CLAUDE_CONFIG_DIR="$SIDE/claude" \
    "$TMUX_BIN" -L "$SOCKET" -f /dev/null new-session -d -s bench -n main -x 120 -y 40 sleep 600
SERVER=$(T display-message -p -t bench '#{socket_path}')

# start_of PID: the process start time, field 22 of /proc/PID/stat
start_of() { sed 's/.*) //' "/proc/$1/stat" | cut -d' ' -f20; }
now=$(date +%s)
i=1
while [ "$i" -le 10 ]; do
    # shellcheck disable=SC2046
    set -- $(T new-window -d -t bench: -n "project-$i" -P -F '#{pane_id} #{pane_pid}' sleep 600)
    pane=$1
    pid=$2
    pending='[]'
    case $i in
    1 | 2 | 3 | 4) status=working ;;
    5) status=permission pending='[{"agent":"main","kind":"permission"}]' ;;
    6) status=input pending='[{"agent":"main","kind":"input"}]' ;;
    7 | 8) status=done ;;
    *) status=idle ;;
    esac
    dir=$SIDE/claude/projects/project-$i
    mkdir -p "$dir"
    echo '{"type":"user","message":{"role":"user","content":"go"}}' >"$dir/s$i.jsonl"
    record=$SIDE/run/s$i.json
    printf '{"id":"s%s","status":"%s","cwd":"/work/project-%s","pid":%s,"pid_start":%s,' \
        "$i" "$status" "$i" "$pid" "$(start_of "$pid")" >"$record.tmp"
    printf '"transcript":"%s","background":0,"pending":%s,' "$dir/s$i.jsonl" "$pending" \
        >>"$record.tmp"
    printf '"status_since":%s,"updated_at":%s,"started_at":%s,' \
        "$((now - 60 * i))" "$now" "$((now - 600))" >>"$record.tmp"
    printf '"mux":{"socket":"%s","pane":"%s"},"headless":false,"title":"project %s",' \
        "$SERVER" "$pane" "$i" >>"$record.tmp"
    printf '"usage":{"cost":%s.5,"api_duration":%s0,"model":"Opus","added":1,"removed":0}}\n' \
        "$i" "$i" >>"$record.tmp"
    mv "$record.tmp" "$record"
    eval "PANE_$i=$pane PID_$i=$pid"
    i=$((i + 1))
done

# The sidebar of the window on screen, marked as the conf marks it
SIDEBAR=$(T split-window -h -b -l 29 -t bench:main -P -F '#{pane_id}' sleep 600)
T set-option -p -t "$SIDEBAR" @cs-sidebar 1
T respawn-pane -k -t "$SIDEBAR" "$CST" sidebar
PID=$(T display-message -p -t "$SIDEBAR" '#{pane_pid}')
tries=0
until T capture-pane -p -t "$SIDEBAR" | grep -q project-1; do
    tries=$((tries + 1))
    if [ "$tries" -gt 100 ]; then
        verdict 0 "sidebar: lists the 10 sessions"
        exit 1
    fi
    sleep 0.1
done
[ "$(cat "/proc/$PID/comm")" = cst ] || {
    echo "bench: pane process $PID is not cst" >&2
    exit 2
}

# A PreToolUse hook on the working sessions in turn, as Claude Code sends while working
HOOK_PATH=$(dirname "$TMUX_BIN"):/usr/bin:/bin
beat=0
activity() {
    k=$((beat % 4 + 1))
    beat=$((beat + 1))
    eval "pane=\$PANE_$k pid=\$PID_$k"
    dir=$SIDE/claude/projects/project-$k
    printf '{"session_id":"s%s","hook_event_name":"PreToolUse","cwd":"/work/project-%s",' \
        "$k" "$k" >"$WORK/beat.json"
    printf '"transcript_path":"%s","tool_name":"Bash"}\n' "$dir/s$k.jsonl" >>"$WORK/beat.json"
    env -i HOME="$SIDE/home" PATH="$HOOK_PATH" CST_RUNTIME_DIR="$SIDE/run" \
        CST_STATE_DIR="$SIDE/state" CLAUDE_CONFIG_DIR="$SIDE/claude" CLAUDE_PID="$pid" \
        TMUX="$SERVER,1,0" TMUX_PANE="$pane" "$CST" hook <"$WORK/beat.json"
    echo '{"type":"assistant","message":{"role":"assistant","content":[]}}' >>"$dir/s$k.jsonl"
}

# Fields 14 and 15 of /proc/PID/stat: user and system time, in clock ticks
ticks() { sed 's/.*) //' "/proc/$PID/stat" | awk '{ print $12 + $13 }'; }
# Voluntary and involuntary context switches of all its threads: how often it ran
wakeups() { cat /proc/"$PID"/task/*/status | awk '/ctxt_switches/ { n += $2 } END { print n }'; }
uptime_s() { cut -d' ' -f1 /proc/uptime; }
calls() { wc -l <"$CALLS"; }
# Live child processes of all its threads
children() { cat /proc/"$PID"/task/*/children | wc -w; }
smaps() { awk -v k="$1:" '$1 == k { print $2 }' "/proc/$PID/smaps_rollup"; }

# sample: SAMPLE seconds of activity; sets TICKS, SECS, WAKEUPS, CALLED, KIDS, RSS, PSS (KiB)
sample() {
    t0=$(ticks) w0=$(wakeups) c0=$(calls) u0=$(uptime_s)
    RSS=0 PSS=0 KIDS=0
    s=1
    while [ "$s" -le "$SAMPLE" ]; do
        sleep 1
        [ $((s % 5)) = 0 ] && activity
        r=$(smaps Rss) p=$(smaps Pss) k=$(children)
        [ "$r" -gt "$RSS" ] && RSS=$r
        [ "$p" -gt "$PSS" ] && PSS=$p
        [ "$k" -gt "$KIDS" ] && KIDS=$k
        s=$((s + 1))
    done
    TICKS=$(($(ticks) - t0))
    WAKEUPS=$(($(wakeups) - w0))
    CALLED=$(($(calls) - c0))
    SECS=$(calc "$(uptime_s) - $u0")
}

# Shown, past its start
sleep 3
sample
cpu=$(calc "$TICKS / $TICK / $SECS * 100")
ok=0
le "$cpu" "$SIDEBAR_CPU_PCT" && ok=1
verdict $ok "sidebar shown: CPU $cpu % of a core over $SECS s, $TICKS ticks" \
    "(target <= $SIDEBAR_CPU_PCT %)"
ok=0
[ "$RSS" -le "$RSS_KIB" ] && ok=1
verdict $ok "sidebar shown: RSS $(calc "$RSS / 1024") MB, PSS $(calc "$PSS / 1024") MB" \
    "(target <= 5 MB RSS)"
ok=0
[ "$CALLED" = 0 ] && [ "$KIDS" = 0 ] && ok=1
verdict $ok "sidebar shown: $CALLED tmux clients started, $KIDS seen running (target 0)"

# Hidden: tmux sends F11 when its window leaves the screen
T send-keys -t "$SIDEBAR" F11
sleep 2
sample
ok=0
[ "$TICKS" = 0 ] && [ "$CALLED" = 0 ] && [ "$KIDS" = 0 ] && ok=1
verdict $ok "sidebar hidden: $TICKS CPU ticks, $CALLED tmux clients, $KIDS children," \
    "$WAKEUPS wakeups over $SECS s (target 0 ticks, 0 clients)"

exit "$FAILED"
