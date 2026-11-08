#!/bin/sh
# Installs cst and registers it with Claude Code, in one step:
#   curl -fsSL https://raw.githubusercontent.com/emanuel36/claude-sessions/main/install.sh | sh
# Running it again updates to the latest release. CST_SOURCE=<checkout> builds that checkout
# instead of downloading; CST_RELEASE_BASE downloads from another release page.
set -eu

say() { printf '%s\n' "$*"; }
fail() {
    printf 'cst: %s\n' "$*" >&2
    exit 1
}

# curl with limits on connecting and on the whole transfer, so a stalled server cannot hang us
fetch() { curl --connect-timeout 10 --max-time 300 "$@"; }

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

# The release's name for this machine's architecture
arch() {
    case "$(uname -m)" in
        x86_64) echo x86_64 ;;
        aarch64 | arm64) echo aarch64 ;;
        *) fail "no release is built for $(uname -m); build one with CST_SOURCE=<checkout>" ;;
    esac
}

latest_tag() {
    case "$base" in
        file://*) fetch -fsSL "$base/latest/tag" ;;
        # The latest release redirects to .../releases/tag/<tag>
        *)
            fetch -fsSLI -o /dev/null -w '%{url_effective}' "$base/latest" \
                | sed -n 's|.*/tag/||p'
            ;;
    esac
}

# Downloads the latest release into $1/cst and checks its SHA-256
download() {
    command -v curl >/dev/null 2>&1 || fail "curl is needed; install it first"
    asset="cst-$(arch)-unknown-linux-musl"
    tag=$(latest_tag) || tag=""
    # The tag goes into a URL: not empty, no `/`, no whitespace
    case "$tag" in "" | */* | *[[:space:]]*) fail "no release found at $base" ;; esac
    say "Downloading cst $tag..."
    url="$base/download/$tag/$asset"
    fetch -fsSL "$url" -o "$1/$asset" || fail "could not download $url"
    fetch -fsSL "$url.sha256" -o "$1/$asset.sha256" || fail "could not download $url.sha256"
    # Compared directly: `sha256sum -c` checks whichever file the line names
    want=$(cut -d' ' -f1 <"$1/$asset.sha256")
    have=$(sha256sum "$1/$asset" | cut -d' ' -f1)
    case "$want" in
        *[!0-9a-f]* | "") want="" ;;
    esac
    [ ${#want} -eq 64 ] && [ "$want" = "$have" ] \
        || fail "$asset does not match its SHA-256; nothing was installed"
    mv "$1/$asset" "$1/cst"
}

# Builds the checkout at $CST_SOURCE into $1/cst
build() {
    command -v cargo >/dev/null 2>&1 || fail "cargo is needed to build $CST_SOURCE"
    say "Building cst from $CST_SOURCE..."
    cargo build --release --manifest-path "$CST_SOURCE/Cargo.toml"
    cp "$CST_SOURCE/target/release/cst" "$1/cst"
}

# uv records ~/.local/bin/cst as the Python version's, so uninstalling that later would delete
# this one: it goes first
remove_python_version() {
    # The uv installer puts uv in $bin; it may not be on PATH
    uv=$(PATH="$PATH:${UV_INSTALL_DIR:-$bin}" && command -v uv) || return 0
    "$uv" tool list 2>/dev/null | grep -q '^claude-sessions ' || return 0
    say "Removing the Python version: uv tool uninstall claude-sessions"
    "$uv" tool uninstall claude-sessions
}

main() {
    base="${CST_RELEASE_BASE:-https://github.com/emanuel36/claude-sessions/releases}"
    base="${base%/}"
    bin="${XDG_BIN_HOME:-$HOME/.local/bin}"

    [ "$(uname -s)" = Linux ] || fail "only Linux is supported"
    check_tmux

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM
    if [ -n "${CST_SOURCE:-}" ]; then
        build "$tmp"
    else
        download "$tmp"
    fi

    # Before anything is removed: stops with cst's own error if it could not register
    chmod 755 "$tmp/cst"
    "$tmp/cst" install --check || exit 1
    remove_python_version
    mkdir -p "$bin"
    # Written beside the old one, then renamed over it: a running cst keeps its own copy
    if ! { cp "$tmp/cst" "$bin/.cst.new" && chmod 755 "$bin/.cst.new" \
        && mv -f "$bin/.cst.new" "$bin/cst"; }; then
        rm -f "$bin/.cst.new"
        fail "could not install $bin/cst"
    fi

    "$bin/cst" install

    say ""
    case ":$PATH:" in
        *":$bin:"*) ;;
        *) say "$bin is not on your PATH: add it in your shell's startup file." ;;
    esac
    # A cst left open from before keeps running its old panes until `cst` runs again, which
    # brings them up to this version; ending the server is not ours to do
    if tmux -L claude has-session 2>/dev/null; then
        say "cst is open from before: run cst again to use this version;" \
            "your sessions keep running."
    fi
    say "Done. Open your sessions with: cst"
}

# Everything runs from here, so a download cut short runs nothing
main "$@"
