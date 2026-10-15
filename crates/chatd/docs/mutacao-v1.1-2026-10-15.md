# Mutacao do chatd v1.1 e secao F (2026-10-15)

Runner: `scripts/mutacao.py` (105 mutantes: 54 da v1 + 51 da v1.1/F). Rodada completa sobre `8a5f59a`; tres mutantes (S04, P07, B07) tinham o trecho desatualizado, foram corrigidos e rodados a parte (todos mortos).

**Resultado da primeira rodada: 81 mortos, 24 sobreviventes.** Depois do lote `tests/lacunas_mutacao.rs` (16 testes independentes do Haiku), os 13 mutantes de lacuna real que o teste de integracao alcanca morreram: **96 mortos (V23 e V24 morreram com 2 testes unitarios em `server.rs`) e 9 equivalentes; nenhum pendente.**

## Equivalentes ou defesa redundante (nao exigem teste): 9
- S13 (sem fsync; so observavel com queda de energia), S17 (fronteira do relogio), S25 (janela 0644 antes do `set_permissions`), B03 (guarda redundante da campainha), A01 (token > 64 caracteres: o hash nunca casa), A09 (hash fixo de nome inexistente; so observavel por tempo), V13 (fechar apos 5 falhas e redundante com a trava por pid: uma conexao tem um unico pid), V17 (conexao sem auth nem chega a `events.resume`), C07 (id so tem caracteres seguros; so diferiria com log forjado).

## Lacunas reais de teste (15): 13 fechadas pelo lote `tests/lacunas_mutacao.rs`; V23 e V24 fechadas por teste unitario em `server.rs`
T01 (take isola destinatario), T06 (denied/auth_failed nao vazam em watch), V16 e V19 (nome invalido fora de `denied`/`auth_failed`), V22 (take com destinatario desconhecido), A03 (auth.json com campo extra), A08 (token orfao em --out), T08 (destinatario ve os proprios accepted/resulted), C01 e C02 (escape de C1/DEL/BEL na exibicao), C04 (--take com --all/--after), C05 (--take com varias mensagens), C08 (watch sem token e CHATD_AS), V23 e V24 (teto e poda do mapa de pids: teste unitario em `server.rs`, `prune_pids`, pendente; provado por sonda pelo auditor Opus com 4.300 pids).
