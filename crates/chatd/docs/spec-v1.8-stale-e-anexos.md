# chatd v1.8: verificador `stale` (v1.7 + v1.7.1) e anexos com validade (RASCUNHO para decisao do Jabbur)

> **Numeracao (decisao do Jabbur, 2026-10-15):** `v1.x` nomeia **iteracoes de spec**, nao lancamentos. A versao do pacote e **0.2.0** e nada muda; a versao 1.0 fica reservada para algo **muito estavel**. O `"v": 1` do protocolo e a compatibilidade no fio, outra coisa.

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
# Parte B: anexos (arquivos) com validade (rev2: incorpora as revisoes de seguranca do Opus e do Sol 6.1, 2026-10-15)

> **Estado:** rascunho **rev2**. Duas decisoes do Jabbur continuam abertas e esta rev2 assume a recomendacao em ambas (marcadas **[CONFIRMAR]**): (1) **sem deduplicacao** entre envios; (2) anexos **desligados por destinatario** ate o Jabbur liberar (opt-in). Se ele decidir o contrario, reverte-se so esses dois pontos.

### Problema (exemplo)
Agentes em sandboxes diferentes **nao enxergam os arquivos uns dos outros**; o unico canal que todos alcancam e o socket do `chatd`. Hoje: o Codex da Web nao le a bancada (grava-se a interface em `docs/agentes/` dentro do repositorio dele); o `codex-blocks` nao leu o repositorio do `agentd`; o brief do Sol precisa ser inline (teto de 64 KB); o corpo da mensagem tem teto de 16 KiB.

### Ameaca declarada (leia antes de implementar)
1. **Os anexos ampliam em ~500 vezes (8 MiB contra 16 KiB) o que um agente induzido por texto malicioso pode tirar da maquina** pelo correio, em especial para um destinatario com rede (o Codex da Web). **Nao ha prevencao de vazamento de segredos nesta versao**: a denylist por nome e a varredura de padroes sao protecao **acidental e best-effort**, contornaveis (renomear, falar direto no socket). A contencao real e o opt-in por destinatario e a autorizacao do dono.
2. **Mesmo usuario do sistema:** o diretorio 0700 nao protege contra outro processo do mesmo uid; o controle e a identidade por token do protocolo.
3. **Disponibilidade:** o correio e um daemon de **trava unica** (`Mutex<Store>`, `sync_data` por evento, log que so cresce). Anexo nenhum pode atrasar `message.send` de ninguem nem encher o disco do log.

### B.0 `status` passa a dizer so "esta vivo" e estatisticas (decisao do Jabbur, 2026-10-15 noite)
Decisao: "o status deve dizer somente se o daemon esta funcionando e estatisticas, nada mais. Nao precisamos dizer quem roda e perfis."
- **`status`** `{v}` -> `{v, ok:true, version, uptime_s, head_seq, messages:{total, registered, accepted, resulted}, bells:{sent, failed}, connections, blobs:{count, bytes}}`. **Sem nomes** de agente, destinatario, perfil, painel nem remetente; so contagens. **Nao exige autenticacao** (qualquer cliente do mesmo uid), mesmo com a auth ligada; nao muda por quem pergunta.
- **O que sai do `status`:** `recipients{NOME:{pending, accepted, bell, last_bell_seq}}`. Quem precisa da propria caixa usa **`mailbox.counts`** `{v}` (autenticado): `{v, pending, accepted, bell, last_bell_seq}` so da identidade que pergunta; CLI `chatd inbox --count`. O estado detalhado por pessoa continua so no `chatd log` (arquivo local) e no `inbox`.
- **Efeitos a corrigir quando entrar:** `cli.rs` (`status_texto_sai_0_e_cita_os_destinatarios`, `status_json_tem_a_estrutura_da_spec`), `campainha.rs`, `auth.rs`, `protocolo.rs`, `v14_recibos.rs`, `bind_v13.rs` e as suites v1.5/v1.6 que leem `recipients` do `status` passam a usar `mailbox.counts` ou o `log`; a regra da v1.6 "status sem identidade conecta sem autenticar" passa a **ter resposta com a auth ligada**.
- **Compatibilidade:** mudanca de contrato publico (campo `recipients` removido do `status`); entra **junto com a Parte B** (daemon), com aviso. Ate la, o `status` atual continua como esta.
- **Testes novos:** **B16** `status` com auth ligada e sem token responde `ok:true` com so contagens; **B17** nenhum nome de agente/destinatario/perfil/painel aparece na resposta (sentinela em nome de destinatario configurado); **B18** `mailbox.counts` devolve so a caixa da identidade; outra identidade nao ve a alheia; **B19** `inbox --count` imprime as tres contagens.

### Modelo
- **Blob = um envio.** Cada `put` cria um blob **independente**, com id opaco `blob:<32 hex aleatorios>` (128 bits de `getrandom`). **Sem deduplicacao** entre envios (ni entre donos, ni do mesmo dono) **[CONFIRMAR]**: o hash SHA-256 e **metadado de integridade**, nunca um identificador de autorizacao. Motivos (Opus S2 e Sol S01 a S03): a deduplicacao cria oraculo de existencia, ambiguidade de vinculo/TTL e corridas; 256 MiB nao pedem dedup.
- **Dono** = identidade que enviou; **destinatario** = exatamente um nome declarado no envio. Ninguem mais le. **Para ausente, vencido ou nao autorizado a resposta e a mesma: `not_found`** (sem `forbidden`, sem campos extras), para nao revelar existencia.
- **Opt-in por destinatario [CONFIRMAR]:** `config.json` `recipients.NOME.blobs: true` (padrao `false`). Envio para quem nao liberou: `blobs_disabled`. O dono liga caso a caso.
- **Validade obrigatoria:** `expires_at` = `created_at` + `ttl_s`. Padrao `blob_default_ttl_s` 86400 (24 h), maximo `blob_max_ttl_s` 604800 (7 dias), **minimo 60 s** (decisao do Jabbur: 24 h e 7 dias). Prazo calculado no relogio **monotonico** do processo; persistem `created_at` e `ttl_s`; na partida vale `min(parede, created_at + blob_max_ttl_s)`.
- **Armazenamento:** `D/blobs/<aa>/<id>` (0600; diretorios 0700; `aa` = 2 primeiros hex), temporarios em `D/blobs/tmp-<rand>` criados com `O_EXCL`. O nome no disco e **sempre o id**; `name` do cliente (`[A-Za-z0-9._-]{1,64}`, validado no servidor) e metadado e **vai ao `log.jsonl`** (0600, como o assunto). O conteudo **nunca** entra no log, em `watch`, `status` nem em erros.
- **Visibilidade dos eventos:** `blob_put`, `blob_got`, `blob_removed`, `blob_expired` so sao vistos por dono e destinatario daquele blob (`visible()` filtra por vinculo); `status` nao os expoe.

### Protocolo (metodos novos; `v: 1` inalterado; exigem autenticacao)
- **`blob.put.begin`** `{v, size, name?, to:N, ttl_s?}` -> `{v, upload_id, chunk_max}`. Reserva **atomicamente** bytes e contagem na cota (incluindo sessoes em andamento). Erros: `too_large`, `quota_exceeded`, `unknown_recipient`, `blobs_disabled`, `rate_limited`, `disk_full`, `invalid_envelope` (`ttl_s` fora de 60..max).
- **`blob.put.chunk`** `{v, upload_id, n, data}`: `n` consecutivo a partir de 0; `data` base64 de ate **512 KiB** brutos; o decodificado vai **direto a um temporario em disco** (nunca acumula em memoria); `recebido <= size` a cada pedaco; repetir ou saltar `n` aborta a sessao.
- **`blob.put.end`** `{v, upload_id, sha256}` -> `{v, blob, size, expires_at}`. O daemon calcula o SHA-256 **incrementalmente** (por pedaco, fora da trava), compara tamanho e hash; divergencia aborta e limpa. Pedaco vazio so para arquivo de 0 byte (sem pedacos).
- **`blob.get`** `{v, blob, offset?, length?}` -> `{v, size, offset, data}` com `length` <= 512 KiB (validado); le sempre do **mesmo descritor** aberto.
- **`blob.rm`** `{v, blob}` -> `{v, removed:true}`: dono **ou** destinatario. **`blob.list`** `{v, after?}` (pagina de 50): os blobs em que a identidade e dono ou destinatario, so metadados.
- **Sessao de upload:** `upload_id` aleatorio de 128 bits, **ligado a identidade e a conexao**; outra identidade ou conexao => `not_found`. Ociosidade 120 s **e prazo absoluto de 10 min**; no maximo **1 upload simultaneo por identidade** e **8 globais**; ate 20 pedacos por sessao.

### Ordem de durabilidade e recuperacao (obrigatoria)
Criar: temporario `O_EXCL` 0600 -> `fsync` -> `rename` para `blobs/<aa>/<id>` -> `fsync` do diretorio -> **so entao** o evento `blob_put` (com sincronizacao) -> ACK. Remover: evento `blob_removed` duravel **antes** do `unlink`; `unlink` idempotente. Na partida o daemon: reconcilia vinculos ativos com arquivos (vinculo sem arquivo => `blob_removed reason=missing`); apaga so nomes `^[0-9a-f]{32}$` e `tmp-[0-9a-f]+` sem evento ou vencidos, abrindo com `O_NOFOLLOW`/`openat`; **recusa subir** se `blobs/` ou `<aa>/` nao for diretorio 0700 do proprio uid ou for link simbolico. As transicoes (checagem, vinculo, `rm`, coleta) sao **linearizadas por blob**; hash e I/O ficam **fora** da trava global.

### Regras de limpeza (quem remove, e o que garante que nao enche)
1. **Validade:** coleta na partida e a cada 60 s; vencido => arquivo apagado, `blob_expired`, `get` => `not_found`.
2. **Direto (decisao do Jabbur):** o agente que **recebeu** remove o arquivo depois de usar (`chatd get --consume` baixa, confere o hash e so entao remove; ou `chatd rm`). **Nao ha remocao automatica ao `resultado`** (decisao "B": o agente pode reler). `chatd ls` mostra `expires_in`.
3. **Cotas (recusa, nunca despejo):** `blob_max_bytes` 8 MiB; por dono 64 MiB e 32 blobs; total 256 MiB; **contagem logica** (cada envio e cobrado inteiro). Espaco livre minimo = tamanho + **reserva fixa de 512 MiB** para o log. Limite de taxa: 30 `put.begin`/min por identidade; vencimento nao devolve cota instantaneamente para burlar a taxa.

### Cliente (CLI)
`chatd put FILE --to N [--name N] [--ttl DUR] [--json]` | `chatd get BLOB [--out PATH|-] [--consume]` | `chatd rm BLOB` | `chatd ls`. **`chatd send --attach FILE`** (**um** anexo por mensagem nesta versao): valida o destinatario e o opt-in **antes** do `put`, depois envia; falha entre os dois passos remove o blob recem-criado (compensacao). `get` baixa **tudo**, confere o SHA-256 contra o `put`, grava em temporario `0600` no destino e so entao `rename` (sem sobrescrever); `--out -` usa buffer de ate 8 MiB e confere antes de escrever; links simbolicos na entrada e na saida sao recusados. **Denylist por nome real** (`.env`, `.credentials.json`, `*.token`, `auth.json`, `config.toml`, `.netrc`, `*.pem`, `*.p12`, `*.key`, `id_*`) e **varredura best-effort no servidor** (cabecalho PEM, `-----BEGIN`, `sk-`, `ghp_`, `refresh_token`) com recusa `secret_suspected`; **nao sao prevencao** (ver Ameaca). Identidade explicita como na v1.6.

### Seguranca, compatibilidade e limites declarados
`blob.*` **exige autenticacao**: com `auth: off`, responde `unauthenticated` e o daemon recusa ligar anexos. **Volta de versao:** o daemon anterior recusa subir com eventos `blob_*` no log ("tipo de evento desconhecido"); depois do primeiro anexo, voltar exige restaurar o backup do estado. Conteudo nunca executado; nenhum nome do cliente vira caminho; o daemon nunca le arquivo por caminho enviado pelo cliente (so bytes).

### Contrato testavel (Parte B)
- **B1** ida e volta: bytes identicos e SHA igual; 0 byte e exatos 8 MiB passam; 8 MiB + 1 `too_large`. **B2** hash/tamanho divergentes aborta e limpa. **B3** pedaco repetido, saltado, maior que `chunk_max`, com conteudo diferente: aborta a sessao, sem arquivo.
- **B4** ACL: terceiro em `get`/`rm`/`list` => `not_found`/ausente, **mesmo codigo e mesmos campos** que para inexistente; dono e destinatario leem.
- **B5** validade (relogio de teste): vencido => `not_found`, arquivo some do disco, `blob_expired`; TTL < 60 s ou > 7 dias recusado; salto de relogio nao prolonga.
- **B6** direto: destinatario `rm` apaga; `get --consume` baixa, confere e remove; `resulted` **nao** remove.
- **B7** nao ha dedup: o mesmo conteudo enviado duas vezes gera dois blobs e dois arquivos; `rm` de um nao afeta o outro; cota cobra os dois.
- **B8** cotas: por arquivo, dono (bytes e contagem), total; recusa sem despejo; reserva no `begin` conta uploads em andamento (4 sessoes concorrentes nao excedem); espaco livre abaixo da reserva => `disk_full`.
- **B9** opt-in: destinatario sem `blobs: true` => `blobs_disabled`; `send --attach` valida antes de subir.
- **B10** reinicio: indice reconstruido; vinculo sem arquivo => `blob_removed reason=missing`; orfaos e `tmp-*` apagados; recusa subir com `blobs/` link simbolico ou com permissao errada.
- **B11** upload: `upload_id` de outra identidade/conexao => `not_found`; prazo absoluto de 10 min; ociosidade 120 s; 2o upload simultaneo da identidade recusado; `n` fora de ordem.
- **B12** privacidade: sentinela no **conteudo** nao aparece no log, `watch`, `status`, erros nem stderr; `name` aparece so no `blob_put` do log (0600) e em `ls` do dono/destinatario.
- **B13** `send --attach`: um anexo; cria o ref `blob`; falha apos o `put` remove o blob; destinatario sem opt-in nao sobe nada.
- **B14** `get --out -`/`--out` em diretorio sem permissao ou sobre link: erro limpo (saida 5 se permissao), sem arquivo parcial e sem `--consume` aplicado.
- **B15** compatibilidade: suites v1.1 a v1.7 e a auditoria F verdes; `blob.*` em daemon antigo => `method_not_found` com mensagem clara; **daemon antigo com eventos `blob_*` no log recusa subir** (documentado).
- **B16 a B19** (B.0): `status` com auth ligada e sem token so estatisticas; nenhum nome aparece; `mailbox.counts` so da propria identidade e ignora seletores (`to`) e troca de identidade; `inbox --count`.
- **B20** corrida: `rm` x `put.end` x coleta x `get` concorrentes no mesmo blob: nunca vinculo sem arquivo nem erro interno; `get` ja aberto termina ou da `not_found` limpo.
- **B21** nao bloqueio: um agente em laco de `put` de 8 MiB nao atrasa `message.send` de outro alem de um limite (medido); saturacao de 64 conexoes.
- **B22** churn: laco `put` de 0 byte + `rm` e TTL minimo batem em `rate_limited` e nao fazem o log crescer alem do orcamento de eventos (30/min por identidade).
- **B23** memoria: 64 conexoes com linhas de quase 1 MiB nao excedem o teto global de buffers; `length` enorme e validado; `list` paginado.
- **B24** `auth: off`: `blob.*` recusado; daemon nao liga anexos.
- **B25** segredos (best-effort): denylist por nome real e por alias; varredura de cabecalho PEM recusa; **teste documenta** que renomear contorna (`secret_suspected` nao e garantia).
- **B26** `ttl_s`/`size` no limite de inteiro (overflow) recusados; `upload_id` previsivel nao existe (128 bits).

## Perguntas abertas (decisao do Jabbur)
1. ~~Validade~~ **Decidido (A13):** 24 h padrao, 7 dias maximo. 2. ~~Tamanhos~~ **Decidido (A14):** 8 MiB, 256 MiB. 3. ~~Remocao automatica ao `resultado`~~ **Decidido (A15): nao.** 4. ~~Broadcast~~ **Decidido (A16): nao.**
5. **[CONFIRMAR] Sem deduplicacao** entre envios (recomendado pelo Opus e pelo Sol). 6. **[CONFIRMAR] Opt-in por destinatario** (padrao desligado; o Jabbur liga um a um).
