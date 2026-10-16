# attest v0: registro de quem verificou o que em qual versao do codigo (contrato)

Origem: consulta ao Opus e ao codex-blocks (2026-10-16, P-84); **escopo aprovado pelo Jabbur em 2026-10-16** ("Aprovado o spec do attest"). Bloco do lego: um binario pequeno, estado em arquivo legivel, **cliente puro** (nao e daemon, nao fala com o `chatd`, nao chama modelo, sem rede). Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes pelo Haiku.

## Problema (exemplo)
Hoje "o `codex-blocks` implementou, o Haiku escreveu os testes sem ver o codigo, eu rodei o gate" e uma convencao: vive na memoria do orquestrador e some na compactacao da conversa. Exemplo: depois de comitar o sink tmux, um sucessor nao tem como saber se os testes eram mesmo independentes nem se o gate rodou **nesta** versao do codigo. O `attest` grava cada verificacao com a identidade da arvore de arquivos e responde, para um commit, **qual passo falta**.

## Estado e modelo de dados
- Diretorio `--state-dir DIR` (ou `ATTEST_STATE_DIR`; padrao `~/.local/state/attest`), modo 0700; arquivo `log.jsonl`, modo 0600, **so de anexar** (`O_APPEND` + `flock`). Uma linha = um registro JSON. Estado que nao existe e criado.
- Campos de todo registro: `v` (1), `seq` (a partir de 1, sem buracos), `ts` (UTC, ISO 8601), `prev` (sha256 da linha anterior, `""` na primeira: cadeia que denuncia edicao), `kind` (`run|note`), `step`, `task`, `tree`, `head`, `agent`, `model`, `effort`, `authorized_by`, `result` (`pass|fail`).
  - `step`: nome livre `[a-z0-9_-]{1,32}` (ex.: `gate`, `author`, `tests`, `review`, `secrets`, `mutation`). `task`: `[A-Za-z0-9._-]{1,64}`. `agent`: `[a-z0-9_-]{1,32}`.
  - `tree`: id git da **arvore de trabalho** atual, calculado com um indice temporario (`GIT_INDEX_FILE` fora do repositorio, `git add -A` no indice temporario, `git write-tree`), respeitando `.gitignore`; **nao altera o indice real nem a copia de trabalho**. `head`: `git rev-parse HEAD` ou `null`.
  - `run` acrescenta: `argv` (lista), `exit` (inteiro ou `null` se nao rodou/sinal), `duration_ms`, `output_sha256`, `output_bytes`. **A saida do comando nunca e gravada** (so o hash e o tamanho): pode conter segredo.
  - `note` acrescenta: `detail` (texto, no maximo 280 caracteres, sem quebra de linha).
- `authorized_by`: texto de quem autorizou, quando e a citacao (`Jabbur (dono), 2026-10-16, "..."`), no maximo 280 caracteres. **E uma declaracao, nao uma prova**: o `attest` nao verifica a pessoa.

## Comandos (`--json` so em `check`, `show` e `verify`; erros em stderr; `--state-dir` vai depois do subcomando)
1. `attest run --step S --task T --agent A --model M [--effort E] --authorized-by TXT -- CMD ARG...`: roda `CMD` por `argv`, **sem shell**, no diretorio atual (deve estar dentro de um repositorio git), com timeout `--timeout SEG` (padrao 900), repassa stdout/stderr do comando ao terminal, grava um registro `run` (`result` = `pass` se sair 0, senao `fail`) e **sai com o codigo do comando** (timeout: 124). Gravar vem antes de sair.
2. `attest note --step S --task T --agent A --model M [--effort E] --authorized-by TXT --result pass|fail --detail TXT`: registro `note` para passos que nao sao um comando (ex.: `author` = quem escreveu, `tests` = quem escreveu os testes, `review`).
3. `attest check (--tree auto|ID) --policy ARQ --task T [--json]`: le a politica e responde se a **arvore indicada** cumpre cada passo. Sai 0 (completo), 3 (falta ou falhou algum passo), 2 (uso), 4 (log corrompido/ilegivel).
4. `attest footer --task T --tree auto|ID`: imprime as linhas `Agente:` e `Autorizado por:` do rodape do commit a partir dos registros `pass` daquela tarefa e arvore.
5. `attest verify`: confere `seq` sem buracos e a cadeia `prev`; sai 0 ou 4 informando a primeira linha ruim.
6. `attest show --task T [--tree ID]`: lista os registros (sem `argv` longo: corta em 200 caracteres).

## Politica (arquivo JSON, so leitura)
```
{"steps": [
  {"name": "gate", "kind": "run"},
  {"name": "tests", "independent_of": "author"},
  {"name": "review", "independent_of": "author"}
]}
```
- `name`: precisa de um registro `pass` com este `step` na **mesma arvore**. `kind` (opcional, `run|note`): exige esse tipo. `independent_of` (opcional): o `agent` deste passo deve ser diferente do `agent` do passo indicado, **na mesma tarefa** (o passo `author` tambem precisa existir e estar `ok`).
- Vale o **registro mais recente** de cada passo na arvore: um `fail` mais novo derruba um `pass` antigo. `check` imprime, por passo, `ok`, `missing`, `failed` ou `not_independent` (com os dois agentes). Arvore diferente (arquivo mudou depois) = passo `missing` (`stale_tree` na saida JSON quando existe registro `pass` de outra arvore da mesma tarefa).
- Politica invalida (JSON ruim), passo repetido ou campo desconhecido = saida 2.

## Seguranca e limites
- **Sem dependencias novas** (o ambiente nao tem rede): so `serde`, `serde_json`, `chrono`, `libc`, `thiserror` ja usados no workspace. O SHA-256 e implementado no proprio crate e testado com vetores conhecidos (**A21**: `""`, `"abc"` e 1.000.000 de `a`).
- Sem shell, sem rede, sem segredo: nunca le `.env`, token nem credencial; nunca grava a saida do comando; **o ambiente do comando e herdado, mas nao e registrado**.
- Limites: linha de registro no maximo 8 KiB; log no maximo 64 MiB (acima, recusa gravar e sai 4); lock de 5 s.
- **Limite honesto:** o log e inviolavel so por visibilidade (cadeia de hash). Quem pode escrever o arquivo pode forjar o historico inteiro. O `attest` **nao e assinatura nem autorizacao**; serve para o orquestrador nao esquecer e para o sucessor conferir. Subir para assinatura fica fora da v0.
- Pasta do repositorio fora de git, ou `git` ausente: `run`/`note`/`check` saem 2 (nao inventam `tree`).

## Contrato testavel (repositorio git temporario e `--state-dir` temporario)
- **A1** `run` com comando que sai 0: registro `run`/`pass` com `exit` 0, `argv` exato, `tree`, `head`, saida 0; `output_sha256` confere e **a saida nao aparece no log**. **A2** comando que sai 3: `fail`, `exit` 3, o `attest` tambem sai 3. **A3** comando inexistente: `fail`, `exit` `null`, saida 127, registro gravado. **A4** `timeout`: registro `fail`, saida 124. **A5** `--` obrigatorio; sem comando = 2. **A6** metacaracteres no argumento (`;`, `$(...)`) chegam literais ao comando (nao ha shell). **A7** `tree` muda quando um arquivo muda e **nao** muda se so o `.gitignore`d muda; `git status` e o indice real ficam intactos. **A8** `note` valido grava; `detail` > 280 ou com quebra de linha = 2. **A9** nomes invalidos (`step`, `task`, `agent`) = 2. **A10** cadeia: apos 3 registros, `verify` sai 0; editar uma linha do meio, remover uma linha ou trocar duas = 4 e informa a linha. **A11** `check` completo (gate `run` pass + `author` note + `tests` note de outro agente + `review`) = 0; remover qualquer = 3 com o passo certo. **A12** `tests` pelo mesmo agente do `author` = `not_independent`, saida 3. **A13** depois do `gate` pass, alterar um arquivo: `gate` fica `missing` (`stale_tree`). **A14** `fail` mais novo derruba `pass` antigo do mesmo passo e arvore. **A15** `footer` imprime `Agente:` e `Autorizado por:` com os agentes e a autorizacao registrados; sem registro `pass`, sai 3 e nao imprime. **A16** dois `run` em paralelo: `seq` sem buracos e cadeia valida. **A17** log com permissao 0600 e diretorio 0700. **A18** politica invalida/duplicada/campo desconhecido = 2. **A19** fora de repositorio git = 2. **A20** `--json` de `check` tem os campos `step`, `status`, `agent`, `seq`. **A21** SHA-256 de `attest` bate com os vetores conhecidos (via `output_sha256` de comandos que imprimem `""`, `abc` e 1.000.000 de `a`).

## Esclarecimentos (testes independentes, 2026-10-16)
1. `--state-dir DIR` e opcao de cada subcomando (depois do nome dele); `ATTEST_STATE_DIR` vale quando ausente.
2. JSON de `check`: objeto `{"task","tree","complete":bool,"steps":[{"step","status","agent","seq"}]}`; `status` em `ok|missing|failed|not_independent`; `stale_tree: true` no passo quando ha `pass` de outra arvore.
3. `footer` imprime exatamente duas linhas: `Agente: A (MODELO[, ESFORCO]); B (...)` (agentes distintos pela ordem do primeiro `seq`) e `Autorizado por: X | Y` (valores distintos de `authorized_by`).
4. Timeout: `result` = `fail`, `exit` = `null`; o `attest` sai 124. O filho e o grupo de processos dele recebem SIGKILL.
5. Comando inexistente: `exit` = `null`, o `attest` sai 127 (convencao do shell).
6. O limite de 280 conta caracteres (escalares Unicode), nao bytes.
7. Limite declarado: a cadeia nao detecta a remocao das **ultimas** linhas (nao ha ponteiro externo). Editar, remover do meio ou trocar linhas = `verify` sai 4 e informa o `seq` da primeira linha ruim.
8. `stale_tree` aparece no JSON (item 2) e como a palavra `stale_tree` na saida de texto.
9. Passo com `independent_of` cujo alvo nao existe ou nao esta `ok`: `not_independent` (com `author: null`); `check` sai 3.
10. `argv` e o programa seguido dos argumentos.
11. `ts` = `YYYY-MM-DDTHH:MM:SSZ`.
12. Campo desconhecido no topo da politica ou dentro de um passo = saida 2.
13. O `git` e procurado no `PATH`; ausente = saida 2.
14. So `run` e `note` criam o estado. `check`, `show`, `footer` e `verify` sobre log inexistente leem um log vazio (`check` sai 3, `footer` sai 3, `verify` sai 0).
15. `run` e `note` nao tem `--json`: `run` repassa so a saida do comando; `note` imprime uma linha `seq=N` e sai 0. `footer` sai 0 quando ha registro `pass`.
16. `--result` diferente de `pass|fail` = saida 2.
17. `output_bytes` e o tamanho exato, em bytes, de stdout+stderr do comando; `effort` e opcional.

## Fora da v0
Assinatura/chave, daemon, integracao automatica com `chatd` ou git hook, cota (isso e o `ledger`), `resume` derivado (o `show`/`check` sao a base), `grantd`.

## Aceite
Testes independentes (Haiku, sem ver `src/`), fmt/clippy `-D warnings`, mutacao (`mutacao.py`), e um piloto: registrar o gate do proprio `attest` com o `attest run` e gerar o rodape do commit com `attest footer`.
