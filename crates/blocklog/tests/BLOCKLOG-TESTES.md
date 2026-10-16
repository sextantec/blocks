# blocklog v0: testes caixa-preta (B1 a B34)

Escritos contra a spec `docs/blocklog-spec-v0.md` (rev1.1), so pela API publica da secao 8.
Nenhum teste le `crates/blocklog/src`. Fakes em memoria em `tests/common/mod.rs`
(`Disco`, `ArmazenamentoFalso`, `ArquivoFalso`, `RelogioFalso`); os testes de arquivo real
(B23 a B26) usam `Log::open_dir` com `tempfile`.

Regras aplicadas: todo negativo tem controle positivo no mesmo cenario; oraculos exatos
(bytes do arquivo, `Op` do fake, `seq`, `ts`, variante de erro, `raw_os_error`); a
recuperacao parte de bytes montados a mao no formato da secao 3.

## Tabela: teste -> item da spec -> ambiguidade / suposicao

| Teste | Arquivo | Item da spec | Ambiguidade ou suposicao |
| --- | --- | --- | --- |
| b01 cauda parcial descartada | recuperacao.rs | 4.2, G3 | Suposicao: truncar (`set_len`) antes do `sync_all`. Nenhuma escrita durante a recuperacao. |
| b02 cauda valida sem `\n` descartada | recuperacao.rs | 4.2 | Controle: a mesma linha com `\n` e aceita. |
| b03 linha invalida na ultima posicao | recuperacao.rs | 4.3 | Suposicao: numero da linha e 1-based. Arquivo nao muda (sem `set_len`). |
| b04 linha invalida no meio | recuperacao.rs | 4.3 | Mesma suposicao de numeracao 1-based. |
| b05 lacuna, repeticao, retrocesso, inicio em 2 | recuperacao.rs | 4.3, G5 | Suposicao: seq do primeiro commit e 1. Repeticao usa outro req (`rX`) para isolar a regra de seq. |
| b06 ts regressivo; ts igual aceito | recuperacao.rs | 4.3 | "ts >= anterior": igualdade e aceita (literal). |
| b07 chave extra (envelope e req), req sem `h`, `tx` vazio, `v` != 1 | recuperacao.rs | 3, 4.3 | `v` != 1 nao esta na lista da secao 9; e coberto pela regra "v = 1" da secao 3. |
| b08 apos cauda, arquivo termina em `\n`; seq 2 | recuperacao.rs | 4.2, 9.B8 | Ts do novo commit = relogio (max com o ultimo). |
| b09 sucesso: uma linha, um `sync_data` | escrita.rs | 5.5 | Suposicao: um unico `write_all` com a linha e o `\n` juntos (ordem `Write`, `SyncData`). |
| b10 `write_all` falho: broken, nada aplicado, seguinte broken | escrita.rs | 5.7, 5.1, G6 | "leitura continua": `Log` nao tem API de leitura de commits; testado com `head_seq` e `is_broken` estaveis e reabertura sem bytes. Nao testado: repeticao de req apos broken (5.1 manda `Broken` antes da idempotencia; nao esta na secao 9). |
| b11 `sync_data` falho | escrita.rs | 5.7, G2 | Spec permite linha existir ou nao; o teste aceita as duas formas com verificacao exata de cada uma. O fake grava antes do sync. |
| b12 escrita curta | escrita.rs | 5.7 | O fake grava 10 bytes e erra. Reabertura trunca a cauda para 0 (`SetLen(0)`). |
| b13 ENOSPC (write e sync) | escrita.rs | 5.7 | `Io` preserva `raw_os_error` 28. |
| b14 linha acima de `line_max` | escrita.rs | 3, 5.3, 8 | Suposicao: o limite conta a linha sem `\n`; exatamente `line_max` e aceito. Ordem: `TooLarge` depois de `InvalidInput`/idempotencia (8). |
| b15 `tx` > `tx_max` ou vazio | escrita.rs | 5.3, 8 | Ordem de validacao da secao 8: `EmptyTx` antes de `TxTooLarge`. Controle com 16 eventos (padrao). |
| b16 `log_max_bytes` = `LogFull` sem broken | escrita.rs | 5.7, 8 | Suposicao: tamanho do arquivo inclui o `\n`; "ultrapassar" = resultado maior que o maximo, entao chegar exatamente em `log_max_bytes` e aceito. Dados: `line_max` 4 MiB para caber a linha de 1 MiB. |
| b17 ts nao regride com relogio regressivo | escrita.rs | 5.4, 4.3 | ts = max(agora, ultimo). Reabertura confere a sequencia. |
| b18 mesmo (agent, id, h): replay sem gravar | idempotencia.rs | 5.2, 6 | Repeticao devolve o `res` gravado e ignora o recebido. Nao afirmado o `res` do primeiro commit: 5.6 diz so `{seq, replayed:false}`. |
| b19 mesmo id, h diferente: `RequestConflict` | idempotencia.rs | 5.2 | Chave e (agent, id): outro agente com o mesmo id e commit novo. |
| b20 idempotencia sobrevive a reabertura | idempotencia.rs | 4.4, 6 | Reabertura dentro da retencao. |
| b21 fora da retencao: pedido novo | idempotencia.rs | 4.4, 6 | Retencao 1 dia; reabertura 2 dias depois (novo) e 12 h depois (replay). Fronteira exata (ts = ts + retencao) nao testada: 4.4 diz "menos de" / ">=", a secao 9 diz "mais de". |
| b22 `req` sem `res`: `MissingRes` | idempotencia.rs | 5 (8) | Ordem: `MissingRes` antes da idempotencia (8). |
| b23 segundo open: `AlreadyRunning` (real) | dono_e_arquivo.rs | 2, 4.1 | Arquivo real com `flock`. Controle: depois de soltar, abre. |
| b24 modo errado, symlink, nao regular: `InsecurePermissions` (real) | dono_e_arquivo.rs | 2, 8 | Contradicao leve: 2 manda recusar "arquivo que nao e regular"; a lista de 8 (`FsStorage`) cita so modo, dono e symlink. Testado como 2 manda. |
| b25 arquivos 0600 e diretorio 0700 (real) | dono_e_arquivo.rs | 2 | Fsync do diretorio NAO testado: nao e observavel por API publica sem strace ou instrumentacao. |
| b26 uid errado (so root) | dono_e_arquivo.rs | 2 | `#[ignore]`: exige euid 0 (verificado por `/proc/self/status`); troca o dono com `chown` para 65534 e volta ao uid do processo como controle. Nao rodado (nao e root). Rodar: `sudo -E cargo test -p blocklog --test dono_e_arquivo -- --ignored b26`. |
| b27 reabrir devolve a sequencia gravada | recuperacao.rs | 4.5, 9.B27 | Commit sem `req`: lido como `req: None, res: None`. A forma em disco (chave ausente ou `null`) nao e fixada pelo teste. |
| b28 SHA-256 vetores NIST | hash.rs | 8, 9.B28 | Vetores: vazio, "abc", 448 bits, 896 bits (extra, FIPS 180-2) e 1 milhao de `a`. Conferidos com `sha256sum` antes de entrar no teste. |
| b29 elemento de `tx` nao objeto: `InvalidInput` | escrita.rs | 8 rev1.1, 9.B29 | Nao objetos testados: string, numero, `null`, array, booleano. Nada gravado; o mesmo req passa depois com seq 1 (prova que a recusa nao indexou). |
| b30 `Req` fora do formato: `InvalidInput` | escrita.rs | 8 rev1.1, 9.B30 | Casos da secao 9 (maiuscula no agent, espaco no id, `h` de 63/65 caracteres, `h` maiuscula) mais agent de 33, agent vazio, id de 65, `h` com `g`. Controle com limites validos (agent de 32, id de 64). |
| b31 duas ocorrencias de (agent, id) dentro da retencao: `Corrupt` | recuperacao.rs | 4.4 rev1.1, 9.B31 | Corrupt na linha da segunda. Retencao padrao 30 dias; casos com `h` igual (1 h depois) e `h` diferente (30 dias menos 1 ms). Controle: agente diferente nao e duplicata. |
| b32 duas ocorrencias separadas por mais que a retencao: validas | recuperacao.rs | 4.4 rev1.1, 9.B32 | Segunda = vigente: replay com o h dela devolve o res dela; o h da primeira vira `RequestConflict`. Teste em 30 dias + 1 s. |
| b33 linha completa com `req` e sem `res`: `Corrupt` (e `res: null` aceito) | recuperacao.rs | 3, 4.3 | Corrupt na linha 1 e na 2, arquivo intacto; controle com `res`. Variante `"res": null` (b33b): leitura literal, `null` e JSON valido ("JSON qualquer"), entao abre; a forma em memoria (`None` ou `Some(null)`) nao e fixada, o teste aceita as duas. |
| b34 linha completa valida maior que `line_max` (4096): `Corrupt` na abertura | recuperacao.rs | 3, 4.3 | Linha de ~5100 bytes; controle com `line_max` padrao abre e linha curta abre com 4096. Ambiguidade: reduzir `line_max` depois de gravar linhas maiores faz a abertura recusar; a leitura literal (3: "no maximo line_max"; 4.3) manda recusar, e o teste fixa isso. |

## Contradicoes e ambiguidades da spec (rev1.1)

1. **Fronteira da retencao (4.4 x 9.B32).** 4.4 aceita a segunda ocorrencia com `ts2 >= ts1 + retencao`; B32 fala em "separadas por mais de". Igualdade exata: a spec nao decide o que vale. Os testes evitam a fronteira (B21, B31 com 1 ms a menos, B32 com 1 s a mais).
2. **Numeracao de linha (4.3).** "numero da linha" nao diz se e 1-based. Os testes assumem 1-based.
3. **`req` ausente na forma em disco (3).** "req opcional" nao diz se a chave e omitida ou `null`. Nenhum teste compara os bytes de um commit sem `req`; B27 so le.
4. **`Committed.res` no primeiro commit (5.6).** A spec diz `{seq, replayed:false}`, sem `res`. Os testes nao afirmam `res` nesse caso; so no replay (5.2).
5. **"leitura continua" (5.7).** A API (8) nao tem leitura de commits em `Log`. Interpretado como `head_seq` e `is_broken` estaveis, e reabertura do mesmo arquivo.
6. **Broken antes da idempotencia (5.1 x 9).** 5.1 manda `Broken` antes de qualquer outra checagem, mesmo para um req ja indexado. Literal, mas nao esta na secao 9 e nao foi testado.
7. **Arquivo nao regular (2 x 8).** 2 manda recusar com `insecure_permissions`; 8 mapeia so modo, dono e symlink. Testado como 2 manda (b24).
8. **Fsync do diretorio (2).** Nao observavel por API publica. Sem teste (b25 cobre so modo e conteudo).
9. **Escopo de `log_max_bytes` (5.7).** Suposto: o `\n` conta no tamanho do arquivo; igualdade e aceita (b16).
10. **Escopo de `line_max` (3).** Suposto: o limite conta a linha sem `\n` (b14).
11. **`res: null` (3).** "res opcional (JSON qualquer ...)": `null` e um valor JSON presente. Leitura literal: aceito (b33b). Se o dono quiser `null` como ausencia, o teste b33b deve mudar.
12. **Reducao de `line_max` apos gravar (3, 4.3).** A abertura recusa linhas maiores que o `line_max` atual, mesmo que tenham sido gravadas com um limite maior (b34). Pode ser o comportamento desejado (validacao estrita) ou um problema de upgrade; a spec nao discute.

## Lacunas da secao 9 (nao cobertas por B1 a B32)

- `Options::validate` e as faixas da secao 8 (`BadOptions`): nenhum teste.
- `tx_max` aplicado na leitura (abertura): a spec nao diz; nenhum teste. (`line_max` na abertura: coberto por b34.)
- `res` sem `req` (3 permite, 5 nao define o que o commit faz): nenhum teste.
- `res` ausente com `req` presente: coberto na abertura por b33 (Corrupt); nao coberto no commit.
- Repeticao de req apos `broken` (5.1): nenhum teste.
- Fsync do diretorio em `FsStorage::open_log` (2): nao observavel.

## Estado da verificacao

- `cargo fmt --check -p blocklog`: limpo (apos formatar so os arquivos de teste).
- `cargo test --offline -p blocklog --no-run`: compila os seis alvos de teste sem warnings.
- Rodada 2 (B33, B34): `cargo fmt --check -p blocklog` limpo; `--no-run` compila sem warnings; `b33` (incluindo b33b) e `b34` passaram 3 vezes seguidas.
- Suite completa NAO executada, por instrucao. B26 fica `#[ignore]` (exige root).
- Nenhum teste usa rede, `tmux`, `codex`, `claude`, `~/.local`, `~/.config`, `~/.claude` ou `~/.codex`.
