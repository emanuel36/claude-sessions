#!/bin/sh
# Installs claude-sessions and registers it with Claude Code, in one step:
#   curl -fsSL https://raw.githubusercontent.com/emanuel36/claude-sessions/main/install.sh | sh
# Running it again updates to the latest version. CLAUDE_SESSIONS_SOURCE installs from another
# place (a fork, a local checkout).
set -eu

say() { printf '%s\n' "$*"; }
fail() {
    printf 'claude-sessions: %s\n' "$*" >&2
    exit 1
}

check_tmux() {
    command -v tmux >/dev/null 2>&1 || fail "tmux 3.3 or later is needed; install it first"
    # `tmux -V` prints e.g. "tmux 3.8", "tmux 3.3a", "tmux next-3.4" or, built from git,
    # "tmux master"
    version=$(tmux -V)
    case "$version" in *master*) return ;; esac
    echo "$version" | sed -n 's/^tmux[^0-9]*\([0-9][0-9]*\)\.\([0-9][0-9]*\).*/\1 \2/p' \
        | awk '{ ok = $1 > 3 || ($1 == 3 && $2 >= 3) } END { exit !ok }' \
        || fail "tmux 3.3 or later is needed; this is $version"
}

find_uv() {
    # The uv installer puts uv here; it may not be on PATH yet
    PATH="${UV_INSTALL_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}:$PATH"
    command -v uv >/dev/null 2>&1 && return
    say "Installing uv, which installs and updates claude-sessions..."
    say "(its installer may add ~/.local/bin to your shell's startup files)"
    curl -LsSf https://astral.sh/uv/install.sh | sh
    command -v uv >/dev/null 2>&1 || fail "could not install uv; see https://docs.astral.sh/uv/"
}

main() {
    source="${CLAUDE_SESSIONS_SOURCE:-git+https://github.com/emanuel36/claude-sessions}"

    [ "$(uname -s)" = Linux ] || fail "only Linux is supported"
    check_tmux
    command -v fzf >/dev/null 2>&1 || fail "fzf is needed (Alt+c uses it); install it first"
    command -v tig >/dev/null 2>&1 || say "Note: tig is not installed, so Alt+g is off."
    find_uv

    say "Installing claude-sessions..."
    uv tool install --force --python '>=3.11' "$source"
    bin=$(uv tool dir --bin)

    "$bin/cs" install

    say ""
    case ":$PATH:" in
        *":$bin:"*) ;;
        *) say "$bin is not on your PATH: run 'uv tool update-shell' and open a new terminal." ;;
    esac
    say "Done. Open your sessions with: cst"
}

# Everything runs from here, so a download cut short runs nothing
main "$@"
