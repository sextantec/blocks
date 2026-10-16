# Mutacao do sink tmux (v1.9 Parte A), 2026-10-16

Script: `crates/chatd/scripts/mutacao_sink.py DIR_TRABALHO DIR_TARGET` (copia o workspace; um mutante por vez em `src/sink_tmux.rs`; roda `v19_sink_tmux` e `v19_sink_limites`).

| rodada | mortos | sobreviveram |
| --- | --- | --- |
| 1 (so K1-K14) | 8 de 18 | M05 timeout por chamada 50 s; M06 timeout total 100 s; M07 limite de saida 64 MiB; M08 corte em 201; M10 list-clients sem aviso novo; M13 aviso repetido por chamada; M14 minutos negativos; M15 nome de cliente de 65; M16 sem ordenar; M17 sem checar prazo antes de chamar |
| 2 (+ L1-L7, `v19_sink_limites.rs`) | **15 de 18** | M14, M16, M17 |

Os 3 sobreviventes sao **equivalentes** (analise do Sextante, sem teste que os distinga): M16, o `stale` ja entrega as entradas em ordem de nome; M17, a checagem so antecipa o `Timeout` que o laco de espera produziria de qualquer forma; M14, o `stale` nao gera minutos negativos (`.max(0)` e defesa). Nao exigem teste novo.

Testes L1-L7 (Haiku 5.5 high, sem ler `src/`): timeout por chamada < 8 s, total < 13 s, saida > 64 KiB = `bad_output`, corte exato em 200, `list-clients` so com aviso novo, um aviso por motivo, nome de cliente de 64 e 65.
