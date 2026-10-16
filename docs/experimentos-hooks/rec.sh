#!/bin/bash
# Gravador de teste: guarda o evento, TMUX_PANE e o stdin do hook.
D="$(dirname "$0")/out"; ev="$1"; n=$(date +%s%N)
{ echo "evento=$ev tmux_pane=${TMUX_PANE:-vazio} cmd_env_TMUX=${TMUX:+definido}"; cat; echo; } > "$D/$n-$ev.txt"
exit 0
