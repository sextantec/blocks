# Backlog do memd

1. 2026-10-16 — codex-blocks — O teste independente `crates/memd/tests/mutacoes.rs:423` esperava `conflict` ao aplicar `reject`/`retire` a registro em estado diferente. O Sextante emendou `docs/memd-spec-v0.md:39` para definir `not_found` em id inexistente/ilegível e `conflict` em estado incompatível; `Service::change_state` foi alinhado. RESOLVIDO 2026-10-16 (ajuste local pendente de commit pelo Sextante).

2. 2026-10-16 — codex-blocks — Q5 em `crates/memd/tests/acesso.rs:197` tratava `next_after` como linha de texto da visão. A emenda do Sextante em `docs/memd-spec-v0.md` (commit `d3808e8`) fixou a forma `{text, trust, view_truncated, next_after, head_seq}`; o defeito era no helper do teste independente. ENCERRADO 2026-10-16, commit `d3808e8`.

3. 2026-10-16 — codex-blocks — Q6/Q6b em `crates/memd/tests/acesso.rs:226,242` esperavam sobreposição entre `due` e `active`. A emenda do Sextante em `docs/memd-spec-v0.md` (commit `d3808e8`) definiu as seções como disjuntas e `state=done` fora de ambas; o teste independente será corrigido. ENCERRADO 2026-10-16, commit `d3808e8`.

4. 2026-10-16 — codex-blocks — O09 em `crates/memd/tests/operacao.rs:301` originalmente esperava `quota` para linha acima de `line_max`; a spec foi emendada para `invalid_params` e o teste independente ajustado. `cargo test --offline -p memd` agora passa. RESOLVIDO 2026-10-16 (suite Haiku fechada; commit da emenda não informado).

5. 2026-10-16 — codex-blocks — Durante a edição dos testes pelo Haiku, `acesso.rs` deixou de compilar por nomes de helpers temporariamente desalinhados. O Sextante confirmou que a suite ainda estava em edição e instruiu não registrar falhas transitórias de compilação. ENCERRADO 2026-10-16 (transiente, suite independente em edição).

6. 2026-10-16 — codex-blocks — Auditoria final: em `crates/memd/src/service.rs:236,691-696,797-816,1168-1174`, validação de `review_by` usa a data efetiva antes de `Log::commit`, mas a projeção é reaplicada depois com relógio atualizado. Se UTC virar à meia-noite entre esses pontos, uma proposta pode ser gravada e depois rejeitada pela projeção; o serviço fica `broken` com evento durável. Corrigir alinhando a data usada na validação e no timestamp efetivo do commit; exige obter do `blocklog` o timestamp realmente gravado, fora dos arquivos autorizados desta tarefa.
