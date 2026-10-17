#!/usr/bin/env bash
# Gerencia os agentes (perfis) do memd: add, list, rotate, remove. Qualquer nome de agente serve
# (claude, codex, pi, hermes...). Cada agente ganha um token proprio e um wrapper memd-<nome>.
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
uso: agente.sh add NOME [--ops propose[,promote]] [--read ESCOPOS] [--write ESCOPOS] [--types decision,pending]
                        [--intent TEXTO] [--expira AAAA-MM-DD] [--no-reload]
     agente.sh list
     agente.sh rotate NOME [--no-reload]     (token novo; o antigo deixa de valer)
     agente.sh remove NOME [--no-reload]     (apaga perfil, token e wrapper)
opcoes comuns: [--bin CAMINHO_DO_MEMD] [--state-dir DIR]
ESCOPOS: lista separada por virgula; '*' = tudo, 'a/b' = exato, 'a/**' = a e tudo abaixo (padrao: '*').
EOF
    exit 2
}
fail() { printf 'agente.sh: %s\n' "$1" >&2; exit 2; }

[[ -n "${HOME:-}" ]] || fail 'HOME nao definido'
(($# >= 1)) || usage
cmd=$1; shift
name=''
case "$cmd" in
    add|rotate|remove) (($# >= 1)) || usage; name=$1; shift ;;
    list) ;;
    *) usage ;;
esac
[[ -z "$name" || "$name" =~ ^[a-z0-9_-]{1,32}$ ]] || fail 'NOME deve casar [a-z0-9_-]{1,32}'

ops=propose
read_scopes='*'
write_scopes='*'
types='decision,pending'
intent=''
expira=$(date -u -d '+90 days' +%F)
reload=1
memd_bin="$HOME/.local/bin/memd"
state_dir=${XDG_STATE_HOME:-"$HOME/.local/state"}/memd
while (($#)); do
    case "$1" in
        --ops) (($# >= 2)) || usage; ops=$2; shift 2 ;;
        --read) (($# >= 2)) || usage; read_scopes=$2; shift 2 ;;
        --write) (($# >= 2)) || usage; write_scopes=$2; shift 2 ;;
        --types) (($# >= 2)) || usage; types=$2; shift 2 ;;
        --intent) (($# >= 2)) || usage; intent=$2; shift 2 ;;
        --expira) (($# >= 2)) || usage; expira=$2; shift 2 ;;
        --bin) (($# >= 2)) || usage; memd_bin=$2; shift 2 ;;
        --state-dir) (($# >= 2)) || usage; state_dir=$2; shift 2 ;;
        --no-reload) reload=0; shift ;;
        *) usage ;;
    esac
done
[[ "$expira" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || fail '--expira deve ser AAAA-MM-DD'
[[ "$ops" =~ ^(propose|promote)(,(propose|promote))*$ ]] || fail '--ops: propose e/ou promote'
[[ -x "$memd_bin" ]] || fail "binario nao encontrado: $memd_bin"
grants="$state_dir/grants.json"
[[ -f "$grants" ]] || fail "estado nao preparado ($grants); rode scripts/prepara.sh primeiro"
command -v python3 >/dev/null || fail 'python3 ausente (usado so para editar o grants.json)'
bin_dir=$(dirname -- "$memd_bin")
token_file="$state_dir/tokens/$name.token"
wrapper="$bin_dir/memd-$name"
umask 077

recarrega() {
    ((reload)) || return 0
    if command -v systemctl >/dev/null 2>&1 && systemctl --user is-active --quiet memd.service 2>/dev/null; then
        systemctl --user reload memd.service && printf 'memd recarregado (SIGHUP): perfis valem agora\n'
    else
        printf 'memd.service nao esta ativo: os perfis valem na proxima partida\n'
    fi
}

edita() { # edita MODO ; usa NAME TOKEN_HASH OPS READ WRITE TYPES INTENT EXPIRA GRANTS no ambiente
    GRANTS="$grants" MODO="$1" python3 -I - <<'PY'
import json, os, sys, shutil, time
p = os.environ["GRANTS"]
data = json.load(open(p))
if not isinstance(data, list):
    sys.exit("grants.json: esperado lista na raiz")
name = os.environ["NAME"]
modo = os.environ["MODO"]
idx = [i for i, g in enumerate(data) if g.get("agent") == name]
shutil.copy2(p, p + ".bak." + time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()))
if modo == "add":
    if idx:
        sys.exit("agente ja existe: " + name + " (use rotate ou remove)")
    data.append({
        "agent": name,
        "token_sha256": os.environ["TOKEN_HASH"],
        "ops": os.environ["OPS"].split(","),
        "read_scopes": os.environ["READ"].split(","),
        "write_scopes": os.environ["WRITE"].split(","),
        "types": os.environ["TYPES"].split(","),
        "expires": os.environ["EXPIRA"],
        "intent": os.environ["INTENT"],
    })
elif modo == "rotate":
    if not idx:
        sys.exit("agente nao existe: " + name)
    data[idx[0]]["token_sha256"] = os.environ["TOKEN_HASH"]
elif modo == "remove":
    if not idx:
        sys.exit("agente nao existe: " + name)
    del data[idx[0]]
with open(p, "w") as f:
    json.dump(data, f, indent=2, ensure_ascii=False)
    f.write("\n")
os.chmod(p, 0o600)
PY
}

novo_token() { printf 'mt_%s' "$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n' | cut -c1-43)"; }

escreve_wrapper() {
    install -d -m 0755 -- "$bin_dir"
    cat > "$wrapper" <<EOF
#!/usr/bin/env bash
# Gerado por agente.sh: chama o memd como o agente "$name" (nao contem o token, so o caminho do arquivo).
exec "$memd_bin" "\$@" --token-file "$token_file" --socket "$state_dir/memd.sock"
EOF
    chmod 0755 -- "$wrapper"
}

case "$cmd" in
    list)
        GRANTS="$grants" python3 -I - <<'PY'
import json, os
for g in json.load(open(os.environ["GRANTS"])):
    print("%-16s ops=%-16s ler=%-12s escrever=%-12s expira=%s" % (g["agent"], ",".join(g["ops"]), ",".join(g["read_scopes"]), ",".join(g["write_scopes"]), g.get("expires", "-")))
PY
        ;;
    add)
        [[ -n "$intent" ]] || intent="Agente $name do time. Leia com view/query; proponha decisoes e pendencias com propose (sempre com source). Nunca grave segredos."
        install -d -m 0700 -- "$state_dir/tokens"
        [[ ! -e "$token_file" ]] || fail "token ja existe: $token_file (use rotate ou remove)"
        novo_token > "$token_file"; chmod 0600 -- "$token_file"
        h=$("$memd_bin" token-hash < "$token_file")
        if ! NAME="$name" TOKEN_HASH="$h" OPS="$ops" READ="$read_scopes" WRITE="$write_scopes" TYPES="$types" INTENT="$intent" EXPIRA="$expira" edita add; then
            rm -f -- "$token_file"; exit 2
        fi
        escreve_wrapper
        printf 'agente %s criado: token %s ; wrapper %s ; ops=%s expira %s\n' "$name" "$token_file" "$wrapper" "$ops" "$expira"
        recarrega
        ;;
    rotate)
        [[ -e "$token_file" ]] || fail "token nao existe: $token_file"
        novo_token > "$token_file.novo"; chmod 0600 -- "$token_file.novo"
        h=$("$memd_bin" token-hash < "$token_file.novo")
        if ! NAME="$name" TOKEN_HASH="$h" edita rotate; then rm -f -- "$token_file.novo"; exit 2; fi
        mv -f -- "$token_file.novo" "$token_file"
        printf 'token de %s girado\n' "$name"
        recarrega
        ;;
    remove)
        NAME="$name" edita remove
        rm -f -- "$token_file" "$wrapper"
        printf 'agente %s removido (perfil, token e wrapper)\n' "$name"
        recarrega
        ;;
esac
