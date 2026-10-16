#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
uso: instala.sh [--dry-run] [--yes] [--prefix DIR] [--service NOME]
                [--no-restart] [--no-build] [--offline] [--allow-dirty]
                [--keep N] [--wait S]
EOF
    exit 2
}

fail() {
    printf 'instala.sh: %s\n' "$1" >&2
    exit 2
}

[[ -n "${HOME:-}" ]] || fail 'HOME nao definido'

dry_run=0
yes=0
no_restart=0
no_build=0
offline=0
allow_dirty=0
prefix="$HOME/.local/bin"
service='agentd.service'
keep=5
wait_seconds=15

while (($#)); do
    case "$1" in
        --dry-run) dry_run=1; shift ;;
        --yes) yes=1; shift ;;
        --no-restart) no_restart=1; shift ;;
        --no-build) no_build=1; shift ;;
        --offline) offline=1; shift ;;
        --allow-dirty) allow_dirty=1; shift ;;
        --prefix)
            (($# >= 2)) || usage
            prefix=$2
            shift 2
            ;;
        --service)
            (($# >= 2)) || usage
            service=$2
            shift 2
            ;;
        --keep)
            (($# >= 2)) || usage
            keep=$2
            shift 2
            ;;
        --wait)
            (($# >= 2)) || usage
            wait_seconds=$2
            shift 2
            ;;
        *) usage ;;
    esac
done

[[ "$prefix" == /* ]] || fail '--prefix deve ser absoluto'
[[ "$service" != -* && "$service" != *$'\n'* && -n "$service" ]] || usage
[[ "$keep" =~ ^[0-9]+$ && "$keep" -ge 1 ]] || usage
[[ "$wait_seconds" =~ ^[0-9]+$ && "$wait_seconds" -ge 1 ]] || usage

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
if ! repo=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null); then
    fail 'raiz do repositorio nao encontrada'
fi
repo=$(cd -- "$repo" && pwd -P)
installed="$prefix/agentd"
new_bin="$repo/target/release/agentd"

if ((allow_dirty == 0)) && ! git -C "$repo" diff --quiet HEAD --; then
    fail 'ha arquivo rastreado modificado; use --allow-dirty para prosseguir'
fi

if ((no_build)); then
    [[ -f "$new_bin" && -x "$new_bin" ]] || fail '--no-build exige target/release/agentd executavel'
    build_text='usar o binario release existente'
elif ((offline)); then
    build_text='cargo build --release -p agentd --offline'
else
    build_text='cargo build --release -p agentd'
fi

if ((dry_run)); then
    printf 'Plano:\n  %s\n' "$build_text"
    printf '  instalar atomicamente %s em %s (backup datado se existir)\n' "$new_bin" "$installed"
    if ((no_restart)); then
        printf '  nao consultar nem reiniciar %s (--no-restart)\n' "$service"
    else
        printf '  em execucao real: reiniciar %s somente se existir e estiver ativa; validar agentd health\n' "$service"
    fi
    printf 'dry-run: nenhuma alteracao feita\n'
    exit 0
fi

if ((yes == 0)); then
    if ! read -r -p 'Continuar? [s/N] ' answer; then
        fail 'cancelado'
    fi
    [[ "$answer" == 's' || "$answer" == 'S' ]] || fail 'cancelado'
fi

if ((no_build == 0)); then
    cargo_args=(build --release -p agentd)
    ((offline)) && cargo_args+=(--offline)
    (cd -- "$repo" && cargo "${cargo_args[@]}") || fail 'cargo build falhou'
fi
[[ -f "$new_bin" && -x "$new_bin" ]] || fail 'binario release ausente ou nao executavel'

service_known=0
service_active=0
if ((no_restart == 0)) && command -v systemctl >/dev/null 2>&1; then
    if systemctl --user cat "$service" >/dev/null 2>&1; then
        service_known=1
        if systemctl --user is-active --quiet "$service"; then
            service_active=1
        fi
    fi
fi

if [[ -f "$installed" && -x "$installed" ]]; then
    installed_sha=$(sha256sum -- "$installed" | awk '{print $1}')
else
    installed_sha='-'
fi
new_sha=$(sha256sum -- "$new_bin" | awk '{print $1}')

install -d -m 0755 -- "$prefix" || fail "nao criei o diretorio de instalacao: $prefix"
backup=''
if [[ -f "$installed" ]]; then
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    backup="$prefix/agentd.$stamp.bak"
    [[ ! -e "$backup" ]] || fail "backup ja existe: $backup"
    install -m 0755 -- "$installed" "$backup" || fail 'nao criei backup do binario atual'
fi

new_temp="$prefix/agentd.new.$$"
[[ ! -e "$new_temp" ]] || fail "temporario ja existe: $new_temp"
if ! install -m 0755 -- "$new_bin" "$new_temp"; then
    rm -f -- "$new_temp"
    fail 'nao preparei o binario novo'
fi
if ! mv -f -- "$new_temp" "$installed"; then
    rm -f -- "$new_temp"
    if [[ -n "$backup" ]]; then mv -f -- "$backup" "$installed" || true; fi
    fail 'nao troquei o binario atomicamente'
fi

restore_previous() {
    if [[ -n "$backup" && -f "$backup" ]]; then
        mv -f -- "$backup" "$installed" || printf 'instala.sh: falha ao restaurar %s\n' "$backup" >&2
    else
        rm -f -- "$installed"
    fi
}

prune_backups() {
    local old_backup
    mapfile -t old_backups < <(find "$prefix" -maxdepth 1 -type f -name 'agentd.*.bak' -printf '%f\n' | sort -r | tail -n "+$((keep + 1))")
    for old_backup in "${old_backups[@]}"; do
        [[ "$old_backup" == agentd.*.bak ]] && rm -f -- "$prefix/$old_backup"
    done
}

if ((no_restart)); then
    prune_backups
    printf 'binario instalado sem reiniciar %s; sha256=%s backup=%s\n' "$service" "$new_sha" "${backup:--}"
    exit 0
fi

if ((service_known == 0)); then
    prune_backups
    printf 'binario instalado; %s nao foi encontrado (ou systemctl --user esta indisponivel).\n' "$service"
    printf 'Falta instalar a unit com scripts/systemd/instala-unit.sh e criar a configuracao conforme scripts/README-instalacao.md.\n'
    exit 0
fi

if ((service_active == 0)); then
    prune_backups
    printf 'binario instalado; %s existe mas esta inativa, portanto nao foi reiniciada.\n' "$service"
    printf 'A verificacao agentd health sera feita quando o servico estiver ativo.\n'
    exit 0
fi

rollback() {
    local reason=$1
    printf 'instala.sh: %s; revertendo o binario\n' "$reason" >&2
    restore_previous
    systemctl --user restart "$service" >/dev/null 2>&1 || true
    printf 'diagnostico: systemctl --user status %s --no-pager\n' "$service" >&2
    exit 1
}

systemctl --user restart "$service" || rollback 'restart do servico falhou'
runtime_dir=${XDG_RUNTIME_DIR:-"/run/user/$(id -u)"}
socket="$runtime_dir/agentd/agentd.sock"
healthy=0
for ((tick = 0; tick <= wait_seconds * 10; tick++)); do
    if "$installed" health --socket "$socket" >/dev/null 2>&1; then
        healthy=1
        break
    fi
    ((tick < wait_seconds * 10)) && sleep 0.1
done
((healthy)) || rollback "agentd health nao respondeu em ${wait_seconds}s no socket $socket"

prune_backups
printf 'instalacao concluida: sha256=%s\nbackup=%s\nservico=%s active; health=ok; socket=%s\n' \
    "$new_sha" "${backup:--}" "$service" "$socket"
if [[ "$prefix/agentd" != "$HOME/.local/bin/agentd" ]]; then
    printf 'aviso: ExecStart da unit usa %%h/.local/bin/agentd; ajuste a unit se --prefix nao for %s/.local/bin\n' "$HOME"
fi
