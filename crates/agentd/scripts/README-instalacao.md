# Instalação do agentd

Os scripts não criam configuração nem habilitam ou iniciam serviços. Revise os caminhos e faça essas etapas conscientemente.

## Binário e unit

Na raiz do repositório:

```sh
bash crates/agentd/scripts/instala.sh --offline
bash crates/agentd/scripts/systemd/instala-unit.sh
```

O instalador do binário compila `agentd` em release, copia para `~/.local/bin/agentd` e só reinicia `agentd.service` se a unit existir e estiver ativa. Nesse caso, verifica `agentd health` no socket da unit e restaura o binário anterior se a verificação falhar. Na primeira instalação, copia apenas o binário. `--dry-run` não escreve arquivos nem consulta o systemd; `--no-build`, `--no-restart`, `--prefix DIR`, `--offline`, `--allow-dirty`, `--keep N` e `--wait S` ajustam o fluxo.

O instalador da unit copia para `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/agentd.service` e executa `systemctl --user daemon-reload`; não habilita nem inicia. Após criar a configuração abaixo, a decisão de iniciar fica com você:

```sh
systemctl --user enable --now agentd.service
systemctl --user status agentd.service
```

## Configuração: dois painéis, sem hooks

Copie as regras incluídas no repositório e crie manualmente `~/.config/agentd/config.json`. Substitua `USUARIO`, `%12`, a tag `codex-main` e o caminho do arquivo de thread pelos valores da sua sessão. Os caminhos JSON devem ser absolutos; o `thread_file` deve conter o UUID da thread Codex que será observada.

```sh
install -Dm644 crates/agentd/rules/default.json \
  "${XDG_CONFIG_HOME:-$HOME/.config}/agentd/rules.json"
```

```json
{
  "rules_file": "/home/USUARIO/.config/agentd/rules.json",
  "agents": {
    "claude-main": {
      "kind": "claude",
      "sources": [{ "type": "tmux", "pane": "%12" }]
    },
    "codex-main": {
      "kind": "codex",
      "sources": [
        { "type": "tmux", "tag": "codex-main" },
        {
          "type": "codex_app_server",
          "thread_file": "/home/USUARIO/.codex/thread-file"
        }
      ]
    }
  }
}
```

O `thread_file` pode ser o mesmo arquivo que o `chatd` já usa (`~/.local/state/chatd/threads/NOME`, só o UUID da thread). Para consultar o daemon, exporte o socket da unit: `export AGENTD_SOCKET="$XDG_RUNTIME_DIR/agentd/agentd.sock"` e rode `agentd list` ou `agentd health`.

O socket Codex usa por padrão `~/.codex/app-server-control/app-server-control.sock`. A unit usa `%h/.config/agentd/config.json` e `%h/.local/bin/agentd`; se você instalar em caminhos XDG ou `--prefix` diferentes, ajuste a unit para esses caminhos antes de iniciá-la.

## Etapa 2 (hooks): configuração manual do Claude

Não aplique este trecho durante a instalação inicial. Quando decidir ativar hooks, acrescente os sete eventos abaixo ao `.claude/settings.local.json` do projeto onde o Claude roda (ou ao `~/.claude/settings.json`, que vale para todos os projetos). **Se já existirem hooks desses eventos, mescle os arrays em vez de substituir.** Cada comando reporta somente o nome do evento e o pane tmux ao agentd (`$TMUX_PANE`); o stdin do hook é ignorado e o comando sai sempre com 0. **Os clientes do `agentd` não têm socket padrão:** por isso o comando leva `--socket` explícito; sem ele o relato falha em silêncio.

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report UserPromptSubmit --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ],
    "PermissionRequest": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report PermissionRequest --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ],
    "Elicitation": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report Elicitation --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report Stop --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ],
    "StopFailure": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report StopFailure --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report SessionStart --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "agentd report SessionEnd --socket \"$XDG_RUNTIME_DIR/agentd/agentd.sock\"",
            "async": true
          }
        ]
      }
    ]
  }
}
```
