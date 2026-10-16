# Testes independentes do attest v0: mapa e ambiguidades

Fonte unica: `docs/attest-spec-v0.md`. Escritos sem ler `crates/attest/src/**`. Arquivo: `crates/attest/tests/attest_v0.rs`.

Estado hoje (esqueleto que sai 2): os 22 testes falham, cada um no primeiro controle positivo (`esperado Some(0), veio Some(2)`). Nenhum passa por acidente.

## Mapa: teste -> item do spec

| Teste | Item | O que afirma |
| --- | --- | --- |
| `a01_run_pass_grava_registro_sem_saida_no_log` | A1 | registro run/pass, campos, argv, tree/head, hash e tamanho da saida; saida nao aparece no log |
| `a02_run_fail_sai_com_codigo_do_comando` | A2 | comando com saida 3 grava fail, exit 3, attest sai 3 (controle: saida 0) |
| `a03_comando_inexistente_grava_fail_exit_null_sai_127` | A3 | comando inexistente: fail, exit null, attest sai 127 |
| `a04_timeout_grava_fail_e_sai_124` | A4 | `--timeout 1` com `sleep 5`: fail, sai 124, em menos de 4 s |
| `a05_separador_obrigatorio_e_comando_obrigatorio` | A5 | sem `--` = 2; `--` sem comando = 2 (controle: com `--` sai 0) |
| `a06_metacaracteres_chegam_literais` | A6 | `;` e `$(...)` chegam literais ao printf; nenhum arquivo criado por shell |
| `a07_tree_ignora_gitignore_e_nao_toca_repositorio` | A7 | tree nao muda com arquivo .gitignored; muda com arquivo novo ou alterado; indice e `git status` intactos |
| `a08_note_detail_limite_e_quebra_de_linha` | A8 | note valido grava (280 caracteres, controle); 281 ou quebra de linha = 2 |
| `a09_nomes_validos_e_invalidos` | A9 | nomes validos nos limites passam; step/task/agent invalidos = 2 |
| `a10_cadeia_detecta_edicao_remocao_e_troca` | A10 | verify 0 em log integro; editar, remover ou trocar linha = 4 |
| `a11_check_completo_e_passo_removido` | A11 | check completo = 0; sem gate, tests ou review = 3 com missing; sem author = 3 |
| `a12_tests_mesmo_agente_do_author_nao_independente` | A12 | mesmo agente = not_independent, 3 (controle: outro agente, 0) |
| `a13_arvore_mudou_gate_fica_stale` | A13 | apos alterar arquivo, gate missing e stale_tree na saida |
| `a14_fail_mais_novo_derruba_pass_antigo` | A14 | fail mais novo na mesma arvore: gate failed, check 3 |
| `a15_footer_imprime_agentes_e_autorizacao` | A15 | footer com pass: linhas Agente: e Autorizado por:, sai 0; sem pass sai 3 sem imprimir |
| `a16_runs_em_paralelo_mantem_seq_e_cadeia` | A16 | quatro runs em threads: todos 0, seq 1..4, verify 0 |
| `a17_permissoes_do_estado` | A17 | diretorio de estado 0700, log.jsonl 0600 |
| `a18_politica_invalida_sai_2` | A18 | JSON ruim, passo repetido, campo desconhecido = 2 (controle: politica valida, 0) |
| `a19_fora_de_repositorio_sai_2` | A19 | run, note e check fora de git = 2 e nada gravado (controle: dentro do repo, 0) |
| `a19b_sem_git_no_path_sai_2` | A19 (git ausente) | check sem git no PATH = 2 (controle: com PATH normal, 0) |
| `a20_check_json_tem_campos` | A20 | check --json: cada passo da politica tem step, status, agent, seq |
| `a21_sha256_vetores_conhecidos` | A21 | output_sha256 de printf vazio, "abc" e 1.000.000 de "a" bate com os vetores |

Cobertura do spec: A1 a A21 cobertos; A19 tem dois testes (fora de git e git ausente). Nao coberto (fora da v0 ou sem item A): `show`, permissao de `.env`, limite de 8 KiB por linha, limite de 64 MiB do log, lock de 5 s, `--json` de run/note/verify em formato.

## Ambiguidades

Cada item abaixo esta marcado `AMBIGUO` no codigo. Nenhum comportamento foi inventado: os testes so afirmam o que o spec diz, e onde o spec e silencioso o teste evita afirmar ou afirma o minimo.

1. **Posicao de `--state-dir`.** Os testes passam a flag logo depois do subcomando (`attest run --state-dir X ...`). O spec nao diz se e global ou por subcomando.
2. **Envelope do JSON de `check`.** O spec nao diz se a lista de passos e um array na raiz ou um campo de um objeto. Os testes aceitam os dois. A12 e A13 procuram o status pelo campo `step`.
3. **Formato do rodape (`footer`).** O spec nao diz a ordem, os separadores nem se varios agentes saem numa linha. O teste so exige o prefixo `Agente:` e `Autorizado por:` e os nomes.
4. **`exit` no timeout.** O spec diz `fail` e saida 124, mas nao diz se o campo `exit` do registro vale null ou 124. Nao e afirmado.
5. **"Saida 127" em A3.** Lido como codigo de saida do proprio attest (127). O spec pode querer dizer outra coisa.
6. **Limite de 280 do `detail`.** Caracteres ou bytes? Os testes usam so ASCII, onde os dois coincidem.
7. **Ordem e cobertura da cadeia (A10).** A troca de linhas nao diz qual linha o verify aponta (so o codigo 4 e afirmado). Remover a **ultima** linha nao e detectavel pela cadeia de hash, e o spec nao diz se deve ser; o teste remove a linha do meio.
8. **Forma de `stale_tree` (A13).** Pode ser status, campo ou texto. O teste so exige a palavra na saida.
9. **Passo reportado sem `author` (A11).** O spec diz que `tests` exige `author` ok, mas nao diz se o passo que aparece como falho e `tests`, `author` ou ambos. O teste so exige exit 3 e que `tests` nao fique ok.
10. **`argv` inclui o proprio comando?** Os testes assumem que `argv` e o comando seguido dos argumentos, incluindo o nome do programa (`["printf", ...]`).
11. **Formato de `ts`.** O spec diz ISO 8601 UTC, sem formato exato. O teste so exige texto com `T`.
12. **Campo desconhecido.** O spec diz "campo desconhecido = 2", mas nao diz se vale no topo da politica ou so dentro dos passos. Os dois casos estao no teste A18.
13. **Git ausente (A19b).** O spec diz "git ausente: exit 2", sem dizer como o attest acha o git. O teste pesa o PATH: se o attest usar caminho absoluto, este teste falha por motivo que nao e defeito do attest.
14. **Quem cria o estado.** O spec diz "estado que nao existe e criado", sem dizer qual comando. Os testes usam `run`.
15. **Saida de `run`, `note` e `verify` com `--json`.** Os testes nao passam `--json` nesses comandos, porque o spec nao diz como a saida de `run` (que tambem repassa a saida do comando) se mistura com o JSON.
16. **Codigo de sucesso de `note` e `footer`.** O spec nao diz explicitamente que `note` sai 0 e que `footer` sai 0 com pass. Os testes assumem 0.
17. **Campo `output_bytes`.** O spec o lista, mas nao diz se e o tamanho exato da saida em bytes. O teste confere o tamanho em bytes.
18. **Efeito de `check` sobre o `not_independent` sem `author`.** O spec nao diz qual status aparece para `tests` quando `author` nao existe.
19. **Timeout e processo filho.** O spec nao diz se o attest mata o comando que estourou o timeout. O teste so mede o tempo total do attest.
20. **Valores de `--result` em `note`.** O spec define `pass|fail`. Os testes usam so `pass` e `fail`, sem testar valor invalido, porque o spec nao diz o codigo de saida para valor invalido.
21. **Campos `effort` e `model`.** `effort` e opcional e nao e testado. `model` e sempre passado.
