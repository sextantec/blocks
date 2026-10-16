# memd v0: daemon de memoria curada compartilhada entre agentes (contrato normativo) | RASCUNHO v0 rev1 (2026-10-16)

**Estado: RASCUNHO rev1 para revisao (substitui a rev0 depois das revisoes do Astra, job 997, e do codex-blocks, msg 621). Nao ha codigo.** Origem: o Jabbur, 2026-10-16: "a minha arquitetura inclui, como sempre, um daemon que e quem da acesso e controla a memoria. Cada agente recebe uma chave e as intencoes de como usar"; nome confirmado ("memd esta otimo"); "vamos fazer o memd e o grantd, specs e depois codigo". Motor de gravacao: `docs/blocklog-spec-v0.md` (normativo; este documento nao o repete). Irmao: `docs/grantd-spec-v0.md`. Pendencia da bancada: P-88.

## 0. O que e, e o que nao e
- **E:** daemon dono de um registro de **decisoes** e **pendencias** curadas. Cada agente fala por socket com uma **chave** e um **perfil** (o que pode fazer, em que escopo, e a "intencao" de uso). Agentes **propoem**; so quem tem `promote` torna ativo. Entrega **visoes com orcamento de tamanho**.
- **Nao e:** o `chatd` (transporte), o `grantd` (poder no mundo: escrever arquivos, instalar, gastar), captura automatica, vetores, SQL, nem sandbox. O texto recuperado e **dado**; o `memd` nao impede um consumidor de obedecer a um texto (o metadado `trust:"data"` e aviso, nao garantia).
- **Escopo da v0 (cortado depois das revisoes):** tipos `decision` e `pending`; operacoes `propose`, `promote`, `reject`, `retire`, `get`, `query`, `view`, `whoami`, `status`. **Fica para v0.2:** instantaneos/rotacao/arquivo, indice invertido, `watch`, `resume`, `export`, `edit_proposal`, tipos `rule` e `handoff`, `visibility`; assinaturas na v1.

## 1. Problema, exemplo, solucao
Ver P-88. Exemplo: o Jabbur diz "Siga em frente" para a rev6; o Sextante propoe e promove `D-031`; o codex-blocks abre a sessao, a visao dele lista `D-031` (escopo `agentd`) e ele nao bloqueia. Se o Jabbur mudar de ideia, `D-040` entra com `supersedes:["D-031"]` e os dois efeitos da promocao sao **um commit so**.

## 2. Invariantes
1. Um dono (`blocklog`: `flock`); todas as mutacoes passam por **um unico ponto de linearizacao** (secao 4).
2. O log e a verdade; a memoria e a projecao reconstruida dele.
3. Agentes propoem; promocao exige `promote`. Nada e promovido sozinho.
4. Toda entrada tem `source`, `author` e `scope`.
5. Aposentar, nunca apagar. 6. Fail-closed: `broken` = somente leitura ate reiniciar.
7. Id inexistente e id sem permissao de leitura respondem **igual** (`not_found`).

## 3. Modelo
**Identidade:** `id` logico (`D-nnn`, `P-nnn`, alocado pelo daemon) + `rev` (1, 2, ...). Chave de uma versao: `(id, rev)`. Ha no maximo **uma** `rev` `active` por `id`.
**Registro** (campo desconhecido recusado; o cliente **nao** envia `author`, `seq`, `ts`, `status`, `promoted_by`, `proposed_seq`, `promoted_seq`, `superseded_by`: sao do daemon):
- `id`, `rev`, `type` (`decision|pending`), `title` (1 a 120 caracteres Unicode), `body` (0 a 4096 bytes UTF-8), `scope` (`[a-z0-9._/-]{1,64}`; global = `global`), `tags` (0 a 8 de `[a-z0-9_-]{1,24}`), `fields` (secao 3.1), `source` (obrigatorio: `{"kind":"commit|doc|quote|msg","ref":"ate 200 caracteres","at":"YYYY-MM-DD"}`), `refs` (0 a 8 de `{"kind":"chatd|git|doc|grant","ref":"ate 200"}`; o `memd` so guarda e devolve), `supersedes` (0 a 4 ids), `status`, `author`, `promoted_by`, `proposed_seq`, `promoted_seq`, `superseded_by` (`"id@rev"`), `review_by` (`YYYY-MM-DD`).
- **`status` por versao:** `proposed`, `active`, `superseded`, `retired`, `rejected`, `expired`. Datas em `YYYY-MM-DD` (UTC); instantes em RFC 3339 UTC (os do `blocklog`).
### 3.1 `fields` por tipo (objeto fechado)
- `decision`: `criterion` (string, ate 400 caracteres, obrigatorio). `review_by` opcional.
- `pending`: `owner` (id de agente, obrigatorio), `next_action` (ate 240, obrigatorio), `state` (`open|blocked|done`, obrigatorio), `due` (data ou ausente). `review_by` **obrigatorio**.
- **Pendencia `done`** continua `active` e **sai** da visao e do `query` padrao (aparece com `state=done` explicito).
- **`review_by` vencido** (data anterior a hoje UTC, `status active`) e atributo **derivado** `unconfirmed:true`, nunca um status; nao e resolvido sozinho.

## 4. Mutacoes (tabela normativa; cada linha e **um** commit do `blocklog`)
Todas exigem `request_id` (secao 6). Ponto de linearizacao: a validacao, a alocacao de id/rev, a checagem de precondicao e o `commit` rodam sob **um mutex**; a projecao em memoria so e atualizada depois de o `commit` ter sucesso, e a troca e visivel aos leitores de uma vez (nenhum prefixo de `tx` e observavel).
| Metodo | Quem (op) | Precondicao | Evento(s) no `tx` | Efeito | Erros |
| --- | --- | --- | --- | --- | --- |
| `propose` (novo) | `propose` + escopo de escrita | sem `id`; agente com menos de `max_open_proposals` abertas; campos validos; sem segredo | `{"e":"propose",...,"id":novo,"rev":1}` | versao `proposed` | `invalid_params`, `missing_source`, `secret_detected`, `body_too_large`, `quota`, `forbidden` |
| `propose` (revisao) | idem | `id` existe e e **legivel** pelo agente; `expected_rev` = `rev` da versao `active` atual (0 se nao ha) | `{"e":"propose",...,"id":X,"rev":n+1}` | nova versao `proposed` | `not_found`, `stale_rev` |
| `promote` | `promote` + escopo | versao `proposed` existe e e legivel; `expected_seq` = `proposed_seq` (evita promover versao diferente da revista); todo id em `supersedes` existe, e `active`, e legivel e gravavel | `{"e":"promote","id","rev","by"}` | versao -> `active`; a `active` anterior do mesmo `id` -> `superseded` (`superseded_by`); cada id de `supersedes` com `active` -> `superseded` (mesmo `tx`) | `not_found`, `stale_seq`, `conflict`, `validation` |
| `reject` | `promote` + escopo | versao `proposed` | `{"e":"reject","id","rev","by","reason"}` (`reason` ate 200) | -> `rejected` | `not_found` (inexistente ou ilegivel), `conflict` (legivel, mas a versao nao esta `proposed`) |
| `retire` | `promote` + escopo | versao `active` | `{"e":"retire","id","rev","by","reason"}` | -> `retired` | `not_found` (inexistente ou ilegivel), `conflict` (legivel, mas a versao nao esta `active`) |
| `expire` (daemon) | o proprio daemon (`by:"memd"`) | versao `proposed` com idade >= `proposal_ttl_days` | `{"e":"expire","id","rev","by":"memd"}` (ate 16 por `tx`) | -> `expired` | n/a |
- **Imutabilidade:** uma versao promovida nunca muda; alterar uma pendencia ativa e `propose` de revisao + `promote`. Nao ha `edit_proposal` (rejeitar e repropor).
- **Substituicao e um commit so:** promover revisao ou `supersedes` mexe nas duas pontas no **mesmo** `tx`; queda no meio nao deixa estado parcial (`blocklog` G3).
- **`expire`:** o daemon verifica na partida e a cada 60 s, com o relogio do `blocklog` (nao regressivo).

## 5. Autorizacao (matriz normativa)
**Perfil** em `grants.json` (0600; editado so pelo dono; carregado na partida e em `SIGHUP`; nenhuma operacao da API o altera): lista de `{agent, token_sha256, ops, read_scopes, write_scopes, types, expires, intent}`.
- `ops` subconjunto de `propose,promote` (leitura e `whoami` sempre permitidas ao agente valido). `read_scopes`/`write_scopes`: lista de padroes; `*` = qualquer, `a/b` = exato, `a/**` = `a` e tudo abaixo. `types`: lista (`decision`,`pending`). `expires`: data (token invalido depois). `intent`: texto (ate 400) devolvido por `whoami`.
- **Leitura de um registro** exige: token valido, `type` em `types` e `scope` casando `read_scopes`. Vale para `get`, `query`, `view`, cadeias (`superseded_by`, `supersedes` so mostram ids legiveis; os demais saem como `"(oculto)"`) e para a repeticao idempotente (a autorizacao e conferida de novo antes de devolver o `res` guardado).
- **Escrita** exige a `op` e `write_scopes` casando; `promote` exige tambem escopo de **escrita** no registro e em **todas** as pontas de `supersedes`.
- **Inacessivel e inexistente respondem `not_found`**, tambem em mutacao cega. `status` devolve `head_seq` e contagens globais por `status` (sem escopos); documentado como revelando so o volume.
- **Token:** `mt_` + 43 caracteres base64url (256 bits de `/dev/urandom`); o daemon guarda so o SHA-256 em hexadecimal; o cliente le o token de um arquivo (`--token-file` ou `MEMD_TOKEN_FILE`), nunca por argumento; o token vai em cada requisicao (`"token"`), para que a revogacao (remover do `grants.json` + `SIGHUP`) valha na hora, inclusive em conexao aberta. Token nunca em log de diagnostico, mensagem de erro, `status` ou `whoami`.
- **Cotas:** `max_open_proposals` por agente (padrao 20), `max_records` global (20000), `max_connections` (32), requisicao <= `line_max`. Estourar = `quota`/`overloaded`.
- **Limite honesto:** agentes do mesmo usuario Unix que leem o arquivo de token de outro se passam por ele, e qualquer processo do mesmo usuario pode editar `log.jsonl`. O token separa papeis; nao isola agentes hostis. A v1 (assinatura do dono, `ssh-keygen -Y`) e as contas Unix por agente fecham o resto.

## 6. Protocolo
Socket Unix `memd.sock` (diretorio 0700, socket 0600). Uma requisicao JSON por linha: `{"v":1,"id":"<cliente>","token":"mt_...","method":"...","params":{...}}`; resposta: `{"v":1,"id":"<igual>","ok":true,"result":{...}}` ou `{"v":1,"id":"..","ok":false,"error":{"code":"...","message":"curta, sem conteudo"}}`. Resposta <= `response_max` (65536 bytes).
- **`request_id`** (`params.request_id`, `[A-Za-z0-9._-]{1,64}`) **obrigatorio nas mutacoes**; carga canonica para o hash = `method` + `params` sem `request_id`, em JSON de chaves ordenadas e sem espacos. Repeticao com mesma carga devolve o resultado original (`replayed:true`); outra carga = `request_conflict`.
- **Parametros e resultados:**
  - `propose`: `{type?,id?,expected_rev?,title,body,scope,tags?,fields,source,refs?,supersedes?,review_by?,request_id}` -> `{id,rev,status:"proposed",seq}`.
  - `promote`: `{id,rev,expected_seq,request_id}` -> `{id,rev,status:"active",superseded:[...],seq}`. `reject`/`retire`: `{id,rev,reason?,request_id}` -> `{id,rev,status,seq}`.
  - `get`: `{id,rev?}` (sem `rev` = a `active`, ou a mais recente se nenhuma) -> registro completo + `unconfirmed` + `head_seq`.
  - `query`: `{type?,status?(padrao active),scope?,tag?,term?,author?,state?,after?,limit?(1..50, padrao 20)}` -> `{items:[{id,rev,type,title,status,scope,review_by,unconfirmed}],next_after,head_seq}`. `term` = substring, dobra so `A-Z` ASCII, em `title` e `tags`; varredura (sem indice). Ordenacao fixa: tipo (`decision` antes de `pending`), depois numero do `id`. **Paginacao por chave** (`after` = ultimo `id`): nenhum item que existia durante toda a paginacao e pulado.
  - `view`: `{name?}` -> visao do perfil: secao `due` (ate `view_overdue_max`, padrao 5, os `unconfirmed` mais antigos) e secao `active` (ordem de `query`), uma linha por item (`id | title | scope | review_by`), contada em caracteres Unicode, ate `view_budget` (padrao 6000; faixa 1000 a 20000). Item que nao cabe: `view_truncated:true`, `next_after`, e o **cursor so avanca sobre item entregue**. Inclui o `intent` do agente no topo (conta no orcamento).
  - `whoami`: `{}` -> `{agent,ops,read_scopes,write_scopes,types,expires,intent}`. `status`: `{}` -> `{head_seq,counts,broken,uptime_s}`.
- **Codigos de erro (fechados) -> saida da CLI:** `unauthorized`(3), `forbidden`(3), `not_found`(4), `conflict`(4), `stale_rev`(4), `stale_seq`(4), `request_conflict`(4), `invalid_params`(2), `missing_source`(2), `secret_detected`(2), `body_too_large`(2), `validation`(2), `quota`(5), `overloaded`(5), `broken`(5). Sucesso 0; uso 2.

## 7. Validacao mecanica
Proposta: formato e limites da secao 3; `source` com `kind` valido, `ref` nao vazio, `at` data valida; `fields` do tipo; `review_by` valido e, em `pending`, presente e hoje ou futuro; **todo** campo textual (titulo, corpo, tags, strings de `fields`, `source.ref`, `refs.ref`, `reason`) passa pela varredura de segredos. Promocao: os `expected_*`, a regra "uma `active` por `id`" e `conflict` quando a versao a promover tem o **mesmo `type`, `scope` e `title`** (igualdade exata apos aparar espacos e dobrar `A-Z`) de outra `active` com `id` diferente e nao listada em `supersedes`.
- **Varredura de segredos (heuristica; nao e garantia):** cabecalho `-----BEGIN` ... `PRIVATE KEY-----`; `AKIA` + 16 de `[0-9A-Z]`; `ghp_` + 36 alfanumericos; `sk-` + 20 ou mais de `[A-Za-z0-9_-]`; `xox[baprs]-` + 10 ou mais; JWT (`eyJ...` tres partes base64url de 10 ou mais separadas por `.`); mais `secret_patterns` do operador (substrings literais, sem regex). Implementada sem dependencia de regex. Padroes novos ou codificacoes escapam; os testes cobrem **exatamente** esta lista e o campo de cada ocorrencia.
- `proposal_ttl_days` (padrao 14; faixa 1 a 90), `view_budget`, `view_overdue_max` (1..20) e as cotas sao chaves de `config.json`; faixa invalida = recusa de partida; chave desconhecida recusada.

## 8. CLI (`--json` em todos; codigos da secao 6)
`memd serve --state-dir D --config C --grants G [--socket S]`; `memd init DIR` (cria o diretorio de estado 0700, `config.json` e `grants.json` de **modelo, sem tokens**, e `LEIA-ME.md` generico que ensina o agente a ler/propor; idempotente; nunca sobrescreve); `memd propose|promote|reject|retire|get|query|view|whoami|status --token-file F [--params JSON|-]` (**`--params` e o objeto `params` da secao 6, em JSON; `-` le o JSON da entrada padrao; ausente = `{}`; a CLI so monta o envelope e imprime a resposta. Flags por campo ficam para a v0.2, aditivas**); `memd validate --state-dir D` (offline: confere o log com o `blocklog` e as invariantes: uma `active` por `id`, ids sem lacuna por tipo, `supersedes` consistentes); `memd token-hash` (le o token de stdin e imprime o SHA-256, para o dono montar o `grants.json`).

## 8A. API de biblioteca (normativa; os testes de tempo, falha e protocolo compilam contra ela)
O crate `memd` e **biblioteca + binario**. O socket e a CLI sao uma camada fina sobre `Service`; as regras vivem em `Service` e sao testadas ali, de forma deterministica (relogio e disco injetados via `blocklog`); socket, CLI, `SIGHUP`, `init` e `token-hash` sao testados no binario.
```rust
pub struct Config { /* chaves da secao 7: proposal_ttl_days, view_budget, view_overdue_max, max_open_proposals, max_records, max_connections, secret_patterns, line_max, response_max */ }
impl Config { pub fn from_json(s: &str) -> Result<Config, String>; }          // faixa invalida/chave desconhecida = Err
impl Default for Config { /* padroes da spec */ }
pub struct Grants { /* perfis da secao 5 */ }
impl Grants { pub fn from_json(s: &str) -> Result<Grants, String>; }
pub fn token_sha256_hex(token: &str) -> String;                                 // SHA-256 hex minusculo do token

pub struct Service { /* privado */ }
impl Service {
    pub fn new(cfg: Config, grants: Grants, log: blocklog::Log, replay: Vec<blocklog::Commit>,
               clock: Box<dyn blocklog::Clock>) -> Result<Service, String>;     // reconstroi o estado dos commits; log invalido = Err
    pub fn handle_line(&mut self, line: &str) -> String;   // uma requisicao (secao 6) -> uma resposta, sem '\n'; erros viram resposta {ok:false}
    pub fn tick(&mut self);                                // roda a expiracao da secao 4 com o relogio injetado
    pub fn reload_grants(&mut self, grants: Grants);       // equivale ao SIGHUP
    pub fn head_seq(&self) -> u64;
    pub fn is_broken(&self) -> bool;
}
```
`handle_line` nunca entra em panico com entrada arbitraria (linha maior que `line_max`, JSON invalido, UTF-8 invalido tratado antes pelo chamador = `invalid_params`). A concorrencia (32 clientes) e do servidor de socket, que serializa as chamadas a `Service` sob um mutex.

## 9. Seguranca e privacidade
Sem rede. Texto de `body`/`title` nunca em logs de diagnostico, `status` ou mensagens de erro (so ids e codigos). A memoria privada do Sextante nao e importada. O repositorio publico `blocks` nao contem estado do `memd`; o estado vive no diretorio do dono. Conteudo e dado, nunca instrucao; a spec nao promete que um consumidor ignore ordens dentro de um texto.

## 10. Contrato testavel (testes independentes; tempo, falhas e protocolo por `Service` (secao 8A) com `blocklog::Storage`/`Clock` falsos; socket, CLI, `SIGHUP`, `init`, `token-hash`, `validate` e concorrencia pelo binario em diretorio temporario; sem rede)
Formato: **pre-condicao -> acao -> oraculo exato.** Os testes do motor estao em `blocklog-spec-v0.md` (B1-B28) e nao se repetem aqui.
- **Mutacoes:** M1 `propose` novo -> `D-001` rev 1 `proposed`; ids por tipo sem lacuna e sem repeticao (32 `propose` simultaneos = 32 ids distintos); M2 revisao com `expected_rev` errado = `stale_rev`; M3 `promote` com `expected_seq` errado = `stale_seq`; M4 `promote` de revisao deixa a anterior `superseded` com `superseded_by`, e ambas mudam no **mesmo** commit (cauda cortada: nenhuma ou ambas); M5 `supersedes` de id `active` o marca `superseded` no mesmo commit; M6 `supersedes` de id inexistente/ilegivel = `not_found` e **nada** muda; M7 `reject`/`retire` mudam so o estado esperado; M8 `expire`: proposta com idade >= `proposal_ttl_days` vira `expired` (relogio injetado), menor nao; M9 relogio regressivo nao expira antes nem ressuscita; M10 duas `active` do mesmo `type`/`scope`/`title` = `conflict`; M11 `max_open_proposals` = `quota`.
- **Idempotencia:** I1 mesmo `request_id` e carga: um unico commit e resposta igual com `replayed:true`; I2 mesmo `request_id` e carga diferente = `request_conflict`; I3 repeticao depois de reiniciar o daemon devolve o original; I4 repeticao apos perder o direito de leitura do registro devolve `not_found`, nao o resultado; I5 mutacao sem `request_id` = `invalid_params`.
- **Autorizacao:** A1 sem `op` = `forbidden` em cada mutacao; A2 `write_scopes` nao casa = `forbidden`; A3 `read_scopes` nao casa: `get`, `query`, `view` e cadeias nao mostram nem confirmam o registro (`not_found`/`(oculto)`); A4 id existente sem permissao e id inexistente: respostas **identicas**, inclusive em mutacoes cegas; A5 token errado/expirado/removido apos `SIGHUP` = `unauthorized`, tambem em conexao ja aberta; A6 `types` restringe; A7 `whoami` devolve o `intent`; A8 nenhuma operacao altera `grants.json`; A9 `status` nao revela escopos; A10 perfil `spec_only` (so escopo `spec`) nao enxerga `impl` por metodo algum.
- **Validacao:** V1 sem `source` = `missing_source`; V2 segredo (um caso por padrao da lista, em **cada** campo textual) = `secret_detected`; V3 `body` > 4096 = `body_too_large`; V4 `pending` sem `review_by`/`owner`/`next_action`/`state` recusado; V5 campo desconhecido e campos reservados enviados pelo cliente (`author`, `status`...) recusados; V6 `review_by` vencido -> `unconfirmed:true` na `view`, status continua `active`; V7 `pending` `done` sai da `view` e do `query` padrao.
- **Acesso:** Q1 `query` ordena por tipo e id e pagina por `after` sem pular item existente (mutacoes concorrentes); Q2 `term` dobra so `A-Z`; Q3 `limit` > 50 recusado; Q4 `view` nunca excede `view_budget` em caracteres Unicode e marca `view_truncated`; Q5 item maior que o orcamento restante: o cursor nao avanca sobre ele; Q6 a secao `due` respeita `view_overdue_max`; Q7 resposta > `response_max` nunca e enviada (`overloaded`).
- **Operacao:** O1 `init` idempotente, sem tokens, sem sobrescrever; O2 `validate` pega duas `active` do mesmo `id`, lacuna de id e `supersedes` quebrado; O3 token nunca em log de diagnostico, `status`, `whoami` ou erro (sentinela); O4 estado reconstruido do log inteiro igual ao vivo (200 mutacoes pseudoaleatorias com semente fixa, reinicio, `query` completa igual); O5 `broken` (falha injetada): mutacoes `broken`, leituras continuam.

## 11. Fases e aceite
- **v0 (esta spec)** -> **v0.2:** instantaneos/rotacao, indice, `watch`, `resume`, `export`, `rule`/`handoff`, `edit_proposal` se o piloto pedir -> **v1:** promocao assinada pelo dono (`ssh-keygen -Y`; verificacao na entrada, resultado gravado) -> **v2:** fora de maquina e cifra com construcao revisada.
- **Piloto (P-88):** 10 decisoes + 10 pendencias reais; sessoes limpas de Claude e Codex antes e depois; metas: >= 30% menos tokens para abrir, 10/10 estados de pendencia certos, <= 15 min/dia de manutencao; **segundo workspace criado so com `memd init`**, e um agente novo (Haiku sem contexto) acerta uma consulta e uma proposta sem ser ensinado. Parar se falhar.

## 12. Em aberto
1. Onde mora a chave de assinatura do dono (v1). 2. Contas Unix por agente. 3. Quem cura quando o Sextante nao esta (requisito do piloto). 4. Se `memd.promote` deve consultar o `grantd` ou manter o proprio perfil (hoje, perfil proprio).
