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

fail_precondition() {
    printf 'instala.sh: %s\n' "$1" >&2
    exit 2
}

dry_run=0
yes=0
no_restart=0
no_build=0
offline=0
allow_dirty=0
prefix="${HOME:?HOME nao definido}/.local/bin"
service='chatd.service'
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

[[ "$prefix" == /* ]] || fail_precondition '--prefix deve ser absoluto'
[[ -d "$prefix" ]] || fail_precondition "prefix inexistente: $prefix"
[[ "$service" != -* && "$service" != *$'\n'* && -n "$service" ]] || usage
[[ "$keep" =~ ^[0-9]+$ ]] || usage
[[ "$keep" -ge 1 ]] || usage
[[ "$wait_seconds" =~ ^[0-9]+$ ]] || usage
[[ "$wait_seconds" -ge 1 ]] || usage

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
if ! repo=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null); then
    fail_precondition 'raiz do repositorio nao encontrada'
fi
repo=$(cd -- "$repo" && pwd -P)
installed="$prefix/chatd"
[[ -f "$installed" && -x "$installed" ]] || fail_precondition "binario instalado ausente ou nao executavel: $installed"

if ((allow_dirty == 0)) && ! git -C "$repo" diff --quiet HEAD --; then
    fail_precondition 'ha arquivo rastreado modificado; use --allow-dirty para prosseguir'
fi

commit=$(git -C "$repo" describe --always --dirty 2>/dev/null) || fail_precondition 'nao determinei o commit'
installed_sha=$(sha256sum -- "$installed" | awk '{print $1}')
installed_size=$(stat -c '%s' -- "$installed")
if ((no_restart == 0)); then
    command -v systemctl >/dev/null 2>&1 || fail_precondition 'systemctl indisponivel'
    systemctl --user cat "$service" >/dev/null 2>&1 || fail_precondition "servico inexistente: $service"
fi

daemon_home=${CHATD_HOME:-"$HOME/.local/state/chatd"}
config_file="$daemon_home/config.json"
service_running=0
current_pid=''
running_sha='-'
if ((no_restart == 0)) && systemctl --user is-active --quiet "$service"; then
    service_running=1
    current_pid=$(systemctl --user show --property=MainPID --value "$service" 2>/dev/null || true)
    if [[ "$current_pid" =~ ^[1-9][0-9]*$ && -e "/proc/$current_pid/exe" ]]; then
        running_sha=$(sha256sum -- "/proc/$current_pid/exe" 2>/dev/null | awk '{print $1}') || running_sha='-'
    fi
fi

printf 'commit: %s\n' "$commit"
printf 'instalado: sha256=%s tamanho=%s caminho=%s\n' "$installed_sha" "$installed_size" "$installed"
if ((service_running)); then
    printf 'processo: MainPID=%s sha256=%s\n' "$current_pid" "$running_sha"
else
    printf 'processo: inativo ou nao verificado\n'
fi

if ((no_build)); then
    new_bin="$repo/target/release/chatd"
    [[ -f "$new_bin" && -x "$new_bin" ]] || fail_precondition '--no-build exige target/release/chatd executavel'
else
    new_bin="$repo/target/release/chatd"
fi

printf '\nPlano:\n'
if ((no_build)); then
    printf '  usar o binario existente %s\n' "$new_bin"
else
    if ((offline)); then
        printf '  compilar: cargo build --release -p chatd --offline\n'
    else
        printf '  compilar: cargo build --release -p chatd\n'
    fi
fi
printf '  testar uso e inicializacao temporaria usando uma copia de %s\n' "$config_file"
printf '  criar backup em %s/chatd.<UTC>.bak e trocar %s atomicamente\n' "$prefix" "$installed"
if ((no_restart)); then
    printf '  nao reiniciar %s; sair com codigo 3\n' "$service"
else
    printf '  reiniciar %s e validar processo e log por ate %s s\n' "$service" "$wait_seconds"
fi
printf '  manter os %s backups mais recentes\n' "$keep"

if ((dry_run)); then
    printf 'dry-run: nenhuma alteracao feita\n'
    exit 0
fi

if ((yes == 0)); then
    if ! read -r -p 'Continuar? [s/N] ' answer; then
        fail_precondition 'cancelado'
    fi
    [[ "$answer" == 's' || "$answer" == 'S' ]] || fail_precondition 'cancelado'
fi

if ((no_build == 0)); then
    cargo_args=(build --release -p chatd)
    ((offline)) && cargo_args+=(--offline)
    (cd -- "$repo" && cargo "${cargo_args[@]}") || fail_precondition 'cargo build falhou'
fi
new_sha=$(sha256sum -- "$new_bin" | awk '{print $1}')
new_size=$(stat -c '%s' -- "$new_bin")
printf 'novo: sha256=%s tamanho=%s\n' "$new_sha" "$new_size"

tmp=$(mktemp -d /tmp/cd.XXXXXX) || fail_precondition 'nao criei diretorio temporario'
smoke_pid=''
cleanup() {
    if [[ -n "$smoke_pid" ]] && kill -0 "$smoke_pid" 2>/dev/null; then
        kill "$smoke_pid" 2>/dev/null || true
        wait "$smoke_pid" 2>/dev/null || true
    fi
    rm -rf -- "$tmp"
}
trap cleanup EXIT

usage_output="$tmp/usage.out"
"$new_bin" >"$usage_output" 2>&1 || true
[[ -s "$usage_output" ]] || fail_precondition 'binario novo nao imprimiu o uso'
[[ -f "$config_file" ]] || fail_precondition 'config.json do daemon ausente para o teste de fumaca'
install -m 0600 -- "$config_file" "$tmp/config.json" || fail_precondition 'nao copiei config.json para o teste de fumaca'
mkdir -- "$tmp/state"

"$new_bin" serve --state-dir "$tmp/state" --config "$tmp/config.json" --socket "$tmp/s.sock" >"$tmp/smoke.log" 2>&1 &
smoke_pid=$!
ready=0
for ((tick = 0; tick < 50; tick++)); do
    if grep -Fq 'chatd: ready' "$tmp/smoke.log"; then
        ready=1
        break
    fi
    if ! kill -0 "$smoke_pid" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if ((ready == 0)); then
    fail_precondition 'teste de fumaca nao iniciou em ate 5 s'
fi
kill "$smoke_pid" 2>/dev/null || true
wait "$smoke_pid" 2>/dev/null || true
smoke_pid=''

backup=''
rollback_source=''
if [[ "$new_sha" == "$installed_sha" ]]; then
    if ((service_running)) && [[ "$running_sha" != "$installed_sha" ]]; then
        if ((no_restart)); then
            printf 'ja instalado; processo divergente nao reiniciado por --no-restart\n'
            exit 3
        fi
        if [[ "$running_sha" != '-' && -r "/proc/$current_pid/exe" ]]; then
            rollback_source="$tmp/rollback-chatd"
            install -m 0755 -- "/proc/$current_pid/exe" "$rollback_source" ||
                fail_precondition 'nao preservei o binario do processo para uma eventual reversao'
        fi
        printf 'ja instalado; processo em execucao divergente, reiniciando o servico\n'
    else
        printf 'ja instalado\n'
        exit 0
    fi
else
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    backup="$prefix/chatd.$stamp.bak"
    [[ ! -e "$backup" ]] || fail_precondition "backup ja existe: $backup"
    new_temp="$prefix/chatd.new.$$"
    [[ ! -e "$new_temp" ]] || fail_precondition "temporario ja existe: $new_temp"
    if ! install -m 0755 -- "$installed" "$backup"; then
        rm -f -- "$backup"
        fail_precondition 'nao criei backup do binario atual'
    fi
    if ! install -m 0755 -- "$new_bin" "$new_temp"; then
        rm -f -- "$new_temp"
        mv -f -- "$backup" "$installed" || true
        fail_precondition 'nao preparei o binario novo'
    fi
    if ! mv -f -- "$new_temp" "$installed"; then
        rm -f -- "$new_temp"
        mv -f -- "$backup" "$installed" || true
        fail_precondition 'nao troquei o binario atomicamente'
    fi
fi

if ((no_restart)); then
    if [[ -n "$backup" ]]; then
        mapfile -t old_backups < <(find "$prefix" -maxdepth 1 -type f -name 'chatd.*.bak' -printf '%f\n' | sort -r | tail -n "+$((keep + 1))")
        for old_backup in "${old_backups[@]}"; do
            [[ "$old_backup" == chatd.*.bak ]] && rm -f -- "$prefix/$old_backup"
        done
    fi
    printf 'instalado sem reinicio; commit=%s sha256=%s backup=%s estado=nao reiniciado\n' "$commit" "$new_sha" "${backup:--}"
    if [[ -n "$backup" ]]; then
        printf 'reversao manual: mv -f %s %s && systemctl --user restart %s\n' "$backup" "$installed" "$service"
    fi
    exit 3
fi

rollback() {
    local reason=$1
    printf 'instala.sh: %s; revertendo\n' "$reason" >&2
    systemctl --user status "$service" -n 40 --no-pager >&2 || true
    if [[ -n "$backup" && -f "$backup" ]]; then
        mv -f -- "$backup" "$installed" || printf 'instala.sh: falha ao restaurar backup %s\n' "$backup" >&2
    elif [[ -n "$rollback_source" && -f "$rollback_source" ]]; then
        local restore_temp="$prefix/chatd.rollback.$$"
        if install -m 0755 -- "$rollback_source" "$restore_temp" &&
            mv -f -- "$restore_temp" "$installed"; then
            :
        else
            rm -f -- "$restore_temp"
            printf 'instala.sh: falha ao restaurar a imagem anterior do processo\n' >&2
        fi
    fi
    systemctl --user restart "$service" >/dev/null 2>&1 || true
    printf 'diagnostico: journalctl --user-unit %s -n 30 --no-pager\n' "$service" >&2
    exit 1
}

if ! systemctl --user restart "$service"; then
    rollback 'restart do servico falhou'
fi
validated_pid=''
for ((tick = 0; tick <= wait_seconds * 10; tick++)); do
    active_now=0
    systemctl --user is-active --quiet "$service" && active_now=1
    candidate_pid=$(systemctl --user show --property=MainPID --value "$service" 2>/dev/null || true)
    if ((active_now)) && [[ "$candidate_pid" =~ ^[1-9][0-9]*$ && -r "/proc/$candidate_pid/exe" ]]; then
        candidate_sha=$(sha256sum -- "/proc/$candidate_pid/exe" 2>/dev/null | awk '{print $1}') || candidate_sha=''
        if [[ "$candidate_sha" == "$new_sha" ]] && \
            env -u CHATD_CONFIG XDG_CONFIG_HOME="$tmp/no-xdg" "$installed" log \
                --state-dir "$daemon_home/state" --tail 1 >/dev/null 2>&1; then
            validated_pid=$candidate_pid
            break
        fi
    fi
    if ((tick < wait_seconds * 10)); then
        sleep 0.1
    fi
done
[[ -n "$validated_pid" ]] || rollback "servico nao ficou pronto em ${wait_seconds}s"

sleep 1
if ! systemctl --user is-active --quiet "$service"; then
    rollback 'servico perdeu active durante a verificacao de estabilidade'
fi
stable_pid=$(systemctl --user show --property=MainPID --value "$service" 2>/dev/null || true)
[[ "$stable_pid" == "$validated_pid" ]] || rollback 'MainPID mudou durante a verificacao de estabilidade'

if [[ -n "$backup" ]]; then
    mapfile -t old_backups < <(find "$prefix" -maxdepth 1 -type f -name 'chatd.*.bak' -printf '%f\n' | sort -r | tail -n "+$((keep + 1))")
    for old_backup in "${old_backups[@]}"; do
        [[ "$old_backup" == chatd.*.bak ]] && rm -f -- "$prefix/$old_backup"
    done
fi

printf 'instalacao concluida: commit=%s\nsha256 antes=%s\nsha256 depois=%s\nbackup=%s\nservico=%s active\n' \
    "$commit" "$installed_sha" "$new_sha" "${backup:--}" "$service"
if [[ -n "$backup" ]]; then
    printf 'reversao manual: mv -f %s %s && systemctl --user restart %s\n' "$backup" "$installed" "$service"
else
    printf 'reversao manual: nenhuma; binario ja instalado, servico reiniciado\n'
fi
printf 'lembrete: nao cria token ou client.json nem altera a configuracao\n'
exit 0
