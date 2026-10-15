# chatd v1.8: verificador `stale` (v1.7 + v1.7.1) e anexos com validade (RASCUNHO para decisao do Jabbur)

Origem: decisoes do Jabbur (2026-10-15 noite). **Anexos:** "Se o arquivo fosse enviado ao chatd para ser disponibilizado em uma area compartilhada do projeto esse problema estaria resolvido" e, ao autorizar o rascunho: "ela ja deve incorporar a todas as anteriores que nao foram implementadas. O armazenamento de arquivos deve ter um limite de tempo (expiry-date) para evitar que fique muito cheio. Caso a mensagem seja direta e nao multitasking ou broadcast, o agente que recebeu deve remover o arquivo do armazenamento." **Stale:** v1.7 aprovada ("V1.7: Aprovado") e v1.7.1 ("6. Concordo"). Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes pelo Haiku. **Esta spec e cumulativa:** quem implementa le so este arquivo; `spec-v1.7-stale.md` fica como historico (a Parte A abaixo a substitui) e os testes `tests/v17_stale.rs` continuam valendo.

## Entrega em duas partes (commits separados)
- **Parte A (`chatd stale`)**: cliente puro, **nao altera o daemon**; pode ser instalada sem reiniciar o daemon.
- **Parte B (anexos)**: **altera o daemon** (metodos novos, disco, coleta); exige revisao de seguranca (Opus e Sol) antes da implementacao e reinicio do daemon na instalacao.

---
# Parte A: `chatd stale` (v1.7 integral + regra v1.7.1)

### Problema (exemplo)
O `codex-blocks` perde a rede da sandbox as 15:40 e a campainha toca de 10 em 10 minutos sem aceite; a RESPOSTA fica 90 min sem ser enviada por uma duvida de autorizacao. Ninguem e avisado.

### Regras
1. **`chatd stale [--state-dir D] [--older-than SEC] [--bells N] [--result-older-than SEC] [--notify SINK] [--stamp-file F] [--json]`.** Le `D/log.jsonl` (sem conectar ao daemon, sem token). Padroes: `--older-than 900` (15 min, confirmado: "P-75c: OK"), `--bells 2`, `--result-older-than 3600` (60 min).
2. **Pendente:** `registered` sem `accepted`/`resulted` depois. **Idade** = agora - `ts` do `registered`. **Campainhas sem aceite** = `bell_sent` do destinatario depois do `registered` mais antigo ainda pendente.
3. **Parado por falta de aceite:** existe pendente com idade > `--older-than` **ou** campainhas sem aceite >= `--bells`.
4. **(v1.7.1) Parado por falta de resultado:** mensagem `kind` **PEDIDO** ou **PERGUNTA** com `accepted` e **sem** `resulted`, ha mais de `--result-older-than`. `AVISO`, `RESPOSTA` e `ACK` nao entram (nao pedem resultado). Medida dos dados de 2026-10-15: PEDIDO/PERGUNTA, mediana aceite->resultado 87 s, 90% em ate 17 min, maximo 73 min; 2 sem resultado.
5. **Episodio:** um destinatario parado gera **um** aviso por episodio; o episodio de um **tipo** (aceite ou resultado) termina quando nao ha mais mensagem nessa condicao.
6. **Saida:** uma linha por destinatario e tipo, **texto fixo**: `[chat] parado: NOME N pendente(s), mais antiga M min, campainha K vez(es)` e `[chat] sem resultado: NOME N pedido(s), mais antigo M min`. **Nunca** remetente, assunto ou corpo. Codigos: 0 nada novo a avisar; **6** pelo menos um aviso **novo** nesta execucao; 2 uso (inclui `--notify tmux` e `--stamp-file` que seja link simbolico); 4 log ilegivel ou ausente.
7. **Deduplicacao:** `--stamp-file` (0600, criado pelo verificador) guarda por destinatario e tipo `{episode_first_seq, last_alert_ts}`; no mesmo episodio nao reavisa antes de `--older-than` (aceite) ou `--result-older-than` (resultado) novamente.
8. **`--json`:** um objeto por destinatario e tipo parado, inclusive os ja avisados, com `new_alert:false`; `true` so no primeiro aviso do episodio. No texto, a linha deduplicada e omitida.
9. **Sink (`--notify`), enumerado e sem shell:** `stdout` (padrao); `file:CAMINHO` (anexa, 0600). **`tmux` reservado e rejeitado com saida 2** ate o `agentd` e a adocao do tmux (decisao do Jabbur).
10. **Somente leitura:** nao envia, nao aceita, nao toca campainha, nao executa nada vindo do log. Linha invalida: `skipped=N` em stderr. **Seguranca:** os `registered` do log carregam `subject` e `body` completos; o verificador le so `type`, `ts`, `seq`, `id`, `from`, `to`, `kind` e `recipient` e **nunca** copia o resto.

### Contrato testavel (Parte A)
**S1 a S12** e **S7j** de `tests/v17_stale.rs` e da spec v1.7, mais: **S13** PEDIDO aceito ha 59 min sem resultado: nada; ha 61 min: saida 6 e a linha `sem resultado`. **S14** AVISO e RESPOSTA aceitos ha 3 h sem resultado: nada. **S15** PERGUNTA conta como PEDIDO. **S16** `resulted` apos o aviso encerra o episodio; uma segunda mensagem nova no mesmo destinatario reavisa. **S17** os dois tipos no mesmo destinatario geram duas linhas distintas, sem se deduplicarem entre si. **S18** `--result-older-than` invalido (0, negativo, nao numerico) e saida 2.

### Esclarecimentos da Parte A (testes independentes S13 a S22, 2026-10-15)
1. **Idade do "sem resultado"** conta a partir do `accepted` da mensagem (o trabalho comeca no aceite), nao do `registered`.
2. **`M` (minutos)** e a parte inteira de `idade/60`, arredondada para baixo; os testes aceitam +-1 (S13b: 60 a 62 para 61 min).
3. **`--json`:** **uma linha por objeto** (JSON Lines, como `chatd log` e `watch`), nao um array. Campos de cada objeto: `name`, `type` (`unaccepted` ou `no_result`), `count`, `oldest_min`, `bells` (so em `unaccepted`) e `new_alert`.
4. **`skipped`** e impresso em stderr como `skipped=N` quando N > 0.

---
# Parte B: anexos (arquivos) com validade

### Problema (exemplo)
Agentes em sandboxes diferentes **nao enxergam os arquivos uns dos outros**; o unico canal que todos alcancam e o socket do `chatd`. Hoje: o Codex da Web nao le a bancada (grava-se a interface em `docs/agentes/` dentro do repositorio dele); o `codex-blocks` nao leu o repositorio do `agentd`; o brief do Sol precisa ser inline (teto de 64 KB); o corpo da mensagem tem teto de 16 KiB.

### B.0 `status` passa a dizer so "esta vivo" e estatisticas (decisao do Jabbur, 2026-10-15 noite)
Decisao: "o status deve dizer somente se o daemon esta funcionando e estatisticas, nada mais. Nao precisamos dizer quem roda e perfis."
- **`status`** `{v}` -> `{v, ok:true, version, uptime_s, head_seq, messages:{total, registered, accepted, resulted}, bells:{sent, failed}, connections, blobs:{count, bytes}}`. **Sem nomes** de agente, destinatario, perfil, painel nem remetente; so contagens. **Nao exige autenticacao** (qualquer cliente do mesmo uid), mesmo com a auth ligada; nao muda por quem pergunta.
- **O que sai do `status`:** `recipients{NOME:{pending, accepted, bell, last_bell_seq}}`. Quem precisa da propria caixa usa **`mailbox.counts`** `{v}` (autenticado): `{v, pending, accepted, bell, last_bell_seq}` so da identidade que pergunta; CLI `chatd inbox --count`. O estado detalhado por pessoa continua so no `chatd log` (arquivo local) e no `inbox`.
- **Efeitos a corrigir quando entrar:** `cli.rs` (`status_texto_sai_0_e_cita_os_destinatarios`, `status_json_tem_a_estrutura_da_spec`), `campainha.rs`, `auth.rs`, `protocolo.rs`, `v14_recibos.rs`, `bind_v13.rs` e as suites v1.5/v1.6 que leem `recipients` do `status` passam a usar `mailbox.counts` ou o `log`; a regra da v1.6 "status sem identidade conecta sem autenticar" passa a **ter resposta com a auth ligada**.
- **Compatibilidade:** mudanca de contrato publico (campo `recipients` removido do `status`); entra **junto com a Parte B** (daemon), com aviso. Ate la, o `status` atual continua como esta.
- **Testes novos:** **B16** `status` com auth ligada e sem token responde `ok:true` com so contagens; **B17** nenhum nome de agente/destinatario/perfil/painel aparece na resposta (sentinela em nome de destinatario configurado); **B18** `mailbox.counts` devolve so a caixa da identidade; outra identidade nao ve a alheia; **B19** `inbox --count` imprime as tres contagens.

### Modelo
- **Blob:** arquivo opaco identificado por `blob:<sha256>` (64 hex). O conteudo nunca e interpretado, executado nem logado.
- **Dono:** a identidade que enviou. **Leitores:** dono + os destinatarios declarados no envio (`to`). Ninguem mais le (identidade pelo token, como hoje).
- **Modo (derivado):** **`direct`** = exatamente um destinatario e nao broadcast; **`multi`** = mais de um destinatario ou broadcast (`to: "*"` quando existir).
- **Validade obrigatoria:** todo blob tem `expires_at`. Padrao `blob_default_ttl_s` = 86400 (24 h); maximo `blob_max_ttl_s` = 604800 (7 dias); pedido acima do maximo e recusado (`invalid_envelope`).
- **Armazenamento:** `D/blobs/<aa>/<sha256>` (0600, diretorio 0700; `aa` = 2 primeiros hex), escrita atomica (arquivo temporario, `fsync`, `rename`); o nome do arquivo no disco **e sempre o hash**; `name` do cliente e so metadado (`[A-Za-z0-9._-]{1,64}`), nunca vira caminho.
- **Metadados** persistem no `log.jsonl` (eventos `blob_put`, `blob_got`, `blob_released`, `blob_removed`, `blob_expired`, com `sha`, `size`, `owner`, `to`, `mode`, `expires_at`, `name`, `reason`); o conteudo nao entra no log. Na partida o daemon reconstroi o indice pelo log e **remove do disco** arquivos sem evento correspondente (orfaos) e os vencidos.

### Protocolo (novos metodos; `v: 1` inalterado; clientes antigos nao sao afetados)
Linha de 1 MiB: o envio e em pedacos de ate **512 KiB** brutos (~700 KiB em base64).
- **`blob.put.begin`** `{v, size, name?, to:[N...], ttl_s?}` -> `{v, upload_id, chunk_max}`. Recusa: `size > blob_max_bytes` (padrao 8 MiB) => `too_large`; cota => `quota_exceeded`; `to` vazio ou nome desconhecido => `unknown_recipient`.
- **`blob.put.chunk`** `{v, upload_id, n, data}` (`n` consecutivo a partir de 0; `data` base64) -> `{v, received}`.
- **`blob.put.end`** `{v, upload_id, sha256}` -> `{v, blob:"blob:<sha>", size, expires_at, mode}`. O daemon recalcula o SHA-256 e o tamanho; divergencia => `hash_mismatch`, nada e gravado. Mesmo conteudo ja existente = **deduplicado** (um arquivo, uma entrada por dono, contagem de referencias).
- **`blob.get`** `{v, blob, offset?, length?}` -> `{v, size, offset, data}` (pedacos de ate 512 KiB). Somente dono e leitores; outro => `forbidden`; inexistente ou vencido => `not_found`.
- **`blob.rm`** `{v, blob}` -> `{v, removed:bool, remaining}`. **`direct`:** dono **ou** destinatario remove (apaga na hora). **`multi`:** o dono remove tudo; um destinatario so **libera** (`blob_released`); o arquivo some quando todos liberaram, o dono removeu ou venceu.
- **`blob.list`** `{v}` -> blobs em que a identidade e dono ou leitora (so metadados).
- Uploads: sessao expira em **120 s** sem pedaco; ate **4** simultaneos por identidade; sem retomada (reinicia).

### Regras de limpeza (quem remove, e o que garante que nao enche)
1. **Validade:** coleta na partida e a cada 60 s (`blob_gc_s`); vencido => arquivo apagado, evento `blob_expired`, `blob.get` => `not_found`.
2. **Direto (decisao do Jabbur):** o agente que **recebeu** remove o arquivo depois de usar (`chatd get --consume BLOB` baixa e remove; ou `chatd rm`). **Rede de seguranca do daemon:** quando uma mensagem `direct` que referencia o blob chega a `resulted`, o daemon remove o blob automaticamente (a limpeza nao depende da disciplina do agente); a validade e o ultimo recurso.
3. **Multi/broadcast:** cada destinatario libera (`chatd release BLOB`); sai quando todos liberaram, o dono removeu ou venceu. Recebedor nao remove sozinho (outros ainda precisam).
4. **Cotas (recusa, nunca despejo):** `blob_max_bytes` 8 MiB por arquivo; `blob_owner_max_bytes` 64 MiB e `blob_owner_max_count` 32 por dono; `blob_total_max_bytes` 256 MiB no total; espaco livre abaixo de 2x o arquivo => `disk_full`. Quota cheia gera `quota_exceeded` com a conta, sem apagar nada de ninguem.

### Cliente (CLI)
`chatd put FILE [--to N]... [--name N] [--ttl DUR] [--json]` | `chatd get BLOB [--out PATH|-] [--consume]` | `chatd rm BLOB` | `chatd release BLOB` | `chatd ls`. **`chatd send --attach FILE`** (repetivel, ate 8): faz `put` para os `--to` da mensagem e acrescenta em `refs` `{"t":"blob","v":"blob:<sha>"}`. `get --out -` escreve em stdout (para sandbox sem pasta gravavel). **Denylist por nome** no cliente (`.env`, `*.token`, `auth.json`, `*.pem`, `id_*`, `*.key`): recusa com saida 2 e a explicacao; **nao ha varredura de conteudo** (limite declarado: o conteudo e responsabilidade de quem anexa). Identidade explicita como na v1.6.

### Seguranca e limites declarados
Mesmo usuario do sistema: o diretorio 0700 nao protege contra outro processo do **mesmo** uid; o controle e por identidade/token do protocolo (limite ja conhecido da v1.1). Conteudo nunca executado, nunca no log, nunca em `watch`. Nenhum nome do cliente vira caminho; links simbolicos nunca sao seguidos; o daemon nunca le arquivo por caminho enviado pelo cliente (so recebe bytes).

### Contrato testavel (Parte B)
- **B1** ida e volta: `put` -> `get` devolve bytes identicos (sha igual); arquivo de 0 byte e de exatamente 8 MiB passam, 8 MiB+1 e `too_large`.
- **B2** `hash_mismatch` e `size` divergente: nada gravado, sessao encerrada. **B3** pedaco fora de ordem, repetido, maior que `chunk_max`: erro, sem gravar.
- **B4** ACL: terceiro `get`/`rm`/`list` => `forbidden`/ausente; dono e destinatario leem.
- **B5** validade: `ttl_s` curto (config de teste) => `get` apos o prazo `not_found`, arquivo some do disco, evento `blob_expired`; TTL acima do maximo recusado; sem TTL usa o padrao.
- **B6** `direct`: destinatario `rm` apaga na hora; `get --consume` baixa e remove; **mensagem `resulted` remove automaticamente** o blob que ela referencia.
- **B7** `multi`: destinatario `release` nao apaga; o ultimo a liberar apaga; dono `rm` apaga tudo; vencimento apaga.
- **B8** cotas: por arquivo, por dono (tamanho e contagem), total; recusa sem despejo; `disk_full` simulado por limite de teste.
- **B9** dedup: mesmo conteudo de dois donos = um arquivo; remover um nao quebra o outro; o ultimo remove o arquivo.
- **B10** reinicio do daemon: indice reconstruido; arquivo orfao e vencido removidos; upload em andamento descartado.
- **B11** upload: sessao expira em 120 s (relogio de teste), 5o upload simultaneo recusado, nome com `../` ou `/`, link simbolico como `FILE`, denylist.
- **B12** privacidade: sentinela no conteudo, no `name` e no assunto: nao aparece no log, em `watch`, em `status`, nem em stderr.
- **B13** `send --attach`: cria `refs` do tipo `blob`, deriva os leitores do `--to`, e o `inbox` mostra a referencia (nao o conteudo).
- **B14** `get --out -` (stdout) e `--out` em diretorio sem permissao: erro limpo (saida 5 pela v1.6), sem arquivo parcial.
- **B15** compatibilidade: suites v1.1 a v1.7 e a auditoria F continuam verdes; cliente antigo (sem `blob.*`) nao e afetado; metodo `blob.*` em daemon antigo => `method_not_found` tratado pelo cliente novo com mensagem clara.

## Perguntas abertas (decisao do Jabbur)
1. TTL padrao de 24 h e maximo de 7 dias servem? 2. Limite de 8 MiB por arquivo e 256 MiB no total servem (o maior documento de hoje, uma spec, tem ~25 KiB; um clipe de camera seria proibido por desenho)? 3. A rede de seguranca "mensagem `resulted` remove o blob direto" fica? 4. Broadcast (`to: "*"`) ja existe no `chatd`? Se nao, `multi` vale so para mais de um destinatario.
