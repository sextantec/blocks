# chatd: script de instalacao/atualizacao (`scripts/instala.sh`, contrato testavel)

Origem: pedido do Jabbur (2026-10-15): "Peca ao Blocks para criar um script de instalacao. Eu reviso e executo. Facilita no futuro". Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes por outro agente que nao le o script. **O Claude nunca executa este script**: o Jabbur o revisa e roda.

## Problema (exemplo)
Cada versao nova do `chatd` (v1.3, v1.4, v1.5) exige o mesmo ritual manual: compilar em release, guardar o binario antigo, trocar, reiniciar o servico, conferir. Feito a mao, cada passo pode ser esquecido (o daemon do systemd ficou duas versoes atras sem ninguem perceber). O script torna o ritual repetivel, conferivel e reversivel.

Ja existe `scripts/systemd/instala-unit.sh` (primeira instalacao: estado, config, tokens, unit). Este script e **so a atualizacao do binario** de uma instalacao existente.

## Interface
`crates/chatd/scripts/instala.sh [--dry-run] [--yes] [--prefix DIR] [--service NOME] [--no-restart] [--no-build] [--offline] [--allow-dirty] [--keep N] [--wait S]`
- `--prefix DIR`: onde fica o binario (padrao `$HOME/.local/bin`).
- `--service NOME`: unit de usuario (padrao `chatd.service`).
- `--keep N`: quantos backups manter (padrao 5). `--wait S`: espera pelo servico (padrao 15).
- Estado do daemon: `$CHATD_HOME` (padrao `$HOME/.local/state/chatd`).

## Regras
1. **Bash**, `set -euo pipefail`, sem `sudo`, sem rede propria (o `cargo` pode usar a rede; `--offline` repassa `--offline`). Escreve **somente** em `--prefix` (o binario e os backups). **Nunca** toca `$CHATD_HOME` (config, tokens, log, estado) nem `~/.config`: so le a `config.json` para o teste de fumaca (regra 5). Nunca imprime nem le tokens.
2. **Pre-condicoes** (falha = saida 2, nada alterado): raiz do repositorio encontrada; `systemctl --user` disponivel e `--service` existente (a menos que `--no-restart`); arvore **sem arquivo rastreado modificado**, a menos que `--allow-dirty`. Imprime o commit (`git describe --always --dirty`), o sha256/tamanho do binario instalado hoje e, se o servico roda, o sha256 de `/proc/<MainPID>/exe`.
3. **Plano e confirmacao:** imprime o plano (passos e comandos) e pede `[s/N]`; `--yes` pula a pergunta; `--dry-run` imprime o plano e **nao executa nada que escreva** (nem build).
4. **Build:** `cargo build --release -p chatd [--offline]` a partir do repositorio (pulado com `--no-build`, que exige `target/release/chatd` existente). Imprime o sha256 do novo binario.
5. **Teste de fumaca antes de tocar o instalado:** o novo binario precisa (a) listar o uso (`chatd` sem argumentos, saida nao vazia) e (b) **subir com uma copia da `config.json` real** em diretorio temporario (`serve --state-dir TMP/state --config TMP/config.json --socket TMP/s.sock`, socket em caminho curto) e escrever `chatd: ready` em ate 5 s; depois e encerrado e o temporario removido. Falha = saida 2, nada alterado.
6. **Idempotencia:** se o sha256 do novo binario for igual ao instalado, imprime "ja instalado" e sai 0; **se** o processo em execucao tiver sha256 diferente do instalado, so reinicia o servico (nao copia).
7. **Backup e troca atomica:** copia o binario atual para `PREFIX/chatd.AAAAMMDDTHHMMSSZ.bak` (0755); grava o novo em `PREFIX/chatd.new.PID` (`install -m 0755`) e `mv -f` sobre `PREFIX/chatd`. Poda os backups alem de `--keep` (so os que casam `chatd.*.bak`, os mais antigos).
8. **Reinicio e verificacao:** `systemctl --user restart NOME` (a menos que `--no-restart`); espera ate `--wait` s pelo estado `active`; confere que `/proc/<MainPID>/exe` tem o sha256 do novo binario e que `chatd log --state-dir $CHATD_HOME/state --tail 1` responde (sem autenticar).
9. **Reversao automatica:** falha em 8 (servico nao ativo no prazo, sha divergente ou log ilegivel) restaura o backup (`mv -f`), reinicia o servico, imprime o motivo e o comando `journalctl --user -u NOME -n 30 --no-pager`, e sai 1.
10. **Relatorio final:** commit, sha256 antes/depois, caminho do backup, estado do servico e o comando exato de reversao manual (`mv -f BACKUP PREFIX/chatd && systemctl --user restart NOME`). Lembrete: o script nao cria token, `client.json` nem muda a configuracao.
11. **Saidas:** 0 ok ou "ja instalado"; 1 falhou apos reversao; 2 uso/pre-condicao/fumaca; 3 instalado com `--no-restart` (servico nao reiniciado).

## Contrato testavel (testes independentes; `systemctl`, `cargo` e o binario sao FALSOS no PATH)
- **I1**: `--dry-run` nao cria nem altera nenhum arquivo (hash do `--prefix` e de `$CHATD_HOME` iguais antes/depois) e imprime o plano.
- **I2**: instalacao feliz: binario novo no `--prefix`, backup `chatd.*.bak` com o conteudo antigo byte a byte, servico reiniciado uma vez, saida 0.
- **I3**: mesmo sha256: "ja instalado", saida 0, sem backup novo, sem reinicio (exceto a regra de processo desatualizado).
- **I4**: servico nao fica `active` no prazo: o binario antigo volta byte a byte, o servico e reiniciado de novo, saida 1.
- **I5**: arvore suja recusa (saida 2) sem `--allow-dirty` e aceita com ele.
- **I6**: `--keep 2` poda os backups mais antigos e so os do padrao.
- **I7**: nenhum arquivo de `$CHATD_HOME` e alterado ou lido alem da copia da `config.json` (hashes iguais; token nunca na saida).
- **I8**: `--no-restart` nao chama `systemctl restart` e sai 3.
- **I9**: `--prefix` customizado; `--service` customizado.
- **I10**: fumaca falha (binario falso que nao escreve `chatd: ready`) = saida 2 e nada alterado.
- **I11**: `bash -n` passa e, se `shellcheck` existir, sem avisos.

## Fora de escopo
Primeira instalacao (ja existe `instala-unit.sh`); criar token `jabbur` e `client.json` (acoes do Jabbur); reinstalar a unit; qualquer `sudo`.
