#!/usr/bin/env bash
# Instala o chatd como servico de usuario. NAO e executado pelo Claude: revise e rode voce.
# Faz: copia o binario release para ~/.local/bin, cria ~/.local/state/chatd (0700) com config e tokens
# (se nao existirem), instala a unit em ~/.config/systemd/user e habilita. Nao usa sudo.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../.." && pwd)
H=${CHATD_HOME:-$HOME/.local/state/chatd}
install -D -m 0755 "$REPO/target/release/chatd" "$HOME/.local/bin/chatd"
install -d -m 0700 "$H" "$H/state"
if [ ! -f "$H/config.json" ]; then
  HB=$(command -v herdr || true)
  cat > "$H/config.json" <<CFG
{"auth":"required","herdr_bin":"${HB:-herdr}","recipients":{"claude":{"delivery":"watch"},"codex":{"delivery":"bell","pane":"w2:p1"}}}
CFG
  chmod 0600 "$H/config.json"
fi
for n in claude codex; do
  [ -f "$H/$n.token" ] || "$HOME/.local/bin/chatd" token new "$n" --state-dir "$H/state" --out "$H/$n.token"
done
install -D -m 0644 "$REPO/scripts/systemd/chatd.service" "$HOME/.config/systemd/user/chatd.service"
systemctl --user daemon-reload
systemctl --user enable --now chatd.service
systemctl --user --no-pager status chatd.service | head -8
