# memd v0: daemon de contexto e memoria compartilhada entre agentes (contrato) | RASCUNHO v0 (2026-10-16)

**Estado: RASCUNHO para revisao (Jabbur, Astra, codex-blocks). Nao ha codigo.** Origem: o Jabbur, em 2026-10-16: "a minha arquitetura inclua, como sempre, um daemon que e quem da acesso e controla a memoria. Cada agente recebe uma chave e as intencoes de como usar" e "Vamos fazer o spec do darmin agora" (conversa do Claude Code). **Nome:** `memd` e nome de trabalho; o Jabbur escreveu "darmin" e a grafia/nome final esta em aberto (secao 14). Consultas: Astra (job 996, `/var/tmp/escriba-status/runs/996-MemAstra*`), codex-blocks (chatd msg 610); pendencia P-88 da bancada.

## 0. O que e, e o que nao e
- **E:** um daemon pequeno, estilo unix, **dono** de um armazenamento de registros curados (decisoes, pendencias, regras, handoffs) compartilhado por varios agentes. Cada agente fala por socket com uma **chave** (token) e um **perfil de permissoes**; o daemon impoe "propor x promover x ler" e entrega **visoes com orcamento de tamanho**.
- **Nao e:** o `chatd` (transporte efemero entre agentes), o `grantd` do roadmap (autoridade geral de permissoes; aqui so as permissoes do proprio `memd`, ver secao 7), captura automatica de observacoes, wiki gerada, vetores/embeddings, MCP, nem um banco SQL. **Mecanismo e conteudo sao separados:** o `memd` e o mesmo em todo workspace; cada workspace tem o seu diretorio de estado e o seu conteudo.

## 1. Problema, exemplo, solucao
**Problema.** Cada agente reconstroi contexto de arquivos soltos (CLAUDE.md, AGENTS.md, memoria privada do Claude, `estado-vivo.md`, `pendencias-jabbur.md` com ~57 mil caracteres, handoffs). Um agente nao ve o que o outro sabe; a licao do `ai-memory` (removido em 2026-10-14: 7.142 observacoes automaticas, handoffs abertos para sempre, paginas contradizendo fatos) e que a falha e **curadoria e escrita livre**, nao o armazenamento.
**Exemplo (2026-10-16).** O Jabbur disse "Siga em frente" para a rev6; o codex-blocks bloqueou o pedido porque a spec ainda nao trazia a autorizacao, e so liberou quando o Sextante a escreveu no cabecalho. Com o `memd`: o Sextante grava `D-031` (status `active`, fonte "conversa de 2026-10-16, 'Siga em frente'"); a visao inicial do codex-blocks lista as decisoes vigentes do escopo dele; ele nao bloqueia. Se o Jabbur mudar de ideia, `D-040` **substitui** `D-031` numa unica transacao.
**Solucao.** Log de eventos de acrescimo, um unico processo dono, indices em memoria, permissoes por agente, validacao mecanica na promocao, visoes limitadas.

## 2. Principios (invariantes)
1. **Um dono.** So o `memd` escreve; trava de arquivo exclusiva (secao 5, A5).
2. **O log e a verdade;** todo o resto (indices, visoes, instantaneos, export markdown) e derivado e reconstruivel.
3. **Agentes propoem; so quem tem a permissao `promote` promove** (Jabbur e Sextante no perfil padrao). Nada e promovido automaticamente.
4. **Toda entrada tem fonte, autor e escopo.** Sem fonte, e rejeitada.
5. **Conteudo recuperado e dado, nunca instrucao.** Toda resposta marca `trust:"data"`; o conteudo nunca eleva a permissao de ninguem.
6. **Aposentar, nao apagar.** Registro substituido ou aposentado continua no log e fora das buscas padrao.
7. **Orcamento fixo de contexto:** nenhuma visao excede o limite configurado; exceder exige curadoria, nunca truncamento silencioso.
8. **Fail-closed:** falha de gravacao deixa o daemon somente-leitura ate reiniciar.

## 3. Modelo de dados
**Registro** (`Record`), JSON; campo desconhecido recusado:
- `id` (`D-nnn` decisao, `P-nnn` pendencia, `R-nnn` regra, `H-nnn` handoff; sequenciais por tipo, atribuidos pelo daemon), `type`, `title` (ate 120), `body` (ate `body_max`, padrao 4096 bytes), `status`, `scope` (texto `[a-z0-9._/-]{1,64}`; `*` = global), `visibility` (`private|public`, padrao `private`), `tags` (ate 8, `[a-z0-9_-]{1,24}`), `fields` (objeto validado pelo tipo, ver abaixo).
- **Proveniencia:** `source` (obrigatoria: `{"kind":"commit|doc|quote|msg","ref":"...","at":"data"}`), `author` (id do agente que propos), `promoted_by`, `proposed_seq`, `promoted_seq`, `supersedes`, `superseded_by`, `review_by` (data; obrigatoria para estado volatil: pendencia e handoff).
- **`status`:** `proposed` -> `active` -> `superseded|retired`; `proposed` -> `rejected|expired`. Unica transicao reversa: nenhuma.
- **`fields` por tipo:** `pending`: `owner`, `next_action`, `state` (`open|blocked|done`), `due`; `decision`: `criterion`, `scope_note`; `handoff`: `to`, `accepted_by` (obrigatorio para encerrar). Tipo novo = revisao da spec.
- **Autorizacao nao e tipo do `memd`:** uma decisao pode **referenciar** uma concessao (`fields.grant_ref`), mas conceder poder e do `grantd` (roadmap). O `memd` guarda o fato "o dono autorizou X", nao a execucao.

## 4. Eventos (o log)
Arquivo `log.jsonl` (0600), uma **linha por commit** (secao 5). Cada linha: `{"v":1,"seq":N,"ts":"ISO","tx":[Evento,...]}`; `seq` cresce sem lacunas.
Eventos: `propose` (cria `proposed`), `promote`, `reject`, `supersede` (`old`->`superseded`, `new` ativo, **sempre no mesmo `tx`**), `retire`, `expire` (o daemon, por `proposal_ttl_days`), `edit_proposal` (so o autor, so em `proposed`). Cada evento traz `by` (agente autenticado), `request_id` opcional e o necessario para a projecao. Conteudo de `body` entra no log; o log nunca e copiado para logs de diagnostico do daemon.

## 5. Durabilidade e atomicidade (o que se aproveita do Pi Durable e do chatd)
Fontes lidas no codigo (regra 13): Pi Durable `ref/pi/packages/durable` (tag v1.0.0) e `crates/chatd/src/store.rs`.
- **A1. Unidade de commit = uma linha.** Varios eventos que precisam ser atomicos (ex.: `supersede`) vao no **mesmo `tx` da mesma linha**. O `chatd` hoje grava varios eventos como varias linhas num so `write_all` (`store.rs:565-590`), o que **nao e atomico**: uma queda no meio deixa um prefixo aplicado (aqui seria "velho substituido e novo ausente", isto e, nenhuma versao vigente). A linha unica e a versao simples do "marcador de commit por ultimo" do Pi (`storage/jsonl/storage.ts:288-300`): como o registro e pequeno (`body_max`), nao ha arquivos auxiliares.
- **A2. Recuperacao de cauda cortada.** Na abertura: linha final sem `\n` ou ilegivel = descartada e o arquivo truncado; linha invalida que **nao** e a ultima = recusa de partida, sem adivinhar (como o `chatd`, `store.rs:203-262`). Linha completa = aplicada **inteira**.
- **A3. Gravar antes de aplicar.** Ordem: validar -> serializar -> `write_all` + `sync_data` -> so entao atualizar memoria e responder. O `chatd` aplica antes de gravar e marca `broken` se falhar (`store.rs:565-587`); aqui a memoria nunca adianta o disco. Falha de escrita = modo somente-leitura (`broken`) ate reiniciar.
- **A4. Idempotencia.** `request_id` (`[A-Za-z0-9._-]{1,64}`) por agente: repetir o mesmo `(agente, request_id)` devolve o resultado original sem novo evento; mesmo `request_id` com conteudo diferente = erro `request_conflict`. Inspirado em `harness/submissions.ts:155-159`. O indice de `request_id` e mantido durante `request_retention_days` (padrao 30) e vai no instantaneo.
- **A5. Um dono entre processos.** `flock(LOCK_EX|LOCK_NB)` em `memd.lock`; segundo servidor recusa subir (`store.rs:204-209`). O Pi nao tem isso ("there is no cross-process locking", `README.md:527`); aqui e obrigatorio porque dois donos duplicariam ids.
- **A6. `fsync`.** `sync_data` por commit (o Pi deixa `fsync` desligado por padrao no JSONL, `jsonl/storage.ts:256`; **nao** adotamos o padrao). Instantaneo: escrever em arquivo temporario 0600, `fsync`, `rename`, `fsync` do diretorio. Rotacao de segmento: criar o novo arquivo e `fsync` do diretorio antes do primeiro acrescimo.
- **A7. Nao adotado do Pi, com a razao:** SQLite/WAL/`BEGIN IMMEDIATE` (`storage/sqlite/node.ts:142-157,190-192`: nosso registro cabe numa linha, sem dependencia nova); retomada de tarefas `running`->`pending` com checkpoint (`harness/scheduler.ts:244-248`: nao ha tarefas); politica de repeticao `safe|unsafe` e intencao gravada antes do efeito (`harness/tool.ts:37-38,88-110`: o `memd` nao tem efeito externo a repetir); arquivos auxiliares + marcador.
- **A8. Pontos de queda (cada um testavel colocando o log no estado descrito e subindo o daemon):** (a) antes de gravar: nada mudou, o cliente repete com o mesmo `request_id`; (b) no meio da linha: cauda descartada (A2), idem; (c) linha completa gravada, sem resposta: o registro existe, repetir devolve o original (A4); (d) apos a resposta: duravel.

## 6. Indices, acesso e orcamento
- **Projecao em memoria**, refeita na partida a partir do instantaneo mais recente + o resto do log: por `id`, por `type`, por `status`, por `scope`, por `tags` e lista invertida de termos (`palavra -> ids`) sobre `title`+`tags` (o corpo so por `get`). Estimativa: dezenas de milhares de registros cabem em poucos MB (a medir; o log do `chatd` tem 314 KB para 208 mensagens).
- **Instantaneo** a cada `snapshot_every` commits (padrao 500): estado + `seq` + indice de `request_id`. **Segmentos:** rotacao do log por tamanho (`segment_max`, padrao 4 MiB); registros `superseded|retired|rejected|expired` mais velhos que `archive_after_days` vao para segmento de arquivo (consultaveis com `--include-archived`).
- **Visoes (o que o agente recebe):** `view NOME` e **gerada** por perfil (secao 7) com orcamento `view_budget` em caracteres (padrao 6000): so `active`, escopo permitido, ordenada por regras fixas (tipo -> `review_by` mais proximo -> mais recente), resumo de uma linha por registro. Excedeu: `view_truncated:true` e o agente pede a pagina seguinte; nunca truncamento silencioso.
- **`query`** devolve **resumos** (`id,type,title,status,scope,review_by`, ate 200 caracteres cada), ate `page_max` (50) por pagina, filtros por `type,status,scope,tag,term,author`, `superseded` excluido por padrao. **`get ID`** devolve o completo, com a proveniencia e a cadeia `supersedes`.
- **Cursor:** toda resposta traz `head_seq`; `watch --since SEQ` entrega so o que mudou (mesma ideia do `chatd watch --resume-file`). Resposta maxima: `response_max` (64 KiB).
- **`review_by` vencido** aparece como `unconfirmed` nas visoes e nunca e resolvido sozinho.

## 7. Identidade e permissoes
- **Fase v0 (esta spec):** cada agente tem um **token** (arquivo 0600 do agente, como o `auth.json`/`*.token` do `chatd`); o daemon guarda so o hash. **Perfil** (`grants.json`, editado so pelo Jabbur; nenhuma operacao da API altera permissoes): `agent`, `ops` (subconjunto de `propose,read,promote,reject,supersede,retire`), `read_scopes`, `write_scopes`, `types`, `view` (perfil de visao, ex.: `spec_only` para quem escreve testes independentes, que **nao** ve registros de implementacao), `expires` (data) e `intent` (texto curto de como o agente deve usar o `memd`, devolvido por `whoami` e embutido na visao inicial).
- **Limite honesto:** agentes do mesmo usuario Unix que leem os arquivos de token de outro se passam por ele. Token separa **papeis**, nao isola agentes hostis. Para isolamento real: contas Unix distintas por agente ou sandbox (executores ja rodam na conta `escriba`); fora desta spec.
- **Fase v1 (assinaturas):** promocao de decisao assinada pela chave do dono com `ssh-keygen -Y sign/verify` (namespace `memd`, `allowed_signers`); verificacao **na entrada**, resultado e impressao digital gravados no evento (`signed_by`), reverificacao so sob demanda. Sem dependencia nova (OpenSSH 9.9 local tem `-Y`; **nao testado ainda**). Efeito: "Autorizado por" deixa de ser texto escrito por um agente e passa a ser verificavel. A chave de assinatura do dono vive fora do alcance dos agentes (a definir, secao 14).
- **Fase v2 (fora de maquina):** cifra de transporte entre maquinas/times com construcao revisada (TLS ou `age`), nunca escrita aqui. Chave publica/privada por agente substitui o token.

## 8. Protocolo
JSON por linha em socket Unix `memd.sock` (dir 0700, socket 0600), mesmas convencoes do `chatd`/`agentd` (`{"v":1,"id":..,"method":..,"params":..}`; erro `{"code":"..."}`). Metodos: `whoami`, `propose`, `edit_proposal`, `promote`, `reject`, `supersede`, `retire`, `get`, `query`, `view`, `watch`, `head`, `status`. Codigos de erro fechados: `unauthorized`, `forbidden`, `invalid_params`, `missing_source`, `secret_detected`, `body_too_large`, `request_conflict`, `not_found`, `conflict` (ja ha versao vigente para o `id`), `broken`, `overloaded`.

## 9. CLI (`--json` em todos; codigos 0 ok, 2 uso, 3 permissao, 4 conflito, 5 indisponivel)
`memd serve --state-dir D --config C`; `memd init DIR` (cria diretorio de estado 0700, `config.json` e `grants.json` de modelo **sem tokens** e a **pagina de entrada** `LEIA-ME.md` generica que ensina o agente a ler e propor; idempotente, nunca sobrescreve); `memd propose|promote|reject|supersede|retire|get|query|view|watch|status|whoami`; `memd validate` (offline: confere o log e as regras da secao 10); `memd export DIR` (um markdown por registro, **somente leitura e derivado**, com `visibility` respeitada).

## 10. Validacao mecanica (as regras contra o `ai-memory`)
Aplicadas na promocao, nao so em guia: campos obrigatorios e enums; `source` verificavel quanto a forma; **uma versao `active` por `id`**; `supersede` troca as duas pontas no mesmo commit; `body_max`; `view_budget`; proposta sem promocao expira em `proposal_ttl_days` (padrao 14, com recibo minimo); pendencia/handoff exige `review_by`; handoff so encerra com `accepted_by`; registro `retired` sai da visao ativa; conflito entre dois `active` do mesmo escopo e tema e bloqueado (`conflict`), nunca resolvido sozinho.

## 11. Seguranca e privacidade
- **Segredos:** `propose` rejeita (`secret_detected`) corpo com cabecalho de chave privada, padroes de token conhecidos e as formas de credencial que o operador listar; apagar depois nao limpa o historico do git, entao a barreira e na entrada.
- **Publico x privado:** `visibility` por registro; `export` e o unico caminho para fora do daemon e recusa `private` quando o destino esta marcado `public` (o repositorio `blocks` e publico). A memoria privada do Sextante (`docs/memoria-sextante`) **nao** e importada.
- **Independencia dos testes:** o perfil `spec_only` nao enxerga registros de implementacao; testado.
- Sem rede. Nenhum texto de `body` em logs de diagnostico, `status` ou mensagens de erro (so ids e codigos).

## 12. Contrato testavel (testes independentes; daemon real em diretorio temporario, sem rede)
- **Atomicidade/durabilidade:** T1 cauda cortada descartada e o resto aplicado; T2 linha invalida no meio recusa subir; T3 `supersede` com cauda cortada nao deixa estado parcial; T4 `request_id` repetido devolve o original e nao cria evento; T5 `request_id` com conteudo diferente = `request_conflict`; T6 `flock`: segundo daemon recusa; T7 falha de escrita (diretorio sem espaco/readonly simulado) = `broken`, memoria nao adianta o disco; T8 instantaneo + resto do log = mesmo estado que o log inteiro; T9 `seq` sem lacunas.
- **Permissao:** T10 cada `op` negada sem a permissao; T11 escopo de leitura/escrita; T12 `spec_only` nao ve registros de implementacao; T13 token errado/expirado; T14 `whoami` devolve o `intent`; T15 nenhuma operacao da API altera `grants.json`.
- **Validacao:** T16 sem fonte; T17 segredo; T18 `body_max`; T19 duas versoes `active`; T20 proposta expira; T21 handoff sem aceite nao encerra; T22 `review_by` vencido = `unconfirmed`.
- **Acesso:** T23 `view` respeita `view_budget` e marca `view_truncated`; T24 `query` so resumos, pagina e filtros, `superseded` excluido; T25 `watch --since` entrega so o novo; T26 arquivamento nao some da busca com `--include-archived`.
- **Operacao:** T27 `init` idempotente e sem tokens; T28 `export` e somente leitura e respeita `visibility`; T29 `validate` pega log corrompido; T30 conteudo nunca aparece em logs de diagnostico (sentinela).

## 13. Fases e aceite
- **v0:** secoes 3 a 12 sem assinaturas. **v1:** promocao assinada (secao 7). **v2:** fora de maquina e cifra. O `journald`/`grantd` do roadmap podem absorver partes; decisao na secao 14.
- **Piloto (P-88):** 10 decisoes + 10 pendencias reais; sessoes limpas de Claude e Codex antes e depois; metas: >= 30% menos tokens para abrir, 10/10 estados de pendencia certos, <= 15 min/dia de manutencao; **segundo workspace criado so com `memd init`** e um agente novo (Haiku sem contexto) que acerta uma consulta e uma proposta sem ser ensinado. Parar se falhar.

## 14. Em aberto (decisao do Jabbur ou dos agentes)
1. **Nome** (`memd`, ou o que o Jabbur quis dizer com "darmin"). 2. **`memd` x `journald`:** fundir (recomendacao do Astra) ou blocos separados. 3. **Onde mora a chave de assinatura do dono** (v1). 4. Se o codex-blocks le a bancada privada a partir do workspace dele (afeta onde fica o diretorio de estado). 5. Contas Unix por agente (isolamento real). 6. Quem cura quando o Sextante nao esta.

## 15. Fora desta spec
Captura automatica, vetores, MCP, SQL, `leased`, agendador de tarefas, execucao, concessao de poder (e do `grantd`), cifra e rede (v2), UI.
