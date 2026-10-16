# Mutacao da v1.11 (canal codex-queue), 2026-10-16

Script: `crates/chatd/scripts/mutacao_v111.py DIR_TRABALHO DIR_TARGET` (copia o workspace; um mutante por vez; roda `v111_codex_queue`, sem os lentos q19/q20 exceto nos mutantes que os miram).

| id | mutante | resultado |
| --- | --- | --- |
| M1 | sucesso sem checar a saida do `queue` (`Queued message `) | MORTO |
| M2 | checagem de versao removida | MORTO |
| M3 | escrita de grupo aceita no `thread_file` | MORTO |
| M4 | escrita de outros aceita | MORTO |
| M5 | dono do arquivo nao checado | SOBREVIVEU (nao testavel sem root; backlog 18) |
| M6 | varias quebras de linha e CRLF aceitas | MORTO |
| M7 | UUID maiusculo aceito | MORTO |
| M8 | opcao do argv trocada (`--thread`) | MORTO |
| M9 | texto enviado deixa de ser o fixo | MORTO |
| M10 | versao fora da lista aceita na 1a consulta | MORTO |
| M11 | timeout do `--version` vira `codex_version_unsupported` | SOBREVIVEU na 1a rodada (lacuna dos testes); o Q23 (Haiku) o cobre: **MORTO** na 2a rodada |
| M13 | sem espera de 60 s apos falha | MORTO (q19) |
| M14 | timeout nao espera `bell_rearm_s` | MORTO (q20) |
| M15 | `codex_versions` deixa de ser obrigatoria | MORTO |
| M16 | `thread_file` relativo aceito | MORTO |
| M17 | `pane` aceito com `codex-queue` | MORTO |
| M18 | `codex-queue` deixa de ser campainha | MORTO |

Correcao feita pelo Sextante na revisao: `O_NOFOLLOW` fixo em `0x20_000` (so correto em x86_64) trocado por `libc::O_NOFOLLOW` (`libc` ja e dependencia).

**Resultado final: 16 de 17 mutantes mortos; o unico sobrevivente (M5, dono do arquivo) e nao testavel sem root.** Gate: fmt, clippy `--bins -D warnings`, `git diff --check` e `cargo test -p chatd` completos verdes (ver commit).
