# Mutacao da v1.10 (campainha tmux sem digitar; rearme duravel), 2026-10-16

Script: `crates/chatd/scripts/mutacao_v110.py DIR_TRABALHO DIR_TARGET` (copia o workspace; um mutante por vez; roda `v110_campainha` e `bind_v13`).

| id | mutante | resultado |
| --- | --- | --- |
| B1 | guarda de `ring` removida (o `verify` rodaria `display-message` do tmux) | MORTO |
| B2 | guarda de `send_prompt` removida (segunda barreira) | SOBREVIVEU: equivalente enquanto B1 vale; mantida de proposito como defesa em profundidade |
| C1 | padrao de `bell_tmux_typing` vira `true` | MORTO |
| S1 | replay ignora a idade do `bell_sent` | MORTO |
| S2 | replay nao reconstroi `outstanding` | MORTO |
| S3 | `ts` no futuro ou ilegivel vira idade enorme | MORTO |
| S4 | controle equivalente (`unwrap_or(now - 0)`) | SOBREVIVEU: sem efeito observavel, como previsto |

Todos os mutantes com efeito observavel morrem; os 2 sobreviventes sao equivalentes.
