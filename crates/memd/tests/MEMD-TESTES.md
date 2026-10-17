# memd v0: testes independentes (caixa-preta)

Fonte: `docs/memd-spec-v0.md` (secoes 4 a 8A e 10) e `docs/blocklog-spec-v0.md` (secao 8). Escritos sem ler `crates/memd/src`.

**Estado:** compilam (`cargo test --offline -p memd --no-run`) e passam no gate `cargo fmt --check -p memd`. **Nao foram executados** (a suite nao foi rodada nesta etapa).

## Tipos de teste

- **Tipo A (biblioteca):** `memd::Service` sobre `blocklog::Log` com `ArmazenamentoFalso`/`ArquivoFalso` (disco em memoria, reabrivel, com falhas injetaveis) e `RelogioFalso` (avanca e regride). Em `common/mod.rs`.
- **Tipo B (binario):** `env!("CARGO_BIN_EXE_memd")`, `tempfile::tempdir()`, socket Unix real, SIGHUP real (utilitario `kill`), arquivos reais. Sao os testes de `binario.rs`: O1, O2, O3, O3b, O6, O7, A5 (SIGHUP), A8, M1 (32 clientes pelo socket).

## Arquivos

| Arquivo | Conteudo |
| --- | --- |
| `common/mod.rs` | fakes (Storage, LogFile, Clock), tokens de teste (`mt_` + 43), helpers de requisicao e de visao |
| `mutacoes.rs` | M1 a M11 |
| `idempotencia.rs` | I1 a I5 |
| `autorizacao.rs` | A1 a A7, A9, A10 (tipo A); A4 compara a linha inteira |
| `validacao.rs` | V1 a V9 (V9 = limites da secao 3, fora da lista da secao 10) e faixas de `Config`/`Grants` |
| `acesso.rs` | Q1 a Q7 |
| `operacao.rs` | O4, O5 (tipo A); O9 = robustez de `handle_line` |
| `binario.rs` | O1, O2, O3, O3b, O6, O7, A5 (SIGHUP), A8, M1 pelo socket (tipo B) |
| `complementos.rs` | rodada 2: a11, a12, a13, m12, a14, m13 (tipo A) |

## Tabela: teste -> item da spec -> ambiguidade ou suposicao

### mutacoes.rs (M)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| m01_propose_novo_gera_d001_rev1_proposed_seq1 | M1, s.6 | Forma exata do resultado `{id,rev,status,seq}`; `author` vem do perfil. |
| m01b_ids_por_tipo_sem_lacuna_nem_repeticao | M1 | Contador de `P-` separado do de `D-`. |
| m01c_32_propose_em_threads_... | M1 (lib) | Concorrencia por `Mutex<Service>` (o servidor serializa; a lib testa o mesmo ponto). |
| m02_revisao_com_expected_rev_errado_e_stale_rev | M2 | "rev n+1" lido como maior rev existente; teste so cria revisao com a `active` como base. |
| m02b_revisao_de_id_inexistente_e_not_found | M2 | ok |
| m03_promote_com_expected_seq_errado_e_stale_seq | M3 | `superseded: []` na primeira promocao (suposicao). |
| m04_promote_de_revisao_supersede_... | M4 | Formato de cada item de `superseded` nao definido: so o tamanho e conferido. `superseded_by` = `"id@rev"`. |
| m04b / m04c / m04d | M4 + blocklog B1/B2 | Cauda cortada no meio e sem `\n`: nada aplicado. Log completo: aplicado inteiro. |
| m05_supersedes_marca_a_active_so_na_promocao_do_novo | M5 | ok |
| m05b / m05c | M5 + blocklog G3 | Cauda cortada no promote com `supersedes`: nenhuma ponta mudou; completo: as duas. |
| m06_supersedes_de_id_inexistente_e_not_found_e_nada_muda | M6 | Tabela de `propose` nao lista `not_found`: `propose` com `supersedes` inexistente e aceito; o erro vem no `promote`. |
| m06b_supersedes_ilegivel_e_not_found_... | M6 | Mesma regra para id ilegivel. |
| m07_reject_e_retire_mudam_so_o_estado_esperado | M7 | **Transicao em estado errado = `conflict`** (spec nao define codigo para isto). |
| m08_expire_no_limite_de_proposal_ttl_days | M8 | Limite lido como instante (ts da proposta + 14 dias), nao como data UTC. Leitura por data daria outro corte. |
| m08b_dezessete_expiracoes_em_dois_commits_de_ate_16 | M8 (`ate 16 por tx`) | 17 expiracoes = 2 commits (suposicao de quebra ate 16). |
| m09_relogio_regressivo_nao_expira_antes_nem_ressuscita | M9 | ts do commit = max(agora, ultimo ts); o `tick` usa o mesmo relogio nao regressivo. |
| m10_duas_active_com_mesmo_tipo_escopo_e_titulo_sao_conflict | M10 | Trim e dobra ASCII como em s.7; controles: titulo diferente, escopo diferente, `supersedes` listado. |
| m11_max_open_proposals_padrao_20_estoura_com_quota | M11 | "abertas" = status `proposed`; promover ou rejeitar libera a vaga (suposicao). |

### idempotencia.rs (I)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| i01_mesmo_request_id_... | I1, s.6 | Primeira resposta pode ter ou nao `replayed`; compara os demais campos. |
| i01b_carga_canonica_ignora_ordem_de_chaves_e_espacos | s.6 (JSON canonico) | ok |
| i02_mesmo_request_id_com_carga_diferente_e_request_conflict | I2 | ok |
| i02b_mesmo_request_id_de_outro_agente_... | blocklog s.6 (chave = agente + id) | ok |
| i03_repeticao_depois_de_reiniciar_... | I3 | ok |
| i04_repeticao_apos_perder_leitura_... | I4 | Perda de `op` na repeticao nao testada (spec so fala de leitura). |
| i05_mutacao_sem_request_id_e_invalid_params | I5 | Formato invalido de `request_id` = `invalid_params` (codigo nao especificado para formato). |

### autorizacao.rs (A)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| a01_sem_op_e_forbidden_em_cada_mutacao_com_controle_positivo | A1 | Ordem op x existencia nao testada (so com id existente). |
| a02_write_scopes_que_nao_casam_sao_forbidden | A2 | ok |
| a03_read_scopes_que_nao_casam_nao_mostram_nem_confirmam_o_registro | A3 | `(oculto)` aplicado a `supersedes` e `superseded_by`. |
| a04_id_existente_sem_permissao_e_id_inexistente_respondem_identico | A4 | Compara a linha inteira com o mesmo id de cliente (mais forte que "exceto o campo id"). |
| a05_token_errado_expirado_ou_removido_e_unauthorized | A5 | `expires` vale no dia inteiro (UTC), inclusive; invalido no dia seguinte. |
| a06_types_restringe_leitura_e_escrita_por_tipo | A6 | **Spec s.5 so cita `types` para leitura; A6 diz "restringe". Escolhido: escrita de tipo fora de `types` = `forbidden`.** `query` de tipo fora de `types` = lista vazia (poderia ser `forbidden`). |
| a07_whoami_devolve_o_perfil_e_o_intent | A7 | Chaves exatas `{agent,ops,read_scopes,write_scopes,types,expires,intent}`. |
| a09_status_nao_revela_escopos_nem_titulos | A9 | `status` liberado a agente valido sem `op` (suposicao); chaves exatas `{head_seq,counts,broken,uptime_s}`. |
| a10_perfil_spec_only_nao_enxerga_impl_por_metodo_algum | A10 | ok |
| (A8 em binario.rs) | A8 | ver abaixo. |

### validacao.rs (V)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| v01_sem_source_... | V1 | `missing_source` para ausente. `kind` invalido, `at` invalida e `ref` vazia = `invalid_params` (codigo nao especificado). |
| v02_segredo_em_cada_campo_textual_e_secret_detected | V2 | Matriz padrao x campo (title, body, tags, criterion, source.ref, refs.ref). `ghp_`+36 tem 40 caracteres e nao cabe em tag (max 24): nao testado em tags. Cabecalho PEM, `AKIA` (maiusculas) e JWT nao cabem em tag. `xoxb-` seguido de digitos (classe do `xox[baprs]-` nao fechada na spec). Operador: `secret_patterns` = substring literal `segredo-operador`. Erro nao ecoa o segredo. |
| v02b_segredo_no_motivo_de_reject_e_secret_detected | V2 (`reason`) | ok |
| v03_body_limite_de_4096_bytes_em_utf8 | V3 | Limite em bytes UTF-8 (2048 x `é` = 4096 passa; +1 `a` = recusa). |
| v04_pending_exige_review_by_owner_next_action_e_state | V4 | Ausencia, `state` invalido, `review_by` passado ou data invalida = `invalid_params`; `review_by` = hoje passa. |
| v05_campos_desconhecidos_e_reservados_do_cliente_sao_recusados | V5 | `invalid_params`; `fields` fechado, tambem para `decision` com campo de `pending`. |
| v06_review_by_vencido_vira_unconfirmed_... | V6 | Vencido = data anterior a hoje UTC; no proprio dia ainda nao. Linha da view `id \| title \| scope \| review_by`. |
| v07_pendencia_done_sai_da_view_... | V7 | Revisao com `state=done`, promovida. `state=done` explicito mostra. |
| v08_config_aceita_as_faixas_... | faixas s.7 / 8A | Chave desconhecida recusada. Chaves ausentes: nao testado (tests mandam config completa). |
| v08b_grants_malformado_... | 8A `Grants::from_json` | **Formato de `grants.json`: spec diz "lista de {...}"; escolhido array no topo.** |
| v08c_token_de_teste_tem_mt_e_43_base64url | s.5 | Apenas valida os tokens fixos de teste. |
| v09_limites_de_campos_da_secao_3_... | s.3 (fora de s.10) | Limites de title, scope, tags, supersedes, refs, criterion = `invalid_params` (codigo nao especificado). Round trip de `refs` e `source`. |

### acesso.rs (Q)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| q01_query_ordena_por_tipo_e_id_e_pagina_por_after_sem_pular | Q1 | `next_after: null` na ultima pagina (suposicao). |
| q01b_paginacao_nao_pula_item_... | Q1 | Itens novos podem aparecer; so os existentes durante toda a leitura sao exigidos. |
| q02_term_dobra_so_a_z_em_titulo_e_tags | Q2 | Termo com acento nao e dobrado; titulo tambem nao (`DECISÃO` nao casa `Decisão`). |
| q03_limit_fora_de_1_a_50_... | Q3 | `limit` 0 = `invalid_params`; padrao 20 (`next_after` = 20o id). |
| q04_view_nunca_excede_view_budget_... | Q4 | Orcamento contado em caracteres do `text` (spec: texto e unica string da visao). `trust:"data"`. |
| q05_item_maior_que_o_orcamento_restante_... | Q5 | A view nao tem `after`: o teste continua com `query(after=next_after)` (suposicao). |
| q06_secao_due_respeita_view_overdue_max_... | Q6 + s.6 (secoes disjuntas) | Pendentes nao vencidas so em `active`; vencidas alem do limite so em `active`. |
| q06b_view_overdue_max_1_... | Q6 | ok |
| q07_resposta_maior_que_response_max_... | Q7 | `response_max` = 1024 na config de teste; a faixa de `response_max` nao esta na spec. Mutacao grava; so a resposta vira `overloaded`. |

### operacao.rs (O4, O5, O9)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| o04_estado_reconstruido_... | O4 | 200 operacoes com semente fixa e relogio que avanca e regride. Compara listagens por status (6 status) com paginacao e `get` por versao. Assume que `query` aceita os 6 status. |
| o05_falha_injetada_torna_broken_... | O5 + blocklog B10-B13 | Falhas: EIO/ENOSPC/escrita curta na escrita, EIO/ENOSPC no `sync_data`. Codigo `broken` (s.6). Nada aplicado. |
| o09_handle_line_nao_entra_em_panico_... | 8A (linha 97) | **Linha maior que `line_max` = `invalid_params`, conforme 8A. A s.5 diz "estourar = quota/overloaded": contradicao; adotado 8A.** Controle: linha de exatamente `line_max` passa (espacos dentro do JSON). Entradas ruins = `invalid_params` (envelope, metodo desconhecido, params nao objeto). |

### binario.rs (tipo B)
| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| o01_init_cria_modelo_sem_tokens_... | O1 | Saida do `init` nao especificada (so os arquivos). Modo 0700. Config e grants de modelo validos pela lib. Nao sobrescreve. |
| o02_validate_aceita_log_valido_... | O2 | Log real (`Log::open_dir` + `SystemClock`). Corrupcao por edicao do `tx`: **depende de `tx[0]["id"]` e `tx[0]["supersedes"]` do evento `propose`** (pre-condicao com `assert`; se o esquema divergir, o teste falha com mensagem). Duas `active` do mesmo id nao testado: nao ha forma de produzir log que o replay aceite sem o esquema normativo dos eventos. |
| o03_token_nao_aparece_... | O3 | Sentinela `mt_Sentinela...` ausente de respostas, saida do servidor e `log.jsonl`. |
| o03b_token_hash_le_do_stdin_... | O3 / s.8 | Entrada sem `\n` final (fim de linha final nao especificado). Saida = hash puro (trim). |
| o06_socket_padrao_fica_no_estado_... | s.6 (socket) | **Caminho padrao `<estado>/memd.sock` e suposicao** (spec so diz "memd.sock"). Socket 0600, estado 0700. |
| o07_serve_recusa_partida_... | s.7 / 8A | Recusa para `proposal_ttl_days` 0, `view_budget` 999, `view_overdue_max` 21 e chave desconhecida. `grants` invalido nao testado. Controle com config valida sobe. |
| a05_sighup_revoga_token_mesmo_em_conexao_ja_aberta | A5 + SIGHUP | Recarregamento assincrono: poll ate 10 s na mesma conexao (spec diz "na hora"). |
| a08_nenhuma_operacao_altera_grants_json | A8 | Bytes e mtime iguais depois de propose, promote, reject, retire, get, query, view, whoami, status e token errado. |
| m01_socket_32_clientes_simultaneos_geram_d001_a_d032 | M1 (socket) | `max_connections` 32; barreira para conectar todos antes de enviar. |

## Contradicoes e lacunas da spec (registro)

1. **Linha acima de `line_max`:** s.5 diz "estourar = `quota`/`overloaded`"; 8A (linha 97) diz `invalid_params`. Adotado `invalid_params` (decisao do coordenador).
2. **`types`:** s.5 cita so para leitura; A6 diz "restringe". Escolhido: vale tambem para escrita.
3. **Codigo para transicao em estado errado** (`reject`/`retire`/`promote` sobre versao que nao esta `proposed`, ou `retire` sobre `proposed`): tabela de s.4 nao define. Escolhido `conflict`.
4. **`validation`** aparece na coluna de erros de `promote` (s.4) sem condicao nomeada. Nao testado.
5. **`supersedes` de id existente mas nao `active`** no `promote`: a tabela pede `active`, o codigo nao esta definido. Nao testado.
6. **Formato de `grants.json`:** "lista de {...}". Escolhido array no topo.
7. **Formato de `view`:** agora normativo (`{text, trust, view_truncated, next_after, head_seq}`); testes usam so `text`; secoes `due:`/`active:` disjuntas.
8. **`view` nao tem `after`:** o cursor so se usa via `query`.
9. **Expiracao:** limite como instante ou como data. Teste usa instante exato (14 dias).
10. **`response_max` e `line_max`:** faixas nao especificadas; os testes usam 1024 e 16384.
11. **`status` sem `op`:** liberado a agente valido (suposicao).
12. **Caminho padrao do socket:** nao especificado; testado como `<estado>/memd.sock`.

## Rodada 2 (complementos.rs)

Lacunas apontadas pela rodada de mutacao. Todos passam no codigo atual (rodados 3 vezes, so este arquivo).

| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| a11_glob_a_barra_casa_o_proprio_a_e_os_de_baixo | A3 / glob `a/**` (s.5) | `a/**` = `a` e tudo abaixo. Controle: `a/x` le; `b` nao. |
| a12_glob_a_barra_nao_casa_prefixo_sem_barra | A3 / glob | `ab` e `a-b` nao casam `a/**`. Query do leitor lista so o legivel. |
| a13_token_com_prefixo_diferente_de_mt_e_unauthorized_... | s.5 (token `mt_` + 43) | O SHA-256 do token `xx_` esta no grants.json; so o formato o barra. Controle: o `mt_` correspondente autentica. Suposicao: a spec exige o prefixo `mt_` na validacao. |
| m12_max_records_fronteira_exata | s.5 (`max_records`) | Fronteira: com 3, a 3a passa e a 4a = `quota`; com 1, a 1a passa e a 2a = `quota`. Conta registros novos (`propose` sem `id`). |
| a14_promote_com_supersedes_de_alvo_legivel_e_nao_gravavel_e_not_found | M6 / A4 / s.5 (escrita nas pontas de supersedes) | Alvo legivel e nao gravavel = `not_found`, resposta linha-a-linha igual a de id inexistente, nada gravado. Controle: quem escreve no alvo promove. |
| m13_log_max_bytes_estourado_e_quota_sem_broken_e_com_limite_folgado_passa | blocklog s.5 (`log_full`, sem broken) | **`log_full` mapeado para `quota`** (escolha do coordenador; s.6 nao lista esse codigo). A checagem e por linha: linha pequena que cabe ainda passa. **A faixa minima de `log_max_bytes` e 1 MiB (blocklog s.8)**: nao da para ter "uma ou duas linhas"; o teste usa propostas de 4 KiB (~226 commits). Controle: 300 commits com limite padrao passam. |

## Nao coberto nesta rodada (e por que)

- **CLI cliente** (`memd propose|...|status --token-file F`): a s.8 nao diz como o cliente acha o socket (nao ha flag `--socket` no cliente). Sem isso o teste seria um chute. Pendente de definicao.
- **Expiracao a cada 60 s e na partida** (s.4): so `tick` e testado.
- **`due` opcional de `pending`** (s.3.1) e **`refs` como filtro**: nao especificados em teste.
- **`Service::new` com log invalido (`Err`)**: sem esquema normativo dos eventos `tx`.
- **Excesso de conexoes** (`max_connections`): comportamento nao especificado.
- **`grants.json` invalido na partida**: nao testado.
- **Duas `active` do mesmo id** no `validate`: ver o teste `o02`.
