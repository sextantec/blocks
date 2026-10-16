#!/usr/bin/env bash
# Teste ponta a ponta MANUAL do sink tmux do `chatd stale` contra um tmux REAL e ISOLADO (servidor e socket
# proprios, em diretorio temporario). Existe porque o tmux falso dos testes automaticos aceitou um argv que o
# tmux 3.7c recusa (`set-option -g @opt -- TEXTO`: "too many arguments"; o certo e `-g -- @opt TEXTO`).
# Uso: e2e_sink_tmux.sh [CHATD_BIN] [TMUX_BIN]. Nao toca o tmux do usuario nem o servico chatd. Precisa de `script`.
set -euo pipefail
CH=${1:-$(cd "$(dirname "$0")/../../.." && pwd)/target/debug/chatd}
TM=${2:-tmux}
[ -x "$CH" ] || { echo "chatd nao encontrado: $CH (rode cargo build -p chatd)" >&2; exit 2; }
T=$(mktemp -d); chmod 700 "$T"; SOCK=$T/lab.sock
cleanup() { "$TM" -S "$SOCK" kill-server 2>/dev/null || true; rm -rf "$T"; }; trap cleanup EXIT
mkdir -p "$T/state"
python3 -I - "$T/state/log.jsonl" <<'PY'
import json, sys, datetime
ts = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=329)).isoformat(timespec="milliseconds").replace("+00:00", "Z")
with open(sys.argv[1], "w") as f:
    for i in range(7):
        f.write(json.dumps({"v": 1, "seq": i + 1, "ts": ts, "type": "registered", "id": "e2e-%d" % i, "from": "outro", "to": "claude",
            "kind": "AVISO", "subject": "assunto #{x} #(id)", "body": "corpo", "thread": None, "state": "registered", "refs": [],
            "note": None, "registered_at": ts, "accepted_at": None, "resulted_at": None, "result": None}) + "\n")
PY
"$TM" -S "$SOCK" -f /dev/null new-session -d -s lab
( script -qfc "$TM -S $SOCK attach -t lab" /dev/null < <(sleep 20) >/dev/null 2>&1 & )
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -n "$("$TM" -S "$SOCK" list-clients -F '#{client_name}')" ] && break; sleep 0.5; done
fail=0
set +e
err=$("$CH" stale --state-dir "$T/state" --notify tmux --tmux-bin "$TM" --tmux-socket "$SOCK" --stamp-file "$T/stamp" 2>&1 >/dev/null); rc=$?
[ "$rc" = 6 ] || { echo "FALHA: 1a execucao saiu $rc (esperado 6)"; fail=1; }
case "$err" in *aviso*) echo "FALHA: aviso do sink: $err"; fail=1;; esac
v=$("$TM" -S "$SOCK" show -gv @chatd_stale)
[ "$v" = "claude 7p 329m" ] || { echo "FALHA: @chatd_stale = [$v]"; fail=1; }
err=$("$CH" stale --state-dir "$T/state" --notify tmux --tmux-bin "$TM" --tmux-socket "$SOCK" --stamp-file "$T/stamp" 2>&1 >/dev/null); rc=$?
[ "$rc" = 0 ] || { echo "FALHA: 2a execucao saiu $rc (esperado 0)"; fail=1; }
[ -z "$err" ] || { echo "FALHA: stderr da 2a execucao: $err"; fail=1; }
[ "$fail" = 0 ] && echo "e2e sink tmux: OK (tmux $("$TM" -V))"
exit $fail
