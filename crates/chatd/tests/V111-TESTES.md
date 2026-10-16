# V111-TESTES: testes independentes da v1.11 (canal codex-queue)

Escritos pelo Haiku 5.5 (agente de testes) a partir de `crates/chatd/docs/spec-v1.11-canal-codex-queue.md`,
secao "Contrato testavel" (Q1 a Q17) e a "Emenda 1" (E1 a E6, Q19 a Q21), sem ler `src/`.
Arquivo de teste: `tests/v111_codex_queue.rs`. Daemon real em diretorio temporario; `codex`, `herdr` e `tmux`
FALSOS (scripts shell) registram o argv de cada chamada. Sem codex, tmux ou herdr reais, sem agente real, sem rede.

Testes extras (fora do contrato Q1 a Q21): Q18 cobre `spawn_failed` no passo 3 (queue); Q22 cobre E5
(`--version` que nao executa vira `spawn_failed`); Q23 cobre E5 para `--version` que dorme (vira `timeout`).

## Resultados

- Contra o HEAD (v1.10, exportado com `git archive` em diretorio temporario, sem tocar no repositorio): **25 de 25
  falham** (Q23 incluido). Nenhum passa sem a funcionalidade.
- Contra o working tree (com as mudancas de `src/` do outro agente, ainda nao commitadas): Q23 **passa** (rodado
  a pedido do coordenador). Os demais testes desta rodada nao foram rodados. Na rodada anterior, 20 de 20 passaram.
  Os quatro testes novos (Q19 a Q22) e as alteracoes de Q2 a Q4 precisam de uma rodada propria.
- `cargo fmt --check -p chatd`: limpo. `cargo test -p chatd --offline --test v111_codex_queue --no-run`: compila
  sem avisos.
- Tempo: a suite leva cerca de 90 s, porque Q19 espera 60 s de regra de nova tentativa.

## Mapa: teste -> item do spec

| Item | Teste | Afirma | Controle positivo no mesmo cenario |
| --- | --- | --- | --- |
| Q1 | `q1_caminho_feliz_chama_queue_uma_vez_com_argv_exato` | argv exato `queue --thread U --message TEXTO`, uma vez; `--version` antes; `bell_sent` | (e o proprio caso feliz) |
| Q2 (E1) | `q2_thread_file_ausente_nao_chama_version_nem_queue_com_controle` | `thread_file_missing`; antes de o valido receber mensagem, nem `--version` nem `queue` rodaram | o destinatario valido, enviado depois, roda `--version` e `queue` |
| Q3 (E1, E2) | `q3_conteudo_invalido_nao_chama_version_nem_queue_com_controle` | 8 casos invalidos (vazio, `--help`, 65 bytes, duas linhas, `\n\n`, `\r\n`, maiusculo, espaco) -> `thread_file_invalid`; nem `--version` nem `queue` antes dos validos | UUID com um `\n` final e UUID sem quebra final chamam `queue` |
| Q4 (E1) | `q4_symlink_e_gravavel_por_grupo_ou_outros_nao_chama_version_nem_queue_com_controle` | symlink, modo 0620 e modo 0602 -> `thread_file_invalid`, sem `--version` nem `queue` antes dos validos | modo 0600 e modo 0644 chamam `queue` |
| Q5 | `q5_versao_fora_da_lista_nao_chama_queue_com_controle` | versao 0.164.0 -> `codex_version_unsupported`; `queue` nao roda; versao foi consultada | outro daemon com 0.163.0 chama `queue` |
| Q6 | `q6_queue_que_sai_com_1_gera_queue_failed_com_controle` | `queue` que sai 1 -> `queue_failed`; sem `bell_sent` | destinatario feliz vira `bell_sent` |
| Q7 | `q7_queue_que_dorme_gera_timeout_sem_travar_o_daemon` | `queue` que dorme 8 s -> `timeout`; `status` responde em < 3 s durante o sono | destinatario feliz vira `bell_sent` |
| Q8 | `q8_saida_0_sem_queued_message_gera_queue_failed_com_controle` | saida 0 sem `Queued message` -> `queue_failed` | saida com `Queued message` vira `bell_sent` |
| Q9 | `q9_segundo_ciclo_dentro_do_rearme_nao_chama_de_novo` | rearme de 600 s: apos o primeiro toque, 1 chamada depois de 3 s | o primeiro toque acontece (espera) |
| Q9b | `q9b_com_rearme_vencido_o_ciclo_volta_e_chama_de_novo` | par positivo de Q9: rearme de 2 s, pendencia aberta, chama de novo | (ele mesmo e o positivo) |
| Q10 | `q10_accepted_da_pendencia_apaga_o_rearme_com_controle` | A com `accepted` + mensagem nova chama; B sem `accepted` nao chama | A (positivo) no mesmo daemon |
| Q11 | `q11_configuracao_invalida_do_codex_queue_e_recusada_com_controle` | 9 configs recusadas (E6): saida com codigo != 0 e daemon nao fica pronto | config valida sobe; config antiga (sem chaves novas) sobe |
| Q12 | `q12_herdr_e_tmux_falsos_nao_recebem_chamada_do_codex_queue` | herdr so recebe chamadas do painel `w2:p1`; tmux nao recebe nenhuma, mesmo com `queue_failed` | queue feliz; `queue_failed`; `agent prompt` do codex-web no herdr |
| Q13 | `q13_nenhum_tipo_de_evento_novo_no_log` | tipos do log dos quatro desfechos estao em `TIPOS_EXISTENTES` | o log tem `bell_sent` e `bell_failed` |
| Q14 | `q14_texto_enviado_so_tem_a_contagem_com_controle` | argv e exatamente `TEXTO`; assunto e corpo com aspas, `$()`, crase e `;` nao aparecem | o `queue` roda uma vez com `TEXTO` |
| Q15 | `q15_codex_bin_com_espacos_e_metacaracteres_executa_como_um_argv` | `codex_bin` em pasta com espaco e nome com `;`, `$()`, `&` e aspas executa com argv exato | (ele mesmo e o positivo) |
| Q16 | `q16_reinicio_dentro_do_rearme_nao_toca_de_novo` | tecnica de V7: A tocado ha 10 s nao toca ao iniciar | B com `bell_sent` de 700 s toca |
| Q16b | `q16b_reinicio_vivo_dentro_do_rearme_nao_toca_de_novo` | reinicio de verdade (`para` + `sobe`): A nao toca de novo | depois do reinicio, B com mensagem nova toca |
| Q17 | `q17_thread_file_e_relido_a_cada_toque` | apos trocar o UUID no arquivo, o toque seguinte usa o UUID novo; nenhum toque posterior usa o antigo | o primeiro toque usa o UUID antigo |
| Q18 (extra) | `q18_binario_que_deixa_de_ser_executavel_gera_spawn_failed_com_controle` | `codex_bin` sem bit de execucao depois do primeiro toque -> `spawn_failed`; sem `queue` | o primeiro toque executa |
| Q19 (E4) | `q19_depois_de_queue_failed_nova_tentativa_so_apos_60_s` | depois de `queue_failed`: no maximo a chamada inicial nos 55 s seguintes; ha nova chamada depois de 60 s | destinatario feliz no mesmo daemon |
| Q20 (E4) | `q20_depois_de_timeout_nova_tentativa_so_apos_bell_rearm_s` | depois de `timeout` (medido no `bell_failed`): nenhuma chamada nova nos 3 s seguintes, com `bell_rearm_s = 4` | depois do rearme, a nova tentativa (caso feliz) vira `bell_sent`; destinatario feliz no mesmo daemon |
| Q21 (E1 a) | `q21_falha_de_thread_file_nao_executa_nem_version` | ausente, `--help` e symlink: nem `--version` nem `queue` rodam | destinatario valido, enviado depois, roda `--version` e `queue` |
| Q22 (E5) | `q22_version_que_nao_executa_gera_spawn_failed_com_controle` | `--version` sem bit de execucao -> `spawn_failed`; nada executa | com o bit de volta, outro destinatario roda `--version` e `queue` (falha nao fica em cache) |
| Q23 (E5) | `q23_version_que_dorme_gera_timeout_e_nao_chama_queue_com_controle` | `--version` que dorme 8 s -> `timeout` (nao `codex_version_unsupported`); `queue` nao roda; `status` responde durante o sono | no mesmo daemon, com o `--version` normal, o destinatario `outro` roda `--version` e `queue` e vira `bell_sent` |

### Controles e negativos

- Os negativos de Q2 a Q4 e Q21 seguem a ordem do E1: os invalidos sao enviados e recusados antes de o valido receber
  mensagem. Assim, a ausencia de `--version` e observada de verdade, e nao atribuida a um destinatario que rodou antes.
  O controle e o valido enviado depois.
- Q5: a versao e global e tem cache de 10 min; o controle roda em outro daemon (outro diretorio).
- Q12: o controle de tmux nao existe. Sem bind (que exige auth e token) nao ha como fazer o tmux falso ser chamado de
  proposito. O teste so afirma que o tmux falso nao recebe nada.
- Q15: o negativo e implicito (se houvesse shell, o caminho quebraria e `queue` nao rodaria); o controle e o caso feliz.
- Q19 e Q20 usam relogio real. Q19 leva cerca de 85 s, e Q20 cerca de 15 s.

## Ambiguidades e suposicoes

Cada linha diz o que o spec nao define e o que o teste assume. A regra foi a leitura mais literal do spec, sem
escolher a favor do codigo.

| Teste | Item | Ambiguidade / suposicao |
| --- | --- | --- |
| Q1, Q3, Q4, Q15 | 3.2, 3.3 | O formato do argv de `--version` nao e fixado; o fake registra `--version` como argv de uma so palavra. |
| Q1, Q2 | 3.3 | Assume `bell_sent` e `bell_failed` com campo `recipient` (como em `v110_campainha.rs`). |
| Q2 | 2 | Assume `bell_failed` com `reason` no formato da v1.10. |
| Q3 | 2 | "65+ bytes": o teste usa 65 bytes. O formato valido cabe em 37 bytes (UUID + `\n`), entao o limite de 64 bytes nunca decide sozinho (Emenda E3, salvaguarda). |
| Q3 | 2, E2 | Um `\n` final e aceito (E2). `\r\n` e `\n\n` sao recusados (E2). Sem quebra final tambem e aceito, por leitura literal ("sem a quebra de linha final, casa"); a Emenda nao diz o contrario. |
| Q4 | 2 | Modo 0644 e aceito (leitura literal de "sem escrita para grupo/outros"). Modos 0620 e 0602 sao recusados. O spec nao lista os modos aceitos. |
| Q4 | 2 | Symlink: `thread_file` aponta para um link que aponta para um arquivo valido. Recusado pela regra "arquivo regular (nao symlink)". |
| Q5 | 3.2 | Cache de 10 min: o controle precisa de outro daemon. Nao se afirma o formato de `codex-cli X` alem de o numero ser o ultimo campo. |
| Q5 | 3.2 | `codex_version_unsupported` assumido como texto exato da razao. |
| Q6 | 3.4, E4 | Apos `queue_failed`, a nova tentativa e tratada pela E4 (60 s). A frequencia depois disso nao e testada alem de `>= 1`. |
| Q7 | 3.4 | O spec fixa "limite 5 s", mas nao diz quando exatamente o `timeout` aparece. O teste exige o evento em ate 15 s e `status` em < 3 s durante o sono. Um daemon que desistisse antes de 5 s passaria. |
| Q7 | 3.4 | Se o daemon executa um destinatario por vez ou em paralelo, nao esta no spec. O teste so exige que o destinatario feliz vire `bell_sent` em ate 15 s. |
| Q7 | 3.4 | O fake usa `exec sleep 8` para que matar o processo tire o sleep junto; se o daemon so fechar o pipe, o teste ainda passa. |
| Q8 | 3.4 | "Saida 0 sem `Queued message`" = `queue_failed`, como diz o item 4 do spec. |
| Q9 | 3 | Assume que a pendencia aberta continua elegivel depois do rearme (sem novas mensagens). Q9b e o par positivo. |
| Q9b | 1 | `bell_rearm_s = 2` assumido como valor valido. |
| Q10 | 3 | A aceitacao e simulada por `accepted` pre-gravado no log (como em v110 V11), e nao por `message.ack`: o nome exato do `stage` de ack nao esta no spec. |
| Q11 | E6 | "Recusa" = saida com codigo != 0 e sem ficar pronto (E6, como na v1.10). A mensagem de erro nomear a chave e um requisito de E6, mas o teste nao o verifica (a saida e lida so pelo codigo). |
| Q11 | 1 | "codex_bin vazio" e "codex_versions vazio" vem de "validada como tmux_bin: nao vazia" e de "nao vazia". Leitura literal. |
| Q12 | 3.5 | Controle de tmux ausente (ver "Controles"). |
| Q12 | 3 | `codex-web` com `delivery: bell` e `pane` sem bind de tmux usa herdr. Assumido como no v110. |
| Q13 | 3.6 | Lista `TIPOS_EXISTENTES` copiada de `v110_campainha.rs`, nao do codigo. Se o daemon tiver outro tipo antigo (ex. de stale), o teste acusa falso positivo. |
| Q14 | 3.3, passo 3 | Nao se testa "N diferente de 1" (ex. 2 pendentes), porque a corrida com o poll tornaria o caso instavel. |
| Q15 | 3.3 | Sem marcador de efeito colateral: o cwd do daemon nao e controlado pelo teste. Por isso o caminho usa `$(false)`, sem efeito. O teste so prova argv e execucao. |
| Q16 | - | Tecnica de V7 (log pre-gravado) e reinicio vivo (Q16b), ambos sem `accepted`. |
| Q16b | 3 | Assume que `bell_rearm_s` vale no reinicio (a v1.10 diz "rearme duravel"). |
| Q17 | 2 | Assume que o arquivo pode ser reescrito com `fs::write` mantendo modo 0600. `bell_rearm_s = 2` para ter um segundo toque sem novas mensagens. |
| Q18 | 3.2, 3.4 | Teste do passo 3 (queue), com o cache de versao servindo. Nao cobre a razao do `--version` (ver Q22). |
| Q19 | E4 | A janela de 60 s conta da falha. O teste mede a partir da primeira chamada vista, que vem logo antes da falha: a janela de 55 s fica dentro do limite. Sem relogio acelerado, o teste leva cerca de 85 s. |
| Q20 | E4 | O rearme depois do timeout e medido a partir do `bell_failed` (evento observado com poll de 50 ms), com margem de 1 s. |
| Q20 | E4 | `bell_rearm_s = 4` e aplicado a todos os destinatarios do daemon; o destinatario feliz tambem pode repetir chamadas, o que nao afeta a contagem do destinatario lento. |
| Q21 | E1 a | Os tres kinds (ausente, `--help`, symlink) cobrem "falha em (a)". Nao cobre CRLF nem modo gravavel (ja em Q3 e Q4). |
| Q22 | E5 | `--version` que nao executa e `spawn_failed`. Nao testado: o `--version` que passa de 5 s (`timeout`) e a saida de 64 KiB. |
| Geral | - | Janelas de observacao de 2 a 5 s (espera e sono) sao escolhas de teste. |
| Geral | - | Nao testado: limite de 64 KiB de saida do `queue`, `stdin` nulo, `timeout` do `--version`, cache de 10 min isolado, `codex_bin` padrao `"codex"`. |

## Pontos do spec

Lacunas 1, 3, 4 e 5 (do relatorio anterior) foram resolvidas pela Emenda 1:

1. **Ordem das validacoes** (resolvida, E1): (a) valida o `thread_file`; (b) checa a versao; (c) `queue`. Falha em (a)
   nao executa o `codex`. Q2, Q3, Q4 e Q21 afirmam isso.
2. **Limite de tamanho** (E3, salvaguarda): o teste de 65 bytes continua, mas o limite nao decide sozinho.
3. **"Sem a quebra de linha final"** (resolvida, E2): um `\n` final e aceito; `\r\n` e mais de uma quebra sao recusados.
   Q3 cobre os dois. Sem quebra final continua aceito (leitura literal, ver a tabela de ambiguidades).
4. **Nova tentativa apos falha** (resolvida, E4): 60 s depois de qualquer falha do canal; `bell_rearm_s` depois de
   `timeout`. Q19 e Q20 cobrem.
5. **Rearme e falha** (resolvido, E4 e fato do codigo citado na emenda): `bell_failed` nao arma o rearme; o limite
   de 60 s da E4 e o que impede a repeticao.

Ainda abertos:

6. **Razao quando `--version` nao executa**: resolvida pela E5 (`spawn_failed`). Q22 cobre.
7. **`codex_bin` padrao**: o spec diz "padrao `codex`", mas o teste nao cobre o padrao, porque o binario real de `codex`
   no PATH nao pode ser usado nos testes.

## Como rodar

    cd /home/mjabbur/Projects/blocks
    cargo test -p chatd --offline --test v111_codex_queue

Os testes usam so o binario `chatd` (`CARGO_BIN_EXE_chatd`), os falsos em diretorios temporarios e nenhum servico
do ambiente vivo. A suite leva cerca de 90 s.
