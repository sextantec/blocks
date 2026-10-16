# Testes independentes do agentd rev5 (escritor de testes)

Escopo: caixa preta de R1 a R20 da secao 11 de `crates/agentd/docs/spec-rev5.md`, mais os testes de
configuracao, arquivo de regras e CLI (secoes 4, 4A, 5, 6, 7, 8 e Anexos A e B) e os esclarecimentos da
**secao 15** (rev5.2). Nao escrito: R14 e os testes da rev4 que so trocam a fonte falsa (G5, G6, G9, G10,
G12, G14, G20, G22), por escopo. Nada commitado.

## Arquivos

| Arquivo | Conteudo |
|---|---|
| `tests/rev5_common/mod.rs` | harness: cenario do tmux falso, subida do daemon (com ou sem variaveis de ambiente), cliente do socket, CLI, goldens lidos do `default.json` |
| `tests/rev5_common/fake_tmux.py` | tmux FALSO (python3 -I): lista fechada, trava flock, feed de eventos, log de chamadas |
| `tests/rev5_harness.rs` | `harness_*` (12): exercitam o falso sem o daemon; passam hoje |
| `tests/rev5_rules.rs` | R1, R6, R7, R8, R9, R15, R16, R17 (20 testes) |
| `tests/rev5_source.rs` | R2, R3, R4, R5, R10, R13 (18 testes) |
| `tests/rev5_events.rs` | R18, R19, R20 e status de eventos (13 testes) |
| `tests/rev5_config_cli.rs` | R11, R12, `rules check`, CLI e prontidao (14 testes) |

Como rodar (offline, sem tmux real, sem agente real):

    cd /home/mjabbur/Projects/blocks
    cargo test --offline -p agentd -- harness_
    cargo test --offline -p agentd --no-fail-fast --test rev5_rules --test rev5_source --test rev5_events --test rev5_config_cli
    cargo clippy --offline -p agentd --test rev5_harness --test rev5_rules --test rev5_source --test rev5_events --test rev5_config_cli -- -D warnings

## Como o tmux falso funciona

- O daemon recebe `tmux_bin` apontando para um wrapper `sh` gerado no temporario, que chama
  `python3 -I fake_tmux.py DIR_CENARIO "$@"` com o python3 por caminho absoluto. Nada do tmux real e executado.
  O teste de `tmux_bin` ausente (spec 15.4) usa um PATH controlado so com o falso.
- O cenario e `cenario.json`, trocado por rename atomico. Tem paineis (campos da secao 4), capturas (arquivo de tela
  por painel), modo do `list-panes` (ok, falha, sem servidor, atraso, texto, arquivo grande), sentinela de stderr e
  falha do cliente de eventos.
- Lista fechada: `list-panes -a -F FMT`, `capture-pane -p -e -J -t %N` e `-C attach -f ignore-size -t $N`, com
  `-S SOCKET` antes do subcomando quando configurado. Qualquer outro argv vira `violacao` (sai 2, o teste falha).
- Chamadas em `chamadas.jsonl` (argv, pid, instante monotonico). `list-panes` e `capture-pane` pegam flock;
  contencao vira `sobreposicao`.
- O cliente de eventos le `eventos-N.txt` a partir do fim do arquivo no inicio do processo, repassa linhas `%...`
  e conta os bytes do stdin. `__EOF__`, `__LONG__` (linha de 1,1 MiB) e `%exit` simulam as mortes do cliente;
  `anexo_falha` faz o `-C attach` sair 1.

## Cobertura por R

| R | Testes | Observacao |
|---|---|---|
| R1 | 1 | percorre as 38 entradas `golden` do `default.json`; a falha nomeia a fixture |
| R2 | 10 | tela sem regra, vazia, de outro programa, servidor fora (nunca gone), saida invalida, pane_id duplicado, tag ambigua, captura falha de um agente, tela > 256 KiB, list-panes > 1 MiB (`source_too_large`, spec 15.3) |
| R3 | 1 | 10 ciclos; so `list-panes`/`capture-pane`; nunca captura nao configurado, morto ou com shell; sem sobreposicao; `-F` = nove campos separados por TAB literal, nesta ordem (spec 15.6) |
| R4 | 1 | `tmux_bin` e `tmux_socket` com `;`, `$()`, espacos e aspas: nada executa; `-S` com o caminho literal antes de `capture-pane` (spec 15.7) |
| R5 | 1 | sentinelas em tela, titulo, stderr do falso, saida invalida e cwd: nunca em `agent.*`, `status`, eventos, CLI ou stderr do daemon |
| R6 | 7 | idle isolado nao publica; idle confirma so depois de `idle_confirm_ms`; polls 1 e ms 0 imediato; `window_activity` mudando zera (sem cliente de eventos); troca de pid zera; erro no meio zera; falso idle do Codex com spinner no titulo fica working |
| R7 | 3 | `input` nulo fora de idle; `unknown` sem reguas; mudar so `input` nao gera evento (head_seq igual) |
| R8 | 2 | marca `background` nao muda o estado e gera evento so pela marca; a marca sai quando o subagente some |
| R9 | 3 | blocked vence working na mesma tela; titulo `Action Required` vence o spinner; duas blocked: vale a primeira do arquivo |
| R10 | 4 | pid que muda vira unknown+error com evento; painel some: gone so depois de `gone_after_s` (medido: nao antes de 4,9 s; o limite exato nao e medido, spec 15.9); morto e volta ao shell (ausente, volta a working com evento gone->working); lista vazia logo apos erros nao da gone |
| R11 | 4 | arquivo invalido nao sobe (campo desconhecido, chave duplicada, regex invalida, campo na regra, kind sem regras, regra sem fixture, blocked sem reason, reason em working, 0666); `rules check` 0 no padrao; 1 para regra sem fixture que a case, sem imprimir tela; uso incorreto sai 2 |
| R11 (15.5) | 1 | `serve` sobe com fixture inexistente (nao abre as fixtures); `rules check` sai 1 e nomeia a fixture |
| R12 | 3 | 25 configs invalidas nao sobem (cada recusa tem controle positivo); chave duplicada; fonte por `pane` sobe e funciona. `tmux_bin` nao-string ou vazio recusa (15.4) |
| R12 (15.4) | 1 | sem `tmux_bin` o daemon usa o `tmux` do PATH (PATH controlado so com o falso) |
| R13 | 1 | leitura travada: unknown+error, depois `stale` com `last_known`; `stale` nao antes de `stale_after_ms`; volta com leitura valida. O limite exato nao e medido |
| R14 | 0 | fora deste escopo |
| R15 | 1 | 11 sinais medidos com a tela real (plano, /model, pergunta, confianca, Claude 60 colunas, Codex `[ . ]`/`[ ! ]`, Update, transcricao, Pi 50 colunas, Pi /model) |
| R16 | 1 | telas ociosas sem a linha de modo nunca viram idle (versao real nao coberta, ver lacunas) |
| R17 | 2 | rascunho logo apos working nao publica idle; rascunho estavel confirma idle com `input: draft` |
| R18 | 4 | `%output` do painel antecipa o ciclo (< 2 s com poll de 10 s); rajada gera poucos ciclos; `%output` de painel nao configurado nao gera ciclo; conteudo do `%output` nunca sai |
| R19 | 4 | saida do proprio painel zera a janela; saida de outro painel nao zera; cliente morto vira `degraded`, nada vira idle, religa; cliente que nao sobe degrada e o polling confirma |
| R19 (15.2, 15.8) | 2 | `events: false` reporta `off`; com cliente vivo, `window_activity` mudando sem `%output` do painel nao zera a quietude (idle em < 3 s) |
| R20 | 3 | argv exato sem `-S` (`-C attach -f ignore-size -t $N`), um por sessao, stdin com zero byte, `events` = `on`; com `tmux_socket`, `-S SOCKET` antes de `-C attach`; no maximo 8 clientes com 9 sessoes |
| CLI | 4 (+1 de prontidao) | codigos 0, 2, 3, 4 e 1; `get --json` = `data.agent` + `epoch` + `head_seq`; `list --json` = `data.agents` + `epoch` + `head_seq` (spec 15.1); `watch` com `--since` sem `--epoch` sai 2; health nao falha por agente unknown. Prontidao: `agentd: pronto` em stderr (spec 15.10) |
| `harness_` | 12 | auto-testes do tmux falso e das goldens; passam hoje |

Total: **65 testes de rev5** (20 + 18 + 13 + 14) e 12 `harness_`.

## Resultado (binario rev4, verificado em 2026-10-16)

| Suite | Passam | Falham | Motivo das falhas |
|---|---|---|---|
| `rev5_harness` | 12 | 0 | — |
| `rev5_rules` | 0 | 20 | daemon rev4 recusa a config da rev5 (`agentd: campo desconhecido na config`: `tmux_bin`, `rules_file`, `events`) e nao sobe |
| `rev5_source` | 0 | 18 | mesmo motivo |
| `rev5_events` | 0 | 13 | mesmo motivo (`events`, `safety_poll_ms`) |
| `rev5_config_cli` | 1 | 13 | `rules check` inexistente (rev4 sai 2 com `agentd: uso`); recusas com controle positivo (a config valida ja e recusada pela rev4); prontidao e health com config rev5 recusada. Passa: `cli_sem_nenhum_socket_sai_2_e_daemon_fora_sai_4`, porque a CLI da rev4 ja cumpre esses codigos e a CLI nao muda na rev5 |

Nenhuma falha vem de bug do harness: cada mensagem aponta para configuracao ou subcomando que a rev4 nao tem.

**Nao validado:** as expectativas nunca rodaram contra uma implementacao rev5. Pode haver erro de expectativa que so
aparece la.

## Decisoes de harness

- Esperas com prazo (poll de 20 ms), sem sleep fixo para sincronizar, exceto folgas curtas: 300 ms depois de o anexo
  aparecer no log (o falso registra o inicio antes de fixar o ponto de leitura do feed).
- Antes da primeira leitura valida o agente aparece `unknown` com `error`. Os testes de estado esperam `observed_at`
  nao nulo antes de aceitar um resultado; os de ausencia e de stale so exigem `unknown`+`error` depois que o `error`
  aparece (antes, o estado ainda e o anterior).
- `poll_ms` 200 nos testes de estado; nos de evento, `poll_ms` e `safety_poll_ms` em 10000, para que so o evento
  dispare a leitura (R18).
- Config base: `events: false`, `source_timeout_ms` 400, `stale_after_ms` 3000, `gone_after_s` 5, `idle_confirm_polls` 2,
  `idle_confirm_ms` 0. Regras copiadas para o temporario com modo 0644.
- Testes de recusa exigem controle positivo: a config valida de referencia tem que subir antes de a mutante ser recusada.
- A CLI recebe `--socket` no fim (subcomando primeiro), como o harness da rev4.
- HOME e XDG_CONFIG_HOME em temporarios; nada toca `~/.config`, `~/.local`, `~/.claude` ou credenciais.
- Os testes nao leem `src/`; `default.json` e fixtures sao lidos como dados.

## Ambiguidades: resolvidas pela secao 15 da spec (rev5.2)

| Item | Resposta da spec | Onde esta testado |
|---|---|---|
| 1 `--json` de get/list | `data` = `{agent, epoch, head_seq}` / `{agents, epoch, head_seq}` | `cli_codigos_de_saida_e_formato_com_daemon_no_ar` |
| 2 `status.sources.tmux.events` | `on` com cliente vivo, `degraded` se algum caiu, `off` com `events: false` | `r20_argv_exato...`, `r19_status_events_off_quando_desligado`, `r19_cliente_*` |
| 3 list-panes > 1 MiB | `source_too_large` | `r02_saida_do_list_panes_acima_de_1_mib...` |
| 4 `tmux_bin` ausente/invalido | padrao `tmux` do PATH; vazio ou nao-string recusa | `r12_tmux_bin_ausente_usa_o_tmux_do_path`, `r12_configuracoes_invalidas...` |
| 5 fixtures | `serve` nao abre; `rules check` sai 1 nomeando a fixture | `r11_fixtures_nao_sao_abertas_pelo_serve...` |
| 6 separador do `-F` | TAB literal (0x09), nove campos | `r03_lista_fechada_...` |
| 7 `-S` | antes do subcomando, tambem no `-C attach` | `r04_caminhos_hostis...`, `r20_com_tmux_socket_o_s_vem_antes_do_c` |
| 8 quietude com cliente vivo | so o `%output` do proprio painel zera; `window_activity` nao entra | `r19_window_activity_nao_zera_a_janela_com_cliente_vivo` |
| 9 `gone_after_s` | nao medir o limite exato; aceitar atraso de ate um poll | `r10_painel_some_vira_gone_so_depois_de_gone_after_s` (so limite inferior) |
| 10 prontidao | linha `agentd: pronto` em stderr | `cli_serve_imprime_pronto_quando_aceita_conexao` |

Itens 9 e a lacuna "troca de regra" ficam como estao, por decisao do autor da spec.

## Lacunas

- **R14** (atomicidade, protocolo, assinante lento, reinicio, `since`/`blocked_reason`): fora do escopo.
- **`watch --json`** (spec 15.1: cada linha traz o `Evento` em `data`): nao testado; o comando nao termina.
- **`rules check` com fixture ilegivel** (15.5): so o caso inexistente e testado.
- **Troca de regra zera a janela** (5'): com `default.json` cada kind tem uma so regra `idle`; nao observavel sem arquivo alternativo.
- **R16 com versao real:** nao existem fixtures de outra versao do Claude, Codex ou Pi; o teste usa telas sem a linha de modo.
- **Medidas da secao 12 da spec** (Claude em `acceptEdits`, `/compact`, Codex em aprovacao de edicao, Pi com extensoes,
  terminais < 40 colunas): sem fixture, sem teste.
- **Limite exato de `stale_after_ms` e `gone_after_s`:** nao medido (spec 15.9).

## Verificacao final (2026-10-16, rev5.2)

- `cargo test --offline -p agentd --no-run`: compila, 0 avisos.
- `cargo test --offline -p agentd -- harness_`: 12 passam.
- Suites rev5 (`--no-fail-fast`): rules 0/20, source 0/18, events 0/13, config_cli 1/14. Total: 1 de 65 passa (o teste
  de CLI que a rev4 ja cumpre).
- `cargo clippy --offline -p agentd --test rev5_harness --test rev5_rules --test rev5_source --test rev5_events --test rev5_config_cli -- -D warnings`: sai 0.
  Ha avisos de clippy em `tests/agentd_v1.rs` e `tests/common/mod.rs`, que nao sao deste trabalho.
- `cargo fmt -p agentd -- --check`: limpo.
- `git status --short`: so arquivos novos em `tests/` (`REV5-TESTES.md`, `rev5_*.rs`, `rev5_common/`). Modificado por
  outro autor: `crates/agentd/docs/spec-rev5.md` (secao 15, editada pelo coordenador). Nada em `src/`, `Cargo.*`,
  `rules/` ou nos testes existentes.
- Nenhum `git add`, `stash`, `reset`, `checkout` ou commit foi feito.
