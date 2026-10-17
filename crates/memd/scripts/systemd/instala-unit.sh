#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
uso: instala-unit.sh [--dry-run]
EOF
    exit 2
}

[[ -n "${HOME:-}" ]] || { printf 'instala-unit.sh: HOME nao definido\n' >&2; exit 2; }
dry_run=0
while (($#)); do
    case "$1" in
        --dry-run) dry_run=1; shift ;;
        *) usage ;;
    esac
done

config_home=${XDG_CONFIG_HOME:-$HOME/.config}
[[ "$config_home" == /* ]] || { printf 'instala-unit.sh: XDG_CONFIG_HOME deve ser absoluto\n' >&2; exit 2; }
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
source_unit="$script_dir/memd.service"
target_dir="$config_home/systemd/user"
target="$target_dir/memd.service"
[[ -f "$source_unit" ]] || { printf 'instala-unit.sh: unit de origem ausente: %s\n' "$source_unit" >&2; exit 2; }

if ((dry_run)); then
    printf 'Plano: copiar %s para %s e executar systemctl --user daemon-reload.\n' "$source_unit" "$target"
    printf 'dry-run: nenhuma alteracao feita; nao habilita nem inicia a unit.\n'
    exit 0
fi

install -d -m 0755 -- "$target_dir"
if [[ ! -f "$target" ]] || ! cmp -s -- "$source_unit" "$target"; then
    install -m 0644 -- "$source_unit" "$target"
    printf 'unit instalada: %s\n' "$target"
else
    printf 'unit ja atualizada: %s\n' "$target"
fi
systemctl --user daemon-reload
printf 'Unit recarregada; nao foi habilitada nem iniciada.\n'
printf 'Quando decidir: systemctl --user enable --now memd.service\n'
printf 'Depois verifique: systemctl --user status memd.service\n'
