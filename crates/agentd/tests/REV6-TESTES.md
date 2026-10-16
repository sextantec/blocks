# Testes independentes do agentd rev6 (escritor dos testes, caixa preta)

Escopo: S1 a S6, H1 a H6, F1 a F6, F2b, P1 e P2 da secao 11 de `crates/agentd/docs/spec-rev6.md`, mais
ambiguidades e contradicoes encontradas na spec. Escrito sem ler `crates/agentd/src/`, a partir da spec
rev6, da rev5 (regras 1b, 4, 4A, 5', secoes 6 a 8), da spec rev4 (protocolo, secao 13) e do harness rev5
(`tests/rev5_common/`). Nada commitado; nada em `src/`, `Cargo.toml` ou `Cargo.lock`.

## Arquivos

| Arquivo | Conteudo |
|---|---|
| `tests/rev6_lacunas.rs` | segunda rodada (lacunas da mutacao): L1, L2, L3 (3 testes, cada um com controle positivo) |
| `tests/rev6_common/mod.rs` | harness rev6: reusa o harness rev5 por `#[path]` (sem copia), monta a configuracao com as fontes estruturadas, sobe o daemon com `HOME` temporario, `agentd report` com `TMUX_PANE`/`AGENTD_SOCKET` controlados, subida com recusa (F2b) |
| `tests/rev6_common/fake_codex.rs` | servidor WebSocket FALSO do app-server (so `std`): handshake RFC 6455 (SHA-1 e base64 proprios), quadros mascarados, registro de metodos e caminhos, `thread/status/changed` por `muda`/`notifica`, queda e recusa (`derruba`/`religa`) |
| `tests/rev6_codex.rs` | S1 a S6 (17 testes: 15 de contrato e 2 `harness6_` que validam o servidor falso e passam sem o daemon) |
| `tests/rev6_hooks.rs` | H1 a H6 (8 testes; H5 em tres) |
| `tests/rev6_fusao.rs` | F1 a F6 (8 testes; F2 em tres) |
| `tests/rev6_config_privacidade.rs` | F2b (4 testes), P1, P2 (6 testes) |

Total: 42 testes, 40 de contrato e 2 autotestes do servidor falso (39 da primeira rodada e 3 da segunda).

Como rodar (offline, sem tmux, Codex, Claude ou herdr reais, sem rede):

    cd RAIZ_DO_WORKSPACE_BLOCKS
    cargo test --offline -p agentd --test rev6_codex --test rev6_hooks --test rev6_fusao --test rev6_config_privacidade
    cargo test --offline -p agentd --test rev6_codex harness6_
    cargo fmt -p agentd -- --check

## Como o teste fala com o daemon

- **Tmux:** o tmux FALSO da rev5 (`rev5_common/fake_tmux.py`), com paineis configurados por `tag`
  (`@agentd`). Nenhum painel sem marca e capturado (`discover` desligado).
- **App-server:** `FakeCodex` em `tests/rev6_common/fake_codex.rs`. O daemon le `codex_socket` do teste; o
  teste nunca usa o `~/.codex` padrao. Todo teste com `codex_app_server` sobe com esse socket.
- **Relatos do Claude:** `agentd report EVENTO` com `TMUX_PANE` (ou sem ele), `AGENTD_SOCKET` do daemon do
  teste e o stdin do teste (sentinela de payload). O teste nao conhece o nome do metodo do socket: so verifica
  efeito observavel (estado, marcas, `status.sources.hooks.reports`, saidas).
- **Ambiente:** `HOME` e `XDG_CONFIG_HOME` temporarios no daemon e no `report`; `TMUX_PANE` e
  `AGENTD_REPORT_TIMEOUT_MS` removidos do ambiente herdado, exceto quando o teste os define.
- **Temporarios:** os diretorios vem do crate `tempfile` (`tempfile::tempdir()` e `tempfile::tempfile()`),
  criados em `$TMPDIR` ou, se nao definido, em `/tmp`. Cada teste tem o seu. Nada e escrito em `~/.codex`,
  `~/.config`, `~/.claude` ou `~/.local`.

## Tabela: teste -> item da spec -> ambiguidade/suposicao

| Teste | Item da spec | Ambiguidade ou suposicao (ver lista abaixo) |
|---|---|---|
| `harness6_aceite_websocket_segue_a_rfc6455` | (autoteste) | valida o SHA-1/base64 do servidor falso contra o exemplo da RFC 6455 |
| `harness6_servidor_falso_faz_handshake_...` | (autoteste) | upgrade em `/`, `initialize` respondido, caminho `/daemon/shutdown` recebe 404 e fica registrado |
| `s1_idle_active_aprovacao_idle_...` | S1; secao 4 (mapeamento); regra 7 | A3 (evidence `reported` com tela sem regra) |
| `s1b_notificacao_de_outra_thread_...` | secao 4 ("escuta ... da thread configurada") | A1 (campo `threadId`) |
| `s2_waiting_on_user_input_...` | S2 | - |
| `s3_status_de_erro_e_tipo_desconhecido_...` | S3; principio 7 | A5 (`systemError` nao e "estruturada inativa") |
| `s3b_not_loaded_vira_unknown_...` | S3; secao 4 (`notLoaded` => unknown + structured_lost); regra 1 | **A4 contradicao** |
| `s3c_active_flag_inesperada_vira_unknown_...` | secao 4 ("campo inesperado => unknown") | A6 (flag desconhecida em `activeFlags`) |
| `s4_lista_fechada_...` | S4; secao 4; secao 9 | A2 (`experimentalApi` ausente), A1 |
| `s5_queda_do_socket_...` | S5; regra 1; secao 4 (reconexao relê estado) | A7 (estado durante a queda: so a tela decide) |
| `s5_tentativas_de_reconexao_...` | S5; secao 4 (recuo entre min e max) | A8 (recuo: so limites, nao dobra exata) |
| `s6_thread_file_ausente_...` | S6; secao 4 (ausente => structured_lost, sem erro fatal) | - |
| `s6_thread_file_invalido_...` | S6; secao 4 (UUID minusculo, <= 64 bytes) | A9 (conteudo com quebra de linha final) |
| `s6_thread_file_symlink_nao_e_seguido` | S6; secao 4 (sem symlink) | - |
| `s6_thread_file_gravavel_por_grupo_ou_outros_...` | S6; secao 4 (sem escrita de grupo/outros) | A10 (dono do arquivo nao testado) |
| `s6_thread_file_criado_depois_e_relido_...` | secao 4 (`thread_file_reread_s`) | A11 (janela de espera) |
| `s6_thread_fora_da_lista_de_carregadas_...` | secao 3 ("thread desconhecida" => structured_lost) | A12 (se `thread/read` e chamado) |
| `h1_userpromptsubmit_working_e_stop_idle_...` | H1; secao 5; regra 4 | A3, A13 (latencia medida a partir do `report`) |
| `h2_permissionrequest_vira_blocked_...` | H2; secao 5 (mapeamento) | - |
| `h3_relato_de_painel_nao_configurado_...` | H3; secao 5 (rejeitado e contado) | A14 (contagem de `rejected`) |
| `h4_evento_fora_da_lista_e_descartado` | H4; secao 5 (lista fechada) | A15 (contador nao afirmado para evento fora da lista) |
| `h5_report_sem_daemon_sai_0_...` | H5; secao 8 (sai 0, < 1 s, sem saida padrao) | - |
| `h5_report_com_socket_mudo_...` | H5; secao 8 (`--timeout-ms`); secao 5 (so `event` e `pane`) | A16 (formato do envio, timeout de 200 ms) |
| `h5_stdin_do_hook_e_ignorado_...` | H5; principio 8; secao 9 | - |
| `h6_sessionend_com_painel_que_some_...` | H6; secao 5 (`SessionEnd` => gone, regra 1b) | **A17 ambiguidade** (SessionEnd sozinho) |
| `f1_stop_com_tela_working_...` | F1; regra 3 da fusao; `diverge_after_ms` | A18 (estado antes do primeiro relato nao afirmado) |
| `f2_override_com_valor_configurado_...` | F2; regra 5 da fusao; `override_after_s` 5 | A19 (idade medida a partir do relato) |
| `f2_override_com_padrao_de_15_s` | F2 (padrao 15 s); secao 7 | - (teste de ~18 s) |
| `f2_sem_regra_idle_na_tela_nao_ha_override` | F2; regra 5 (sem regra de idle, sem override) | - |
| `f3_blocked_nunca_sofre_override_...` | F3; regra 2 e regra 5 | - (controle: working com a mesma tela vira idle) |
| `f4_stop_falso_repetido_...` | F4; regra 3 | - (historico de eventos sem `idle`) |
| `f5_sem_fonte_estruturada_...` | F5; regra 1 (= rev5) | A20 (`evidence` `observed` em modo so tela) |
| `f6_unmatched_ratio_sobe_...` | F6; secao 3 (contadores de saude) | **A21 localizacao de `screen`** |
| `f2b_chaves_de_tempo_sao_aceitas_nos_extremos_...` | F2b; secao 7 (tabela de faixas) | - (controle: configuracao de referencia sobe) |
| `f2b_valores_zero_abaixo_e_acima_...` | F2b; secao 7 ("fora da faixa, ou 0, a config e recusada") | A22 (saida 1 na recusa) |
| `f2b_codex_reconnect_min_maior_que_max_...` | F2b; secao 7 | - (controle: min == max sobe) |
| `f2b_chave_desconhecida_inclusive_...` | F2b; secao 7 (chave desconhecida recusada) | - |
| `p1_sentinelas_..._nunca_saem` | P1; secao 9; principio 8 | - (varredura: stderr, `agent.*`, `status`, eventos, CLI, saidas do report) |
| `p2_daemon_so_usa_list_panes_..._nunca_captura_painel_nao_configurado` | P2; regra R3 da rev5 | A23 (eventos desligados: nao cobre `-C attach`) |

## Ambiguidades, suposicoes e contradicoes

Cada item diz o que o teste afirma e por que. Quando a spec nao decide, o teste segue a leitura mais literal.

**A1. Forma das mensagens do app-server.** A spec nao diz o nome de campo `threadId` nem o formato de resposta
de `thread/loaded/list` e `thread/read`. O servidor falso responde `{"id","result"}` com `result` =
`{"data":[ids],"nextCursor":null}` (lista) e `{"thread":{"id","status"}}` (leitura), e notificacoes
`{"method","params":{"threadId","status"}}` **sem** o campo `jsonrpc`, como o app-server real. Se o daemon
exigir `jsonrpc` ou outro nome, o teste pode falhar por forma e nao por regra.

**A2. `initialize` "sem experimentalApi" (secao 4 e S4).** Lido como: a chave `capabilities.experimentalApi`
esta **ausente** (ou `capabilities` ausente). Um daemon que envie `experimentalApi: false` falha o teste. Leitura
literal; registrada.

**A3. `evidence` com tela sem regra.** A secao 3 define `both` como "concordam na ultima leitura valida". Tela
sem regra e `unknown`, nao concordancia; por isso S1 e H1 afirmam `evidence: reported`. Se a implementacao
usar `both` com `idle` e tela sem regra, o teste falha.

**A4. CONTRADICAO: `notLoaded` (S3 x secao 4 x regra 1).** S3 diz "`notLoaded` => `unknown`, nunca `idle`". A
secao 4 diz `notLoaded` => `unknown` com a marca `structured_lost`. A regra 1 da fusao diz que, sem estruturada
ativa (`structured_lost`), **a tela decide**, e `idle` por prova positiva. Com tela `codex/idle` (positiva), a
regra 1 publica `idle`, contra S3. O teste `s3b_not_loaded_...` segue S3 ("nunca idle") e tem a tela idle
positiva para expor o caso. **Decisao pendente do Jabbur:** ou `notLoaded` nao e "structured_lost" (e fica como
`unknown` sem a regra 1), ou a regra 1 vale e S3 e a secao 4 mudam.

**A5. `systemError` e a regra 1.** A secao 4 nao da marca a `systemError` (so a `notLoaded`). Pela regra 1,
so `structured_lost` cai na tela. Logo `systemError` nao pode publicar `idle` pela tela. `s3_status_de_erro_...`
afirma `unknown` e nunca `idle` com tela idle positiva.

**A6. `activeFlags` com valor desconhecido.** A secao 4 manda `unknown` para "campo inesperado". Um valor de
`activeFlags` fora de `waitingOnApproval`/`waitingOnUserInput` e tratado como campo inesperado. `s3c_...` afirma
`unknown`. `p1_...` nao afirma o estado final, so que nao vira `idle` e que o daemon processou a mensagem.

**A7. Estado durante a queda.** A regra 1 diz "a tela decide", e a tela sem regra da `unknown`. `s5_queda_...`
nao afirma o estado logo apos `derruba`; afirma `structured_lost`, depois `working`/`idle` pela tela (fonte
`tmux`), e a volta a `blocked` pela releitura (fonte `app_server`).

**A8. Recuo de reconexao.** A secao 4 diz "recuo exponencial (dobra a cada falha) entre min e max". O teste
afirma so que cada intervalo (a partir do segundo) cabe em `[min, max]` com folga de 500 ms e que ha pelo
menos 3 tentativas em 6,5 s. A dobra exata nao e afirmada (o relogio e ruidoso).

**A9. Quebra de linha no thread_file.** A spec nao diz se um `\n` final e aceito. O teste grava o UUID sem
quebra. Os casos invalidos sao: UUID em maiusculas, texto, vazio e mais de 64 bytes.

**A10. Dono do thread_file.** "Do usuario do daemon" nao e testavel sem `root` nem segundo uid. Nao coberto.

**A11. Releitura de thread_file.** O teste cria o arquivo depois do boot e espera 8 s, com
`thread_file_reread_s` = 5 (min da faixa). A folga cobre um ciclo e o atraso de leitura.

**A12. Thread fora da lista de carregadas.** A secao 3 diz "thread desconhecida => structured_lost". A spec
nao diz se `thread/read` pode ser chamado nesse caso. O teste exige so que `thread/loaded/list` foi consultado e
que o estado sai da estruturada (`structured_lost`, fonte `tmux`).

**A13. Latencia "em menos de 1 s do evento".** Medida a partir do inicio do `report` (H1, H2) ou da chamada do
notificador (S1), ate o estado publicado, com amostragem de 20 ms.

**A14. Contagem de `rejected`.** A H3 diz que o relato de painel nao configurado "sobe `reports.rejected`". O
teste afirma +1 por relato rejeitado e que `accepted` nao sobe. Formato `status.sources.hooks.reports.{accepted,
rejected}`: suposicao (a secao 8 escreve `{ok, last_ok_at, last_error, reports: {accepted, rejected}}`).

**A15. Evento fora da lista.** A H4 so diz "descartado". O contador nao e afirmado (a spec nao diz se conta
como `rejected`). Afirmado: estado nao muda e o `report` sai 0.

**A16. Envio do `report`.** A spec nao fixa o metodo nem o envelope do socket. O teste com socket mudo afirma:
o report envia algo com o nome do evento e o painel, nunca o payload do stdin, e sai 0 dentro de
`AGENTD_REPORT_TIMEOUT_MS` = 200 (faixa 100 a 5000). Suposicao: o report envia pelo menos um relato antes de
esperar a resposta.

**A17. CONTRADICAO/AMBIGUIDADE: `SessionEnd`.** A secao 5 lista `SessionEnd => gone` e, entre parenteses,
"(confirmado pela ausencia do painel, regra 1b)". A spec nao diz se `SessionEnd` com painel presente leva a
`gone`. `h6_...` so afirma: o relato e aceito; depois que o painel some, `unknown` com `error` ate
`gone_after_s` (5 s) e `gone` com folga de 2,5 s. Nenhum teste afirma o caso com painel presente.

**A18. Estado antes do primeiro relato.** A spec nao diz qual e o estado de um agente com `claude_hooks`
configurado e sem relato ainda (E ausente). Os testes de F1 e F4 fazem um relato antes de afirmar.

**A19. Idade de `E` para o override.** A regra 5 diz `idade(E) >= override_after_s`, com `idade` medida "desde o
ultimo relato de E". O teste mede a partir do instante do relato (um pouco depois do `t0` do teste). A janela
de checagem nao e exata (checa `working` em 3,5 s e 4,5 s de 5 s, e `idle` depois).

**A20. `evidence` no modo so tela.** A secao 2 e a rev5 dizem `evidence: observed`. F5 afirma isso.

**A21. CONTRADICAO/AMBIGUIDADE: localizacao de `screen` em `status`.** A secao 3 diz "contadores de saude da
tela em `status`", e a secao 8 escreve "`status` ganha `sources.app_server`, `sources.hooks` ... e `screen`".
O daemon pode pôr `screen` em `status.screen` ou em `status.sources.screen`. F6 aceita as duas. Forma dos
contadores: literal da secao 3 (`screen.reads.<kind>`, `screen.unmatched_ratio.<kind>`), com a razao em
fracao (0 a 1).

**A22. Codigo de saida de configuracao recusada.** A spec diz "nao sobe (saida 1)" (rev4 secao 13 e rev5 secao
6). F2b afirma saida 1 em toda recusa.

**A23. P2 com eventos desligados.** P2 usa `events: false`. O `-C attach` da rev5 (secao 4A) nao e afirmado
aqui; R20 da rev5 cobre a forma do processo.

**Nao cobertos (fora dos itens S/H/F/P ou sem como afirmar sem a implementacao):**
- `hooks_silence_s` (secao 5, relato velho => `structured_lost` com tela de trabalho): so a faixa e testada (F2b).
- `screen_health_window_s`: so a faixa e testada (F2b); a janela deslizante nao.
- Combinacoes de fontes recusadas (secao 3: duas estruturadas, `codex_app_server` em `claude`, sem `tmux`).
- Campos de evento `source`/`evidence` em `Evento` (regra 7): so `agent.get` e `status` sao lidos.
- Dono do `thread_file` (A10) e `codex_socket` padrao (`~/.codex`): nunca exercitado, de proposito.
- Descendente de `report` e concorrencia de varios relatos: nao testados.

## Resultado

Rodada final (2026-10-16), contra o binario e o `src/` que existiam no momento da rodada. O outro agente
ainda estava alterando o `src/`, e o `cargo fmt --check` do crate apontou diferenca em `src/app_server.rs`
(nao e dos testes). Os testes desta rodada passaram todos, exceto:

- `s6_thread_fora_da_lista_de_carregadas_e_structured_lost`: falhou uma vez na rodada em paralelo com as
  outras suites; depois passou 5 de 5 (3 isolados, 2 do arquivo inteiro). Causa nao identificada; pode ser
  tempo de resposta sob carga (prazo de 3 s). Registrado, nao corrigido.

Falhas que apareceram em rodadas anteriores (e que mudaram conforme o `src/` mudou): `s3_status_de_erro_...`
(A5), `s3b_not_loaded_...` (A4), `s3c_active_flag_inesperada_...` (A6) e `f6_...` (A21, era erro meu de
localizacao, corrigido). As tres primeiras sao divergencias do contrato literal, dependendo do comportamento do
`src/` em cada rodada.

Verificacao de compilacao: `cargo test -p agentd --offline --test rev6_codex --test rev6_hooks --test
rev6_fusao --test rev6_config_privacidade --no-run` compila sem avisos. `cargo fmt -p agentd -- --check` passa
para os arquivos de teste (a falha restante e em `src/app_server.rs`, do implementador).

**Nao validado:** os testes nunca foram rodados contra uma implementacao quebrada de proposito (teste de
mutacao). Ausencia de falso positivo e garantida so pelos controles positivos dentro de cada teste.

## Rodada 2: lacunas da mutacao (`tests/rev6_lacunas.rs`)

| Teste | Item da spec | Ambiguidade ou suposicao |
|---|---|---|
| `l1_override_conta_a_idade_do_ultimo_relato_working_nao_so_a_tela` | secao 6, regra 5: `idade(E) >= override_after_s` | A24 |
| `l2_override_conta_a_tela_idle_desde_que_ela_ficou_idle_nao_o_relato` | secao 6, regra 5: "T idle por ... esse tempo todo" | A25 |
| `l3_relato_idle_logo_apos_troca_de_pid_nao_publica_idle_antes_da_segunda_leitura` (rodada 3) | secao 6, regra 4 (esclarecimento rev6.2) | A26 (decidida), A27 |

Controles positivos: L1 termina em `idle` com `screen_override` depois de parar os relatos (3,5 s working, depois
idle ate 8 s); L2 termina em `idle` com `screen_override` depois de 5 s de tela idle; L3 publica `idle` com o
mesmo tipo de relato depois de 2,8 s de par novo estavel (**so roda se a parte negativa passar**; nesta rodada
nao rodou).

**A24. Idade do override (L1).** A regra 5 mede `idade(E)` a partir do ultimo relato `working`. Relatos repetidos
a cada 1 s mantem a idade abaixo de `override_after_s` (5 s), entao o estado deve continuar `working` mesmo com a
tela idle positiva desde o boot.

**A25. "Esse tempo todo" (L2).** A regra 5 diz que a tela deve estar idle "por esse tempo todo". Lido aqui como:
a tela idle positiva ininterrupta por `override_after_s`, contada a partir de quando ela ficou idle. Por isso, com
relato velho (mais de 5 s) e a tela virando idle, o override nao e imediato. Leitura alternativa (a idade de E
ja conta) daria override imediato e falharia L2.

**A26. CONFIRMACAO DO TERMINAL (L3): DECIDIDA na rev6.2 (ver rodada 3).** A regra 4 da rev6 exige "a confirmacao da rev5 sobre
o terminal (mesmo par pane_id/pane_pid), mas nao as duas leituras de tela". A rev5 3' manda que, depois de troca de
par, o estado fique `unknown` com `error` ate uma leitura valida do novo par, e que `idle` volte pela regra 5'. A
5' tem tres condicoes: (i) duas leituras idle (retiradas na rev6), (ii) mesmo par, (iii) `idle_confirm_ms`. O teste
le a rev6 como (ii) + (iii) mantidas: o par novo tem de durar `idle_confirm_ms` (1500 no teste) antes de `idle`.
Se a decisao for que o relato estruturado so exige o par na leitura atual (sem tempo), L3 deve ser trocado por
uma variante com uma unica leitura do par novo. **Resultado nesta rodada: L3 falha de forma estavel (3 de 3):** o
`Stop` logo depois da troca de `pane_pid` publica `idle` antes de 1,2 s. O teste nao foi enfraquecido.

**Resultado da rodada 2:** L1 3 de 3 passam; L2 3 de 3 passam; L3 3 de 3 falham na parte negativa (ver A26).
Compilam sem avisos com `cargo test -p agentd --offline --test rev6_lacunas --no-run`.

## Rodada 3: rev6.2 (`tests/rev6_lacunas.rs`)

Esclarecimentos rev6.2 do spec (secao 6, regras 2 e 4), decisao do Sextante, 2026-10-16.

| Teste | Item | Ambiguidade |
|---|---|---|
| `l3_relato_idle_logo_apos_troca_de_pid_nao_publica_idle_antes_da_segunda_leitura` | regra 4 (rev6.2): troca de par exige o mesmo par em DUAS leituras validas consecutivas (a da troca conta como a primeira), sem `idle_confirm_ms`. `poll_ms` 1000, `idle_confirm_ms` 5000 | A26 (decidida), A27 |
| `b1_tela_com_blocked_rebaixa_working_do_relato_e_volta_quando_a_tela_sai` | regra 2 (rev6.2): E working + T blocked publica blocked com `blocked_reason` da tela, `source` tmux, de imediato; ao sair, volta E. Controle: E blocked com tela idle nao e rebaixado | A28 |
| `b1_controle_tela_com_blocked_sem_fonte_estruturada_publica_blocked` | controle da B1 (rev5, so tela): `permission-bash` => blocked/permission, source tmux | - |

**A26 (decidida, rev6.2).** Troca de `pane_pid` no mesmo `pane_id`: o relato `idle` so publica `idle` depois de o
mesmo par ser lido em duas leituras validas consecutivas; a leitura em que o par mudou conta como a primeira.
Nao usa `idle_confirm_ms` (o teste poe 5000 para provar isso: o `idle` aparece bem antes de 5 s). O atraso e no
maximo um `poll_ms` (1000 no teste). O teste detecta a primeira leitura pelo contador de `list-panes` do tmux
falso (ver A27), manda um `Stop` 30 ms depois, exige que nao haja `idle` em 800 ms, e depois de 1,4 s manda outro
`Stop` e exige `idle` ate 4 s depois da troca.

**A27 (rodada 3). Como o teste sabe o instante da primeira leitura.** Com o relato em `working`, a troca de par
nao muda o estado publicado (a regra 2 da rev6 publica `working`); a rev5 3' fala em `unknown` com `error`, mas a
rev6 nao confirma isso para o caso com relato. Por isso o teste nao usa o estado: o instante da primeira leitura
e o primeiro `list-panes` depois da troca (contador do tmux falso). Se um `list-panes` ja estava em voo na troca,
o instante e um pouco antecipado; a janela negativa continua valida, porque o proximo ciclo (com o par novo) cai
ainda mais tarde.

**A28 (rodada 3). Precedencia relato x tela (B1).** A regra 2 (rev6.2) manda a tela `blocked` rebaixar `working`
do relato; o teste afirma `blocked_reason` da tela, `source` `tmux` e `evidence` `observed`. Ao sair de `blocked`
pela tela, o `working` do relato volta com `source` `hooks`. Limitacao: a regra de tela usada e a fixture
`claude/permission-bash` (regra `claude.blocked.permission`); outras classes de `blocked` (`user_input`) nao sao
cobertas aqui.

**Resultado da rodada 3:** L3 passa 3 de 3. B1 falha 1 de 1: o estado fica `working` com `source` `hooks`, com `rule`
nulo e sem marcas. O controle (mesma tela, so tmux) passa, entao a falha e da precedencia do relato sobre a tela,
nao do harness nem da regra. Compila sem avisos; `cargo fmt -p agentd -- --check` passa.

## Rodada 4: relato `blocked` x tela `blocked` (`tests/rev6_lacunas.rs`)

| Teste | Item | Ambiguidade |
|---|---|---|
| `b2_relato_blocked_nao_e_rebaixado_pela_tela_blocked_de_outra_classe` | regra 2 (rev6.2): E `blocked` e publicado como E e nunca rebaixado pela tela. Relato PermissionRequest (permission, hooks) e tela `claude/ask-user` (regra `claude.blocked.menu`, user_input). Controle no mesmo daemon: agente `claude-tela`, so tela `ask-user`, publica blocked/user_input com source tmux | A29 |

**Fixture:** `claude/ask-user` (classe `user_input`, regra `claude.blocked.menu`, usada pelas regras rev5). A classe
da tela e diferente da do relato, de proposito.

**A29 (rodada 4). `evidence` quando as classes diferem.** A secao 3 define `both` como "concordam na ultima leitura
valida". Com classes diferentes (permission do relato, user_input da tela), a leitura da tela nao concorda no motivo.
O teste nao afirma `evidence` no caso de classes diferentes. Afirma `blocked_reason` `permission` e `source` `hooks`
(o relato decide). Com a mesma classe, o esperado seria `evidence` `both`; esse caso nao foi escrito nesta rodada
(ficaria como teste separado, com a fixture `claude/permission-bash`).

**Resultado da rodada 4:** `b2_...` passa 3 de 3. Compila sem avisos; `cargo fmt -p agentd -- --check` passa.
