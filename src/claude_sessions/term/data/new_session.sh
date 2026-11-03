#!/usr/bin/env bash
# Alt+c: pick a recent project folder (or type a path) and open Claude in a new window there.
set -uo pipefail
# --print-query prints the typed text first and the picked line second; fzf exits 1 when
# nothing matched (the typed path is the answer) and 130 on Esc
out=$(cs new --projects \
    | fzf --reverse --print-query --prompt 'New session in: ' \
          --preview 'cs new --preview {}' --preview-window 'right,45%' \
          --header 'Enter opens it. A path that is not listed works too. Esc cancels.')
case $? in
    0|1) ;;
    *) exit 0 ;;
esac
query=$(head -n 1 <<<"$out")
query="${query/#\~/$HOME}"
picked=$(sed -n 2p <<<"$out")
# A typed path that exists wins over whatever fuzzy match fzf highlighted
if [[ "$query" == /* && -d "$query" ]]; then
    dir="$query"
else
    dir="${picked/#\~/$HOME}"
    dir="${dir:-$query}"
fi
[ -n "$dir" ] || exit 0

if [ ! -d "$dir" ]; then
    tmux display-message "No such folder: $dir"
    exit 0
fi
tmux new-window -c "$dir" claude
