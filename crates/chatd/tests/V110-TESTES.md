# V110-TESTES: testes independentes da v1.10 (seguranca da campainha)

Escritos pelo Haiku (agente de testes) a partir de `docs/spec-v1.10-seguranca-da-campainha.md`, sem ler `src/`.
Arquivo de teste: `tests/v110_campainha.rs`. Daemon real em diretorio temporario; `herdr` e `tmux` falsos (scripts
shell) registram o argv de cada chamada. Sem multiplexador real, sem agente real, sem rede.
Os pre-testes gravam `state/log.jsonl` antes de subir o daemon (V7 a V13), no formato dos testes existentes
(`v18_stale_resultado.rs`, `v19_sink_tmux.rs`).

## Mapa: teste -> item do spec

| Item | Teste | Esperado hoje | Motivo do resultado atual |
| --- | --- | --- | --- |
| V1 | `v01_tmux_sem_chave_nao_digita_e_registra_bell_failed_do_motivo_novo` | FALHA | tmux recebe `send-keys` (comportamento ausente) |
| V2 | `v02_tmux_com_chave_false_explicita_nao_digita` | FALHA | config recusada: chave `bell_tmux_typing` desconhecida (esperado ate a chave existir) |
| V3 | `v03_tmux_com_chave_true_digita_como_na_v13_uma_vez` | FALHA | config recusada pela mesma chave (controle de V1/V2) |
| V4 | `v04_valor_ou_chave_invalida_recusa_a_config_e_booleano_sobe` | FALHA | "sim" e chave com outro nome sao recusados (ok); o controle `true` falha pela chave ausente |
| V5 | `v05_herdr_working_nao_recebe_prompt_e_idle_recebe` | PASSA | regressao do ramo herdr, sem mudanca |
| V6 | `v06_nenhum_tipo_de_evento_novo_nas_rodadas_v1_a_v5` | FALHA | com tmux ligado nao ha `bell_failed` (o tmux digita); falha pelo comportamento ausente |
| V7 | `v07_bell_sent_de_10s_do_claude_nao_toca_ao_iniciar` | FALHA | claude recebe `agent prompt` no inicio (rearme nao duravel) |
| V8 | `v08_bell_sent_de_700s_do_claude_toca_em_um_ciclo` | PASSA | positivo: 700 s >= 600 s toca uma vez |
| V9 | `v09_bell_sent_com_ts_no_futuro_nao_toca` | FALHA | claude toca no inicio (rearme nao duravel) |
| V10 | `v10_bell_sent_com_ts_ilegivel_nao_toca_e_o_daemon_sobe` | FALHA | claude toca no inicio (rearme nao duravel) |
| V11 | `v11_accepted_da_pendencia_apaga_o_rearme_e_a_nova_toca` | PASSA | positivo: toca hoje e deve continuar tocando depois da correcao |
| V12 | `v12_so_o_destinatario_com_bell_sent_recente_fica_quieto` | FALHA | claude (bell_sent de 10 s) toca no inicio |
| V13 | `v13_replay_nao_acrescenta_eventos_ao_log` | PASSA | replay nao grava hoje; o controle (idle grava `bell_sent`) confirma que o log e vivo |

### Controles (negativo x positivo)
- V1 e V2 (tmux desligado) tem controle em V3 (mesma config com `true`: digita `send-keys -l` e `Enter`, uma vez cada).
- V7 (claude, 10 s) tem controle com o codex (700 s) no mesmo daemon; V8 e o par positivo com claude a 700 s.
- V9, V10 e V12 tem controle com o codex (700 s, ou sem `bell_sent`) no mesmo daemon: o daemon esta vivo e a campainha funciona.
- V13 tem controle: com `idle` no restart, o mesmo log ganha `bell_sent` do codex.
- V6 tem controle de nao vazio: o conjunto de tipos contem `bell_failed` e `bell_sent`.

## Testes antigos alterados

`tests/bind_v13.rs` (somente a chave `"bell_tmux_typing": true` nos testes que digitam pelo tmux falso; nada apagado):
- `cfg_v13` continua igual em comportamento; passou a chamar a nova `cfg_v13_com(dir, mux, codex, extra)`.
- Novos helpers `amb_com(codex, extra)` e `amb_digita(codex)`; `amb(codex)` continua igual.
- Testes com a chave: `c1_`, `c3_`, `d1_` (ignored), `d2_` (ignored), `e2_`, `f2_` (via `amb_digita`).
- `t1_tmux_real_...`: a chave foi acrescentada na propria config do teste (tmux real em `-S` temporario).
- Os demais testes de `bind_v13` nao mudaram (nao digitam pelo tmux).

Outros testes de campainha: `v14_recibos.rs` digita pelo herdr falso (`digita.log` do herdr), nao pelo tmux: nao alterado.
`v19_sink_tmux.rs` testa o sink de `stale`, nao a campainha: nao alterado.

Resultado de `cargo test --offline -p chatd --test bind_v13` apos a mudanca: 11 passam; 5 falham (`c1_`, `c3_`, `e2_`, `f2_`, `t1_`) com
`config invalida: unknown field bell_tmux_typing`, ou seja, falha de config esperada ate a chave existir; `d1_` e `d2_` ignorados.

## Ambiguidades (o spec nao define; marcadas `AMBIGUO` no codigo)

1. V1: o spec proibe `send-keys` e "nenhum comando"; nao diz se a campainha pode rodar outro subcomando (ex. `display-message`) antes de recusar. Aqui qualquer chamada ao tmux depois do bind conta como erro.
2. V4: "saida de erro como as outras chaves invalidas" nao fixa codigo de saida; aceita-se qualquer valor diferente de 0.
3. V10: `ts` ilegivel = idade 0 pelo spec; nao diz se a linha com `ts` ruim afeta a leitura de outros campos. Aqui so se exige subir e nao tocar.
4. `TIPOS_EXISTENTES` (V6): lista montada dos docs e dos testes existentes (`bound`, `unbound`, `denied`, `auth_failed`, `auth_ok`, `blocked`, `unaccepted`, `due_expired`), nao do codigo.
5. Janelas de observacao: 3 s para "nao toca", 2 s depois do primeiro toque para contar "uma vez". Sao escolhas de teste, nao do spec.
6. `bell_failed` com `tmux_typing_disabled`: o spec diz "no maximo um por minuto"; aqui exige exatamente 1 em 3 s de rodadas (a primeira falha sempre gera o evento).
7. Formato do log pre-gravado (`ts` no nivel de cima da linha; campos de `registered` como em `v18_stale_resultado.rs`): o spec nao fixa os campos de `bell_sent`; usa-se `recipient` e `ts`.
8. V1 usa `delivery: bell` com `pane` fixo mais bind tmux; pela v1.3 (E), a ligacao prevalece. O spec da v1.10 nao repete isso.
9. Os falsos respondem assim: `tmux display-message` le o arquivo `pane_PANE` (pid); `send-keys` sai 0; `herdr agent get` le `estado.txt`. Nao e um teste de contrato desses programas.
10. O daemon e subido com `serve --state-dir --config` e o socket padrao (`state/chatd.sock`), como nos outros testes; nao ha flag `--socket` no helper.
