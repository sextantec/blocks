# grantd v0: testes caixa-preta (G1 a G59)

Escritos contra `docs/grantd-spec-v0.md` (rev1.2: secao 5A prevalece; 6A normativa; secao 10 com G38 a G55), so pela API publica: `grantd::{Config, Grants, Service, token_sha256_hex}`, `blocklog::{Log, Options, Storage, LogFile, Clock}` e o binario `grantd` (`CARGO_BIN_EXE_grantd`). Nenhum teste le `crates/grantd/src`, `crates/blocklog/src`, `chatd`, `agentd` ou `memd`.

Estado: **compila** (`cargo test --offline -p grantd --no-run`, exit 0), `cargo fmt --check -p grantd` passa e clippy nao aponta aviso nos arquivos de teste (o unico aviso do pacote esta em `src/lib.rs`, fora do escopo). **Nao foi executado**: a suite e a verificacao de comportamento ficam para quem integra o crate.

Arquivos:
- `common/mod.rs`: fakes em memoria (`Disco`, `ArmazenamentoFalso`, `ArquivoFalso`, `Relogio`), abertura de `Log` + `Service`, tokens de teste (`gt_` + 43 caracteres), perfis, envelope e helpers.
- `decisao.rs` (G1 a G11, G35, G38, G39, G54), `limites.rs` (G12 a G14, G36, G40 a G42), `poder.rs` (G15 a G21, G37, G43, G52, G53, G55), `delegacao.rs` (G22 a G27, G44 a G48, G56 a G58), `durabilidade.rs` (G28, G33, G49, G59), `binario.rs` (G29 a G32, G34, G50, G51 e os auxiliares `init` e `token-hash`).

Tipos: **A** = biblioteca (`Service::handle_line` com disco e relogio falsos). **B** = so o binario (socket Unix real, SIGHUP, concorrencia, arquivos temporarios, codigos de saida).

Regras aplicadas: todo negativo tem controle positivo no mesmo cenario; oraculos exatos (`decision`, `reason`, `grant`, codigo de erro, `head_seq`, bytes do log); o envelope de cada resposta e conferido (uma linha, `v` = 1, `id` ecoado).

## Convencoes assumidas

Os nomes de campos, resultados, codigos e fronteiras seguem a secao 5A (rev1.2), 6A e a secao 10. Abaixo, so o que nenhuma delas fixa.

1. **Envelope**: `{"v":1,"id":"...","token":"gt_...","method":"...","params":{...}}`; resposta `{"v":1,"id":"...","ok":true,"result":{...}}` ou `{"v":1,"id":"...","ok":false,"error":{"code":"...","message":"..."}}` (6A).
2. **grants.json**: lista na raiz; `expires` = data `YYYY-MM-DD` (fim do dia, UTC).
3. **Relogio**: o instante de decisao e `t_max` = maximo entre o relogio observado (leituras incluidas) e o ultimo `ts` do log (5A.6). Validade `[not_before, until)`. Janela de `max_amount`: `ts >= agora - window_s`.
4. **Binario**: usa o relogio real; os testes B usam `until` relativo ao agora real.
5. **Socket da CLI**: os testes passam `--socket` ao `check`; a spec nao diz o caminho padrao.
6. **Relogio do `Service`**: `Service::new` recebe um `Clock` separado do `Log::open`; os testes passam o mesmo relogio nos dois (helper `sobe_com`).
7. **Formato de `t_max`** em `status` e `validate --json`: string RFC 3339 (o teste compara instantes).
8. **`validate --json` com falha**: a saida `{"ok":false,"error":"..."}` vai para a saida padrao.
9. **`list` com cursor**: `next_after` e o id do ultimo item entregue, e `null` quando a pagina nao esta cheia (5A.15). Ids `G-...` e `R-...` sao opacos; a ordem e a do numero.

## Tabela: teste -> tipo -> item -> ambiguidade ou suposicao

Na coluna "Item", `G#` e o item do contrato e o numero depois da virgula e a secao da spec.

| Teste | Tipo | Item | Ambiguidade ou suposicao |
| --- | --- | --- | --- |
| g01_sem_concessao_e_deny_no_grant | A | G1 | `grant` null no deny. Controle: a concessao vira allow. |
| g02_allow_casando_devolve_o_id_e_o_until | A | G2 | `until` comparado como instante; `signed:false`. Controle: a2 sem concessao = deny. |
| g03_deny_vence_allow_nas_duas_ordens | A | G3, 6A | `denied_by` = id da deny (5A.5: menor id entre as deny). Duas ordens de criacao. |
| g04_until_no_passado_e_deny_no_grant | A | G4, 6A | `until` exclusivo: em `t = until` ja nao e ativa. Controle: um segundo antes, allow. |
| g05_not_before_futuro_e_deny_no_grant | A | G5, 6A | `not_before` inclusivo. Controle: no instante de `not_before`, allow. |
| g06_glob_por_segmentos_doze_caminhos | A | G6, 4 | Tabela de 12 casos. Cada negativo tem amostra positiva no mesmo cenario. Nao testado: `a*` (glob parcial) e `/**` (raiz), que a spec nao define. |
| g07_resource_nao_canonico_em_check_e_invalid_params | A | G7, 4 | `..`, `//`, relativo, `/` final e glob em `check` = `invalid_params`. `/` e canonico. Nao testado: resource nao canonico em `grant`. |
| g08_grantee_curinga_vale_para_qualquer_agente | A | G8 | Nenhuma. Controle: concessao so de a1 nao vale para a2. |
| g09_acao_fora_da_lista_e_invalid_action_nao_deny | A | G9, 4 | `invalid_action` como erro de `check`. Controle: acao valida sem concessao = deny. |
| g10_relogio_regressivo_nao_reviva_concessao_nem_reabre_janela | A | G10, 5A.6 | `t_max` nao regride: concessao vencida nao volta com o relogio em 0 s; o `ts` de commit feito com o relogio em 0 s e o ultimo (61 s). |
| g10b_relogio_regressivo_nao_fecha_nem_reabre_janela | A | G10, 5A.6 | O `check` com o relogio voltado responde como em `t_max` (61 s). |
| g11_explain_lista_as_que_casaram_e_a_regra | A | G11, 6A | `matched` = `{id, effect, active, why}`: conjunto exato de ids; `effect` e `active` conferidos; `denied_by` = id da deny. `why` so presente (texto livre). |
| g12_max_uses_conta_so_use_e_check_nao_consome | A | G12, 5A.2 | `use` = `{...check, replayed}`. `use` negado = `denied`. `check` nao consome. |
| g13_max_amount_com_janela_deslizante | A | G13, 5A.3 | Janela com borda fechada (`ts >= agora - 60`): em t=60 ainda conta o uso de t=0; em t=61 volta a permitir. Quantidades inteiras. |
| g14_use_repetido_consome_uma_vez_e_carga_diferente_conflita | A | G14, 5A.2 | `replayed:true` na repeticao; nada gravado; carga diferente = `request_conflict`. |
| g15_agente_nao_cria_nega_revoga_nem_aprova | A | G15, 6A | `forbidden` nas quatro operacoes de agente; `head_seq` nao anda. Controle: o dono faz as quatro. |
| g16_sem_until_e_sem_permanent_e_missing_until | A | G16 | `missing_until` tambem para `deny_rule`. Controle: com `until` passa; `permanent` sem `until` passa. |
| g17_permanent_aparece_destacado_no_list_do_dono | A | G17, 5A.5 | `permanent: true`, `status: active`, `until: null` no item do `list`. |
| g18_request_cria_pendente_e_nao_concede | A | G18, 6A | `request` = `{id, status:"pending", seq}`. Controle: aprovado, o check vira allow. |
| g19_approve_gera_concessao_no_mesmo_commit_e_nao_excede_o_pedido | A | G19, 6A | Um commit so (`approve` + `grant`, `issuer` = owner). `until` aprovado maior = `invalid_params`. |
| g20_mais_de_cinco_pendentes_e_quota | A | G20, 5A.10 | `max_pending_requests` = 5. `reject` libera vaga. |
| g21_show_e_list_alheios_iguais_a_inexistente | A | G21, 6A | `show` alheio x inexistente: resposta crua igual, trocando id da requisicao, ids `G-...` e id pedido. `list` do agente: comparado sem `head_seq` (o list traz estado global, 6A). |
| g22_delegacao_acima_do_pai_e_exceeds_parent | A | G22, 5A.8 | **Assumido**: filho com `action` diferente da do pai = `exceeds_parent` (5A.8 diz "acao igual", sem codigo). Recurso fora, `until` maior e `constraints` maiores = `exceeds_parent`. Controle: delegacao contida passa. |
| g23_pai_permanente_exige_until_finito_no_filho | A | G23, 5A.8d | `missing_until` (6A). Controle: com `until` passa. |
| g24_profundidade_3_e_exceeds_parent | A | G24, 6A | Dono -> a1 (prof. 1, delegavel) -> a2 (prof. 2, nao delegavel). Delegar a partir de prof. 2 = `exceeds_parent`. |
| g25_quarto_filho_e_quota | A | G25, 5A.8g | `quota` no 4o filho do mesmo pai. Controle: outro pai aceita. |
| g26_revoke_em_cascata_e_um_commit_so_e_cauda_cortada_nao_aplica | A | G26, 5A.8h | Um commit com 3 eventos `revoke`. Cauda cortada (1 e 20 bytes): nada aplicado, log volta ao anterior. Caso completo: tudo `no_grant`. Formato do evento `revoke` (campo `id`) nao esta na spec. |
| g27_delegable_do_filho_respeita_a_profundidade_restante | A | G27, 6A | `delegable:true` sem profundidade restante = `invalid_params`. Controle: sem `delegable` passa. |
| g28_reconstruir_do_log_igual_ao_vivo_apos_150_mutacoes | A | G28 | Semente fixa. Cada resposta `ok` de mutacao = um commit (`head_seq` = numero de respostas ok). Apos reabrir, `list` do dono e do a1 sao bytes iguais. |
| g29_sighup_invalida_token_em_conexao_aberta | B | G29 | Tres casos num SIGHUP (removido, expirado, token trocado) na mesma conexao. Recarga assincrona: repete ate 5 s. |
| g30_nenhuma_api_altera_perfis | B | G30, 6A | Dez nomes de metodo de gestao de perfil = `invalid_params` (metodo desconhecido, 6A). `grants.json` byte a byte igual. |
| g31_token_e_reason_nao_vazam_para_diagnostico_status_whoami_ou_erro | B | G31, 5A.13 | Sentinelas de token e de motivo. Log de diagnostico = stderr. `whoami` = `{agent, role, can_request, can_delegate, expires}` (5A.13). |
| g32_validate_pega_active_sem_until_delegacao_fora_do_pai_e_cascata_incompleta | B | G32, 6A | Controle: `validate` sai 0 e imprime `ok t_max=<RFC3339>` (`t_max` = ultimo ts do log); `--json` = `{"ok":true,"t_max":...}`. Cada corrupcao sai 4; `--json` = `{"ok":false,"error":...}`. Formato dos eventos corrompidos e assumido. |
| g33_falha_de_escrita_deixa_broken_e_check_continua | A | G33, 5A.7 | EIO e ENOSPC. Em `broken`: `check` = `deny` `reason:"broken"` (nao mais allow); `status.broken` true e `uptime_s` numerico; `list` segue. Nao coberto: falha em `sync_data` (G2 do blocklog). |
| g34_check_com_daemon_fora_sai_5 | B | G34, 6A, 7 | `check` exige `--token-file`. Socket inexistente = saida 5. Controle: com daemon no ar, `allow G-...` sai 0 e `deny no_grant` sai 3. Flag `--socket` assumida (convencao 5). |
| g35_agente_nao_consulta_outro_agente_dono_consulta | A | 5A.1, 6A | `check` e `explain` de a1 sobre a2 = `forbidden`. Controles: a1 sobre si; a2 sobre si; dono sobre a2 = allow com o id. |
| g36_use_sem_qty_em_concessao_com_max_amount_e_invalid_params | A | 5A.3, 6A | `use` sem `qty` em concessao com `max_amount` = `invalid_params`, sem commit. Controle: com `qty` e `unit` passa. |
| g37_reason_e_source_ref_com_segredo_sao_secret_detected | A | 5A.11, 6A | `reason` do revoke e `source.ref` com `AKIA` + 16 de `[0-9A-Z]` = `secret_detected`, sem commit. Controle: motivo limpo passa. |
| g38_use_e_check_so_do_proprio_principal | A | 5A.1 | `use` com `agent` de outro = `forbidden`, tambem o do owner (**assumido**: 5A.1 diz "use e sempre do proprio principal" sem codigo para o owner). `check` de a1 sobre a2 = `forbidden`; o owner consulta a2 (allow). Recusas nao gravam. |
| g39_request_id_igual_de_principais_diferentes_sao_chaves_independentes | A | 5A.1 | Mesmo `request_id` em a1 e a2: a2 nao recebe o resultado de a1 e nao e recusado; cada repeticao devolve o historico do proprio principal. |
| g40_use_repetido_apos_revoke_devolve_historico_e_nao_grava | A | 5A.2 | Repeticao apos `revoke` devolve a resposta historica (`replayed:true`, mesmos `decision`, `reason`, `grant`, `until`), sem commit. Controle: `use` novo = `denied`. |
| g41_qty_e_max_amount_inteiros_unit_obrigatoria_e_check_sem_qty | A | 5A.3 | `max_amount` e `qty` nao inteiros, negativos, fora de `0..9007199254740991` ou string = `invalid_params`; `unit` ausente ou divergente = `invalid_params`. `check` sem `qty` = qty 0: allow com saldo, `exhausted` com saldo 0. **Assumido**: `check` com `qty` 0 e saldo 0 = `exhausted`. Controle: limite exato aceito; `qty` 0 nao consome. |
| g42_grantee_curinga_compartilha_o_saldo_da_concessao | A | 5A.4 | Saldo de `grantee:"*"` e da concessao (a1 6 + a2 4 esgotam; a3 negado). Controle: concessao propria de a3 tem saldo proprio. |
| g43_empates_e_permanent_por_regra_fixa | A | 5A.5 | Entre allow: `until` menor vence `permanent`; empate de `until` = menor id (comparacao de string). `denied_by` = menor id entre deny. `permanent` + `until` = `invalid_params`. Controle: permanente sozinha = allow com `until` null. |
| g44_filho_consome_e_debita_o_pai | A | 5A.4, 5A.8f | Pai `max_uses` 1, tres filhos herdam: so um `use` passa; vizinhos = `denied`; check = `exhausted`. Controle: check antes do uso = allow. |
| g45_deny_do_delegante_barra_a_delegacao | A | 5A.8c | **Assumido**: delegacao barrada por deny do delegante = `forbidden` (a spec nao nomeia o codigo). Controle: recurso nao negado passa. |
| g46_glob_no_filho_grantee_curinga_e_pai_curinga_nao_delegam | A | 5A.8a,b, 6A | Recurso do filho com glob = `invalid_params`; grantee do filho `*` = `invalid_params` (**assumido**: codigo nao nomeado); pai com grantee `*` = `exceeds_parent` (nao delegavel). Controle: filho concreto passa. |
| g47_revoke_alcanca_future_e_max_children_conta_revogados | A | 5A.8g,h | Revoke do pai: 3 eventos (pai, filho ativo, filho `future`); check do filho = no_grant. `max_children` conta o filho revogado: 4o = `quota`. Controle: outro pai aceita o 3o. |
| g48_constraints_omitidas_herdam_as_do_pai | A | 5A.8e | Filho sem constraints herda `max_amount` 10 (11 negado, 10 aceito). Valor maior = `exceeds_parent`. **Assumido**: `unit` diferente no filho = `exceeds_parent`. Controle: valores menores passam. |
| g49_broken_e_fail_closed_mas_leitura_segue | A | 5A.7 | `check`, `explain` = `deny` `reason:"broken"`; `use` = `denied` (**assumido**: o reason de `use` nao aparece no envelope de erro). `show`, `list`, `status` seguem ok. |
| g50_sighup_com_grants_invalido_mantem_perfis_anteriores | B | 5A.13 | `agent` duplicado e `role` invalido: perfis anteriores mantidos (a1 segue allow), erro registrado no stderr (crescimento do log de diagnostico, texto nao verificado). Controle: arquivo valido sem a1 e recarregado (unauthorized). |
| g51_clientes_concorrentes_disputam_uma_unidade | B | 5A.2, G51 | 8 conexoes concorrentes, `max_uses` 1: exatamente 1 `allow` (replayed false) e 7 `denied`. Controle: `max_uses` 8 = 8 allow em concorrencia, o 9o = `denied`. |
| g52_list_paginado_com_cursor_string | A | 5A.15 | `limit` 3 com 5 concessoes e 3 pedidos: concessoes por numero, depois pedidos; `after` = ultimo id entregue; `next_after` null no fim (pagina nao cheia). `limit` 0 ou 51 = `invalid_params`. Controle: `limit` 50 entrega tudo. |
| g53_pedido_vencido_nao_conta_e_conflitos_de_estado | A | 5A.10 | Pedido com `until` passado nao conta na quota. `approve`/`reject` de nao `pending` = `conflict`; `revoke` de revogada = `conflict`. Controle: 5 pendentes validos, o 6o = `quota`. |
| g54_leitura_observada_avanca_t_max_sem_commit_e_nao_regride | A | 5A.6 | Leitura em 61 s (sem commit) faz `t_max` 61; relogio em 0 s continua deny. `status.t_max` = 61 s (formato assumido, convencao 7). Controle: concessao criada depois vale. |
| g55_task_ref_e_reason_de_pedido_com_segredo_sao_secret_detected | A | 5A.11 | `task_ref` com `AKIA...` e `reason` do `request` com segredo = `secret_detected`, sem commit. Controle: textos limpos passam. |
| g56_delegar_de_concessao_nao_delegavel_e_exceeds_parent | A | G22, 5A.8a, 6A | Pai do dono com `delegable:false` (profundidade 0): delegar = `exceeds_parent`. Controle: o mesmo pai com `delegable:true` delega e o filho decide allow. |
| g57_delegar_concessao_alheia_e_forbidden_e_pai_curinga_exceeds | A | 5A.8b, 5A.16 | a2 (com `can_delegate`) delegando concessao cujo grantee e a1 = `forbidden`; recusa nao grava. Pai com grantee `*` = `exceeds_parent` (5A.8b). Controle: a1 delegando a propria passa. |
| g58_window_s_divergente_no_filho_e_exceeds_parent | A | 5A.8e, 5A.16 | Pai com `window_s` 60: filho com 30 = `exceeds_parent`; filho sem `window_s` = `exceeds_parent` (**leitura assumida**: a ausencia de `window_s` e "vida da concessao", diferente de 60 s, portanto divergente; a 5A.8e diz "herdam" so para constraints ausentes). Controle: janela 60 igual passa. |
| g59_use_em_broken_tem_message_broken_e_check_reason_broken | A | 5A.7, 5A.16 | Em `broken`, `use` = `denied` com `error.message` exatamente `"broken"`; `check` = `deny` com `reason:"broken"`; `use` nao grava. Controle: antes da falha, `use` passa e `check` e allow. |
| aux_init_cria_estado_sem_tokens_e_e_idempotente | B | 7 | Diretorio 0700; `LEIA-ME.md`; nenhum arquivo contem `gt_`; segunda execucao sai 0 sem mudar o `LEIA-ME.md`. |
| aux_token_hash_bate_com_a_biblioteca | B | 6A (`token-hash`) | Token pela entrada padrao (6A). Saida = SHA-256 em hex. Controle: outro token tem outro hash. |

## Lacunas que sobram na spec

1. **Codigo para deny do delegante (G45), para grantee `*` no filho (G46) e para `unit` divergente no filho (G48)**: assumidos (`forbidden`, `invalid_params`, `exceeds_parent`); a spec nao os nomeia.
2. **`action` diferente no filho (G22)**: a 5A tira `delegate` da lista de acoes e diz "acao igual" sem codigo; o teste assume `exceeds_parent`.
3. **`use` do owner em nome de terceiro (G38)**: codigo assumido `forbidden`.
4. **`reason` de `use` em `broken` (G49)**: a 5A diz `deny` com `reason:"broken"`; `use` negado e erro `denied` (6A). O reason nao e verificavel no envelope de erro.
5. **`check` com `qty` 0 e saldo 0 (G41)**: assumido `exhausted`.
6. **Socket da CLI**: flag `--socket` e caminho padrao nao estao na secao 7.
7. **Relogio duplo** (`Service::new` e `Log::open`): a 7A pede os dois; o contrato pode querer um so.
8. **Glob parcial e raiz**: `a*` e `/**` (casa a raiz?) nao estao definidos.
9. **Resource nao canonico em `grant`**: so `check` esta testado.
10. **Validade por data em `expires`**: fim do dia (UTC) nao tem teste de fronteira.
11. **Formatos de eventos do log** (`revoke` com `id`, `grant` com `until`, `delegate` com `parent`): usados por g26, g32 e g47; a spec nomeia campos de resposta, nao de evento.
12. **Campos de `show`, `delegate` e `explain.why`**: so `id`/`effect`/`active` conferidos.
13. **Falha em `sync_data`** (G2 do blocklog): indeterminada pela spec; nao coberta.

## Nao coberto (e por que)

- `max_connections` e concorrencia alem de 8 conexoes: a spec nao define limite para teste.
- Permissoes de arquivo do estado (0700/0600): coberto pelo `blocklog`.
- `watch`, `export` e hook do Claude: fora da v0.
