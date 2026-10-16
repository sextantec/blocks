# Mutacao do agentd rev6 (2026-10-16)

Script: `crates/agentd/scripts/mutacao_rev6.py` (copia o workspace, aplica UM mutante por vez, roda os alvos `rev6_*`). 20 mutantes sobre a fusao (`daemon.rs`), o mapeamento do Codex (`app_server.rs`) e as faixas de configuracao (`config.rs`).

| Rodada | Mortos | Sobreviventes | O que mudou |
| --- | --- | --- | --- |
| 1 (39 testes) | 13/17 | F2, F3, F4, F5 | lacunas: esperas do `screen_override` e confirmacao do terminal |
| 2 (+L1, L2, L3) | 16/17 | F4 | F2, F3, F5 morrem |
| 3 (+B1, regra `blocked` da rev6.2; 20 mutantes) | 17/20 | F4, B2, B3 | B1 morre; B2 era lacuna real |
| 4 (+b2) | B2 morto | F4, B3 | gate: 189 testes, 0 falhas |

**Equivalentes por construcao (nao sao lacunas):** F4 e B3 removem a exigencia de `screen_rule` na tela; a tela so publica `idle`/`blocked` por regra positiva e copia a regra junto com o estado (`daemon.rs:1138-1141`), entao a condicao nunca difere. Nao provado formalmente.

Decisoes de contrato tomadas no caminho (spec-rev6.md, rev6.1 e rev6.2): `notLoaded`/status inesperado = `unknown` sem recuo para a tela; `idle` relatado apos troca do par exige 2 leituras do mesmo par (sem `idle_confirm_ms`); relato `working` com tela `blocked` publica `blocked`.

Testes oscilantes: um teste (`s6_thread_fora_da_lista...`) falhou uma vez em paralelo e passou 5 de 5; nao reproduzido nas 4 rodadas de mutacao nem no gate final.
