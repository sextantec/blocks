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

    cd RAIZ_DO_WORKSPACE_BLOCKS
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
| R14 | 42 (rodada 5, `rev5_protocol.rs`) | protocolo, CLI, reinicio, atomicidade, assinante lento: ver "Rodada 5" |
| R15 | 1 | 11 sinais medidos com a tela real (plano, /model, pergunta, confianca, Claude 60 colunas, Codex `[ . ]`/`[ ! ]`, Update, transcricao, Pi 50 colunas, Pi /model) |
| R16 | 1 | telas ociosas sem a linha de modo nunca viram idle (versao real nao coberta, ver lacunas) |
| R17 | 2 | rascunho logo apos working nao publica idle; rascunho estavel confirma idle com `input: draft` |
| R18 | 4 | `%output` do painel antecipa o ciclo (< 2 s com poll de 10 s); rajada gera poucos ciclos; `%output` de painel nao configurado nao gera ciclo; conteudo do `%output` nunca sai |
| R19 | 4 | saida do proprio painel zera a janela; saida de outro painel nao zera; cliente morto vira `degraded`, nada vira idle, religa; cliente que nao sobe degrada e o polling confirma |
| R19 (15.2, 15.8) | 2 | `events: false` reporta `off`; com cliente vivo, `window_activity` mudando sem `%output` do painel nao zera a quietude (idle em < 3 s) |
| R20 | 3 | argv exato sem `-S` (`-C attach -f ignore-size -t $N`), um por sessao, stdin com zero byte, `events` = `on`; com `tmux_socket`, `-S SOCKET` antes de `-C attach`; no maximo 8 clientes com 9 sessoes |
| CLI | 4 (+1 de prontidao) | codigos 0, 2, 3, 4 e 1; `get --json` = `data.agent` + `epoch` + `head_seq`; `list --json` = `data.agents` + `epoch` + `head_seq` (spec 15.1); `watch` com `--since` sem `--epoch` sai 2; health nao falha por agente unknown. Prontidao: `agentd: pronto` em stderr (spec 15.10) |
| `harness_` | 12 | auto-testes do tmux falso e das goldens; passam hoje |

Total: **66 testes de rev5** (20 + 18 + 14 + 14) e 13 `harness_` (contagem atualizada na rodada 3).

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

## Correcoes pos-implementacao (rodada 3)

Auditoria de todos os testes e do harness contra a secao 7 (limites), a secao 15 (esclarecimentos) e a consistencia
de ids/tags/paineis. Cada correcao, com arquivo:linha atual, antes/depois e o trecho da spec que a motiva:

1. **`tests/rev5_common/mod.rs:372-383`** (helper `cfg`, usado por todos os testes): `stale_after_ms` fixo em 3000 e
   `safety_poll_ms` fixo em 5000. Com `poll_ms` 10000 (R18, `rev5_events.rs`) o daemon recusava a config. Depois:
   `stale_after_ms = max(3000, poll_ms + 400 + 2000)` e `safety_poll_ms = max(5000, poll_ms)`.
   Spec 7: "`stale_after_ms` > `poll_ms` + `source_timeout_ms`" e "`safety_poll_ms` >= `poll_ms`" (limite superior de
   stale: 300000). Testes que querem outro stale (ex.: R13, `rev5_source.rs`) continuam sobrescrevendo pelo `ajuste`.
2. **`tests/rev5_rules.rs:307`** (`r06_falso_idle_do_codex_...`): o painel era recriado com `.tag("codex-blocks")`, que e o
   ID do agente, nao a tag. Com isso o painel original virava ausente. Antes: `.tag("codex-blocks")`. Depois:
   `let (_, tag, cmd) = agente_do_kind("codex");` (tag `codex`, cmd `codex`). Spec 7: `tag` casa com a opcao `@agentd`
   do painel; o ID e so o nome do agente na configuracao.
3. **`tests/rev5_config_cli.rs:152-158`** (mutante "poll_ms acima de 10000"): antes, so `poll_ms = 10001`, e a
   `safety_poll_ms` (5000) ficava menor que o poll, violando tambem a regra safety >= poll. Depois: tambem
   `safety_poll_ms = 20000` e `stale_after_ms = 30000`, para que so o poll viole o limite. Spec 7: limites de cada chave.
4. **Auditoria sem outras inconsistencias de config**: todas as atribuicoes `c[...] = ...` nos testes (rules, source,
   events, config_cli) foram conferidas contra os limites da secao 7 (poll 200..10000, timeout 200..30000, stale,
   safety >= poll e <= 60000, gone 5..86400, idle_confirm_polls 1..5, idle_confirm_ms <= 30000, ring 64..65536,
   debounce 50..1000). Nenhuma outra violacao. Os mutantes de R12 que violam um limite de proposito estao isolados.
5. **Auditoria de ids, tags e paineis**: `tag("codex-blocks")` so aparece com `agentes_todos()` (que define essa tag) e
   no painel duplicado de `r02_pane_id_duplicado` (de proposito). Os `m.paineis(&[])` e a troca para `zsh` em
   `rev5_source.rs:492`, `:539` e `:577` sao intencionais (ausencia e volta). Nenhum outro painel virou ausente por engano.
6. **Auditoria de expectativas contra a secao 15**: nenhuma contradicao nova. Itens 1 a 10 continuam cobertos como na
   tabela acima.

Acrescimos desta rodada (nao sao correcoes de teste errado):
- `tests/rev5_harness.rs:224` `harness_config_gerada_respeita_os_limites_da_secao_7`: auto-teste que valida todos os
  limites da secao 7 sobre a config gerada pelo harness (para poll 200, 1000, 2000 e 10000). Passa hoje e impede a
  regressao do item 1.
- `tests/rev5_events.rs:466` `r19_depois_de_idle_confirmado_uma_saida_do_proprio_painel_reconfirma_sem_laco`: novo.
  Confirmado o idle, uma unica linha `%output` do proprio painel; depois de sem saida nova, a confirmacao volta e em
  2 s ha no maximo 3 ciclos (so o ciclo de seguranca, spec 4A). Ver "Defeito apontado" abaixo.
- `tests/rev5_common/fake_tmux.py:157`: registra `ev="emitida"` por linha `%output` repassada (instrumentacao, sem
  mudar o comportamento). Serviu para provar que o falso emitiu exatamente as 50 linhas da rajada.

## Defeito apontado pelos testes (rodada 3): resolvido na rodada 4

Na rodada 3, com a implementacao em andamento, estes dois testes falhavam. Na rodada 4 (binario atual, rev5.2
comitada) `rev5_events` passa 14/14, inclusive os dois: o defeito foi corrigido pela implementacao. Os testes nao
foram afrouxados. Registro historico:
- `r18_rajada_de_output_gera_poucos_ciclos_nao_um_por_linha` (`rev5_events.rs`): 50 linhas `%output` em rajada geram
  15 a 17 ciclos em ~1 s (cadencia de ~50 ms). A spec (R18) pede **um** ciclo por rajada e (4A) no maximo um ciclo
  por `event_debounce_ms` (150 ms).
- `r19_depois_de_idle_confirmado_...` (`rev5_events.rs`): reproduzido em isolamento (diagnostico temporario, removido):
  depois de `idle` confirmado (sem marca), **uma** linha `%output %1` do proprio painel deixa `idle_unconfirmed` e o
  daemon segue em ~20 ciclos/s. A confirmacao volta (teste passa nessa parte), mas o laco nao para. Sem `%output`
  nenhum, o mesmo painel nunca entra em laco. O falso emitiu exatamente 1 linha (contado pelo log `emitida`).
  Config do reproducao: `events: true`, `poll_ms` 200 ou 10000, `safety_poll_ms` 5000 ou 10000, `event_debounce_ms` 150,
  `idle_confirm_ms` 0 ou 1500, tela `claude/box-empty` estavel.
Esses dois testes sao a prova; nao foram afrouxados.

## Verificacao final (rodada 3)

- `cargo test --offline -p agentd --no-run`: compila, 0 avisos.
- `cargo test --offline -p agentd -- harness_`: 13 passam (12 anteriores + `harness_config_gerada_respeita_os_limites_da_secao_7`).
- Suites rev5 contra o binario atual (implementacao em andamento), `--no-fail-fast`:
  rules 20/20, source 18/18, config_cli 14/14, events 12/14. Falham: `r18_rajada...` e `r19_depois_de_idle_confirmado...`
  (defeito acima). Total: 64 de 66 passam.
- `cargo clippy --offline -p agentd --test rev5_harness --test rev5_rules --test rev5_source --test rev5_events --test rev5_config_cli -- -D warnings`: sai 0.
  Os avisos de clippy que sobram em `tests/agentd_v1.rs` e `tests/common/mod.rs` nao sao deste trabalho.
- `cargo fmt -p agentd -- --check`: meus arquivos limpos. Aparecem diferencas em `src/daemon.rs` e `src/main.rs`, que sao
  do implementador. **Ressalva:** rodei `cargo fmt -p agentd` varias vezes nesta rodada, com `src/` ja em alteracao;
  nao consigo provar que nao reformatei `src/` nesses momentos. Por isso o `fmt` foi rodado so no fim e os arquivos
  de teste estao limpos; quem commita deve conferir o diff de `src/` antes de commitar.
- Contagem: 66 testes rev5 (rules 20, source 18, events 14, config_cli 14) e 13 de harness.
- `git status --short`: `M crates/agentd/docs/spec-rev5.md` (secao 15, do coordenador); `M` e `??` em `tests/rev5_*` e
  `tests/REV5-TESTES.md`. Alteracoes do implementador em `src/`, `Cargo.*` (fora deste trabalho).
- Nenhum `git add`, `stash`, `reset`, `checkout` ou commit foi feito por mim.

## Rodada 4 (rev5.3): descoberta de agentes (R21 a R27)

Arquivos: `rev5_discovery.rs` (novo, 22 testes), `rev5_common/fake_tmux.py` (10 campos e modificador `s|`),
`rev5_common/mod.rs` (`Painel::cwd`, `agente_cwd`), `rev5_harness.rs` (+2 testes: sanitizacao e 10 campos; total 15),
`rev5_source.rs` (R3 agora espera 10 campos). Nada em `src/`.

**Falso e formato:** `list-panes` tem 10 campos, TAB literal, nesta ordem: `pane_id`, `pane_pid`, `pane_dead`,
`pane_dead_status`, `window_activity`, `pane_current_command`, `session_id`, `@agentd`,
`#{s|[^a-zA-Z0-9._/-]|_|:pane_current_path}` e `pane_title` por ultimo (spec 16). O falso expande o modificador `s|`
com `re.sub` (o que o tmux faz) numa passada so. O cenario guarda o caminho cru de cada painel (`Painel::cwd`, padrao
`/work/proj`) e a marca `@agentd` (NOME, `off` ou vazio). Config: `discover` e `discover_forget_s` nao entram na base
(o padrao da spec e `false`/3600, e so os testes de descoberta os ligam), para que as suites antigas nao falhem por
config. Fonte nova: `agente_cwd(c, id, kind, cwd)` gera `{type: tmux, cwd}`.

**Cobertura (22 testes):**

| R | Testes |
|---|---|
| R21 descoberta | `r21_descoberta_ligada_acha_claude_codex_e_pi_com_origin_discovered` (ids `claude-horus-reality-check`, `codex-blocks`, `pi-pi-proj`; estado pelas regras); `r21_descoberta_desligada_nao_mostra_nem_captura_painel_sem_marca` (controle: o objeto `discover` existe); `r21_shell_vim_desconhecido_e_morto_nunca_viram_agente` |
| R22 nome estavel | `r22_id_fixo_mesmo_com_diretorio_mudando_no_cenario`; `r22_dois_ou_tres_paineis_no_mesmo_diretorio_sufixo_pela_ordem_numerica_do_pane_id` (`%9` antes de `%10`; `-2` e `-3`; conferido pelo estado de cada tela); `r22_o_segundo_nao_rouba_o_id_quando_o_primeiro_some_e_reaparece`; `r22_troca_de_pane_pid_no_mesmo_pane_id_recalcula_o_id` |
| R23 precedencia | `r23_configurado_por_cwd_vence_e_nunca_e_descoberto_de_novo`; `r23_marca_sem_entrada_vira_agente_com_origin_mark`; `r23_marca_off_exclui_o_painel_da_descoberta` (nao captura); `r23_descoberto_que_colide_com_configurado_leva_sufixo_mesmo_com_pane_menor` |
| R24 ciclo de vida | `r24_painel_que_some_vai_a_gone_e_e_esquecido_depois_de_discover_forget_s` (60 s, o minimo valido; ~90 s de teste); `r24_teto_de_32_conta_excedentes_em_ignored` (33 candidatos: 32 listados, `ignored` = 1) |
| R25 captura minima | `r25_so_captura_configurado_marcado_ou_descoberto_vivo` (configurado, descoberto, marcado sem entrada; nao captura shell, `vim`, morto nem `off`); `r25_clientes_de_eventos_nascem_para_sessao_descoberta_e_no_maximo_8` |
| R26 caminho hostil | `r26_caminhos_hostis_geram_ids_validos_sem_executar_nem_vazar` (TAB, aspas, `$()`, unicode, 300 caracteres, raiz; sentinela so no componente pai, varrida em lista, status, eventos, CLI e stderr); `r26_ids_derivados_sao_os_da_spec` (`/w/Blocks`->`codex-blocks`; `/w/a---b`->`codex-a-b`; `/w/_abc_`->`codex-abc`; `/`->`codex`; 40 `a` -> 32 caracteres com `codex-`; `/w/x y`->`codex-x_y`); `r26_pane_current_path_vazio_e_source_bad_output` |
| R27 contrato | `r27_origin_em_get_list_eventos_e_cli` (`agent.get`, `agent.list`, eventos, `get --json`, `list --json`, linha da CLI com `discovered`); `r27_status_discover_tem_on_found_e_ignored`; `r27_validacoes_de_descoberta_recusam_saida_1` (forget 59 e 86401, cwd relativo, com espaco, com `..`, `discover` nao booleano, `agents` vazio com `discover` false; controle positivo); `r27_agents_vazio_so_e_aceito_com_discover_true` |

**Recusa de configuracao (R27):** cada mutante tem controle positivo (a config valida de `discover: true` sem agentes
sobe) antes da recusa, para que um binario que recusa tudo nao passe por acidente.

**Ambiguidades (a secao 16 nao fixa; nao inventei): os testes so afirmam o que a spec garante**

1. **`status.sources.tmux.discover.found`:** conta os descobertos publicados, ou os candidatos (incluindo os ignorados)?
   O teste so afirma `found == 1` com um unico candidato (caso em que as duas leituras coincidem) e `ignored` no teto.
2. **Quais candidatos ficam de fora acima de 32:** a spec so diz que os excedentes nao entram e sao contados. O teste
   afirma a contagem (32 listados, `ignored` = 1), nao a identidade do excluido.
3. **"Some da lista de ativos" (R24):** a spec nao define "ativos" nem diz se um `gone` continua em `agent.list` antes do
   esquecimento. O teste afirma so `gone` (depois de `gone_after_s`), a presenca antes de `discover_forget_s` e
   `unknown_agent` depois.
4. **Painel que reaparece com o mesmo `pane_id`/`pid` (R22):** o teste afirma que volta a ser `codex-blocks` e que nao
   surge um terceiro id. Isso e uma leitura de "guardado junto de (pane_id, pane_pid)" e de R22, nao uma frase explicita.
5. **Formato da origem na linha da CLI (R27):** a spec diz que a origem aparece "na linha da CLI", sem dizer onde. O
   teste so afirma que a palavra `discovered` aparece na linha.
6. **`..` em `cwd`:** a secao 16 diz que o caminho vai "sem `..`", mas R27 so lista "nao absoluto ou com caractere fora
   de `[A-Za-z0-9._/-]`". O teste recusa `/w/../x` por causa da secao 16 (a unica fonte da regra).
7. **`agents` vazio com `discover` false:** a secao 16 diz que `agents` "passa a poder ser vazio quando `discover` e
   `true`", o que implica recusa com `false`. O teste afirma a recusa; e uma inferencia da frase.
8. **Valor invalido de `@agentd` (ex.: maiusculas, espaco):** a spec nao diz se a marca e validada, descartada ou usada
   como nome. Nao testado.
9. **Truncamento com sufixo de colisao:** quando o base chega a 32 e a colisao pede `-2`, nao esta dito se o base e
   cortado para caber o sufixo. Nao testado (so o truncamento sem colisao, `r26_ids_derivados...`).
10. **Caracteres multibyte no `s|`:** a spec diz so que "unicode vira `_`" (medido). Se e um `_` por caractere ou por byte
    nao esta dito. Os testes de unicode afirmam so a validade do id, nao o numero de `_`.
11. **Esquecimento com exatidao:** a spec diz "depois de `discover_forget_s` em `gone`", sem a precisao do limite. O
    teste de 60 s afirma so que nao esquece antes de 20 s e que esquece ate 75 s depois da observacao de `gone`.
12. **Clientes de eventos misturando configurado e descoberto:** a spec diz que o limite de 8 e "inalterado". O teste
    cobre so o caso com 8 descobertos.

**Lacunas:** o teste de esquecimento leva ~90 s (o minimo valido de `discover_forget_s` e 60 s). Nao ha teste de
`cwd` de tamanho limite (200+ caracteres) alem do caminho de 300 caracteres de R26.

**Resultado contra a implementacao atual (rev5.2, sem descoberta):**
- `rev5_discovery`: 0/22. Motivos: 18 testes falham so por `campo desconhecido na config` (a rev5.2 recusa
  `discover` e `discover_forget_s`); 2 (`r27_validacoes...` e `r27_agents_vazio...`) falham no controle positivo
  (a config valida de `discover: true` e recusada); 1 (`r27_status...`) porque o objeto `status.sources.tmux.discover`
  nao existe (`null`); 1 (`r21_descoberta_desligada...`) pelo mesmo objeto ausente, no controle positivo. Nenhum motivo
  vem de bug do harness.
- `rev5_rules` 20/20, `rev5_events` 14/14, `rev5_config_cli` 14/14, `rev5_source` 17/18.
- `rev5_source::r03_lista_fechada...`: **falha por causa do 10o campo**. O falso agora manda o `-F` com 10 campos, e a
  checagem de R3 espera os 10 (com `pane_current_path` sanitizado antes de `pane_title`). A rev5.2 manda o proprio `-F`
  com 9 campos, e a asserção falha exatamente nisso ("o -F deve ser os dez campos", com a string enviada pelo daemon).
  Nenhum outro teste antigo falha por causa do falso de 10 campos: a rev5.2 absorve o campo extra sem erro, e os testes
  de regras, eventos e config continuam passando (o titulo ainda casa nas regras de Codex, que sao as sensiveis a ele).
- `harness_`: 15/15.

**Verificacao final (rodada 4):**
- `cargo test --offline -p agentd --no-run`: compila, 0 avisos.
- `cargo test --offline -p agentd -- harness_`: 15/15.
- `cargo clippy --offline -p agentd --test rev5_harness --test rev5_rules --test rev5_source --test rev5_events --test rev5_config_cli --test rev5_discovery -- -D warnings`: sai 0.
- `cargo fmt -p agentd -- --check`: limpo.
- `git status --short`: so `tests/rev5_*` e `tests/REV5-TESTES.md` (e os arquivos de `rev5_common/`).
- **Ressalva de escopo:** nesta rodada eu rodei `cargo fmt -p agentd` (a formatacao do workspace inteiro) para manter os
  arquivos limpos. No inicio da rodada `cargo fmt -p agentd -- --check` apontava diferencas em `src/daemon.rs` e
  `src/main.rs`; no fim nao havia nenhuma e `git status` de `src/` esta limpo. Nao consigo provar se o meu `fmt` mudou
  `src/` antes de o implementador commitar. Quem commita deve conferir o diff de `src/` no commit do implementador.
  Daqui em diante usei so a checagem, sem reescrever `src/`.
- Nenhum `git add`, `stash`, `reset`, `checkout` ou commit foi feito por mim.

## Rodada 5: migracao do `agentd_v1.rs` (herdr) para o tmux falso

Escopo (coordenador, 2026-10-16, autorizado pelo Jabbur: "A"): migrar para `tests/rev5_protocol.rs` **so** os testes
de protocolo, CLI, atomicidade, assinante lento e reinicio (grupos G5, G6, G9, G10, G12, G14, G20 e G22 da rev4 secao
10), e apagar o resto. `tests/agentd_v1.rs` e `tests/common/` (herdr) foram **removidos** depois que os migrados
passaram contra a implementacao (rev5.3, commit 2c74c6b). Nada em `src/`; nenhum commit.

**Contagem:** o `agentd_v1.rs` tinha 83 testes. Migrados 41 (+1 novo: `watch --json`; 1 ignorado). Apagados 42.

### Migrados (`rev5_protocol.rs`, 41 passam + 1 ignorado + 1 novo)

| Grupo | Old | Migrados | Nomes novos (`r14_...`) e nota |
|---|---|---|---|
| G5 eventos | 3 | 3 | `g5_seq_sem_lacunas` -> `seq_sem_lacunas_e_previous_state_encadeado` (3 eventos: unknown->working, working->blocked, blocked->working; seq 1 primeiro); `g5_evidence_e_source` -> `primeira_leitura_valida_gera_evento_com_evidence_observed_e_source_tmux` (source tmux, controle: antes evidence none); `g5_observed_at_freshness_e_cwd` -> `observed_at_freshness_e_cwd_nao_geram_evento` |
| G6 events.resume | 6 | 6 | `retoma_valido_com_last_seq_igual_ao_head`; `resume_gap_informa_oldest_seq_e_fronteira_exata` (anel 64, 32 agentes, `oldest = head - ring + 1`, rev4 13.5); `epoch_mismatch_apos_reinicio`; `last_seq_maior_que_head_e_negativo`; `segunda_chamada_de_resume`; `sem_eventos_head_zero_retoma_e_mais_um_falha` |
| G9 protocolo | 10 | 10 | campo desconhecido (`data.path` = `params.extra`, rev4 13.2); 1 MiB com LF aceita; 1 MiB + 1 = `invalid_request` e conexao fechada (13.2; antes era "nao processada"); EOF sem LF; UTF-8 invalido; `params` null (aceita 2 codigos, ver ambiguidade 4); pedido sem id; lote = `invalid_request` (13.2); `method_not_found`; `unsupported_version` |
| G10 seguranca | 2 | 1 + 1 ignorado | 0700/0600 migrado; outro uid segue `#[ignore]` (nao ha segundo uid no ambiente) |
| G12 CLI | 11 | 11 + 1 novo | get sem JSON (linha com `tmux`), get JSON (envelope + `data.agent` da 15.1), list com `--kind` (`data.agents`), health 0 (agente unknown, fonte ok), health 1 (fonte falha, 13.3), status, codigos 2/3/4, socket por `--state-dir` e `AGENTD_SOCKET`, watch (uma linha por troca). **Novo:** `watch --json` traz o Evento em `data` (15.1) |
| G14 reinicio | 1 | 1 | epoch muda; `agentd: pronto` em stderr (13.6); nada persiste (`last_known` null e estado nao volta) |
| G20 atomicidade | 1 | 1 | snapshot (`agent.get`) e `head_seq` do mesmo instante; replay + ao vivo sem lacunas; >= 20 eventos (controle) |
| G22 assinante lento | 1 | 1 | `closed(slow_consumer)` depois de 256 eventos sem leitura; `agent.get` < 1 s durante toda a troca (controle) |
| contrato (rev4 3, 5) | 6 | 6 | campos do Agente (`source` = `tmux`); freshness/last_seen/observed_at null antes da primeira leitura valida; epoch string de 64 bits; `unknown_agent`; `agent.list` ordenada por id e filtro por kind; forma do `status` com `last_error` = `source_exit_nonzero` |

Os migrados que eram negativos ganharam controle positivo (o mesmo pedido sem o defeito tem que dar resultado), para
que um daemon que responde sempre o mesmo erro nao passe. Ex.: `epoch_mismatch` tambem exige que a epoch nova retome;
`health 1` exige que, com a fonte de volta, `health` sai 0; `codigo 4` exige que, com o daemon no ar, `get` sai 0.

### Apagados (42)

| Grupo | Old | Motivo | Cobertura atual |
|---|---|---|---|
| G1 mapeamento herdr | 6 | mapeia `agent_status` do herdr (`done`, `unknown` etc.) | R1 (38 goldens) e R15 (`rev5_rules`) |
| G2 leitura falha (herdr) | 7 | `agent` nulo, `kind_mismatch`, `pane` duplicado do herdr | R2 (`rev5_source`) |
| G3 stale travado | 1 | leitura travada no herdr | R13 (`rev5_source`) |
| G4 gone | 2 | ausencia no herdr | R10 (`rev5_source`) |
| G17 erro + ausencia | 1 | precedencia herdr | R2/R10 (`rev5_source`) |
| G7 lista fechada (herdr) | 1 | so `herdr agent list` | R3 (`rev5_source`) |
| G8 sem shell (herdr) | 2 | `pane`/`herdr_bin` | R4 (`rev5_source`) |
| G11 privacidade (herdr) | 1 | sentinelas em titulo/stderr do herdr | R5 (`rev5_source`) |
| G13 configuracao | 6 | campo desconhecido, pane/id repetido, > 32, stale | R12 (`rev5_config_cli`) |
| G15 confirmacao de idle | 7 | fila de leituras do herdr | R6 (`rev5_rules`) e R19 (`rev5_events`) |
| G16 zeragem da janela | 2 | idem | R6 (`rev5_rules`) |
| G18 limites exatos | 2 | stale/gone exatos | R13 e R10 (limite exato nao medido, spec 15.9) |
| G19 terminal_id | 1 | troca de terminal no herdr | R10 (`rev5_source`, troca de pid) |
| G21 subprocesso | 2 | travamento e descendente vivo | R3 (sem sobreposicao) e R13 (stale). **Lacuna:** descendente vivo nao coberto |
| G23 launch_pending | 1 | campo exclusivo do herdr | sem equivalente no tmux |

### Pendentes (nao migrados de proposito)

- **G23 a G29 da rev4 secao 14** (`since`, `blocked_reason`): nao implementados (rev4 13.7 / secao 14). Nao migrados,
  como pedido. Ficam pendentes ate a implementacao: `since` (inicio da mudanca, sem mudanca em `marks`), `blocked_reason`
  (`permission|user_input|unknown`, `null` fora de `blocked`), e a emenda 14.4 (campos aditivos).
- **G21 descendente vivo nao impede os ciclos**: nao ha equivalente no falso tmux (exigiria um filho do `list-panes`
  que sobrevive ao timeout). Lacuna real de R3/R13.
- **G18 limites exatos** (stale no limite, gone no limite): spec 15.9 diz que os testes nao medem o limite exato.

### Ambiguidades (a spec nao fixa; afirmamos so o garantido)

1. **Linha do `watch` sem `--json`:** o formato nao esta fixado. O teste so afirma que a linha traz o id e o novo estado.
2. **Codigo de `params` null:** `invalid_request` ou `invalid_envelope` (rev4 5 nao fixa). Os dois sao aceitos.
3. **Listagem do diretorio de estado (G14):** o antigo teste afirmava so o socket no diretorio. A spec (rev4 2/9) proibe
   persistir estado, mas nao lista os arquivos permitidos. Nao afirmado; afirmamos so que nada do estado anterior volta.
4. **Campo `cwd` no `Agente` (tmux):** a rev4 3 o define, mas o tmux nao traz o diretorio de trabalho do agente (o
   caminho do painel e a fonte da descoberta, rev5.3 16). Afirmamos so a presenca da chave.
5. **`observed_at` antes da primeira leitura valida:** rev4 13.3 fixa `null`. Afirmado.
6. **Epoch depois de SIGTERM:** rev4 13.2 fixa saida 0 com SIGTERM. Afirmado.
7. **Anel depois de lotado:** rev4 13.5 fixa `oldest = head - ring + 1`. Afirmado (o antigo tinha a formula como AMBIGUO).
8. **Linha acima de 1 MiB:** rev4 13.2 fixa `invalid_request` e conexao fechada. Afirmado (o antigo aceitava "nao processada").
9. **Outro uid:** nao ha segundo uid no ambiente; o teste fica `#[ignore]` com o motivo.

### Lacunas

- **Tempo:** `rev5_protocol` leva ~18 s (o G20 ~9 s, o G22 com teto de 25 s e ~5 s de troca). O esquecimento de
  `rev5_discovery` (60 s) continua sendo o teste mais longo do conjunto.
- **Descendente vivo** (G21): nao coberto (ver pendentes).

### Verificacao (rodada 5)

Saidas coladas no relatorio final da rodada (ver mensagem ao coordenador). Resumo:
- `cargo fmt -p agentd -- --check`: limpo.
- `cargo clippy --offline -p agentd --all-targets -- -D warnings`: sai 0.
- `cargo test --offline -p agentd --no-fail-fast`: 144 passam, 1 ignorado (outro uid). `rev5_discovery` leva ~66 s por causa
  do esquecimento de 60 s.
- `git status --short`: `D tests/agentd_v1.rs`, `D tests/common/mod.rs`, `M tests/rev5_common/mod.rs`, `?? tests/rev5_protocol.rs`,
  `M tests/REV5-TESTES.md`. `src/` limpo.
