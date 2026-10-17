# grantd v0: autorizacoes legiveis por maquina, com validade e revogacao (contrato normativo) | RASCUNHO v0 rev1 (2026-10-16)

**Estado: RASCUNHO rev1.1 para revisao (secao 6A fecha lacunas de campos e codigos achadas pelos testes independentes). Nao ha codigo.** Origem: roadmap dos `blocks` (item 3: "machine-readable authorisations: may this agent do this, until when"); o Jabbur, 2026-10-16: "Estou convencido de que o memd e grantd ajudam muito no processo" e "vamos fazer o memd e o grantd, specs e depois codigo". Aplica ao `grantd` as licoes das revisoes do `memd` (Astra, job 997; codex-blocks, msg 621). Irmao do `memd` (`docs/memd-spec-v0.md`); **mesmo motor de gravacao**: `docs/blocklog-spec-v0.md` (normativo; nao repetido aqui).

## 0. O que e, e o que nao e
- **E:** daemon dono de um registro de **concessoes** ("o agente A pode fazer a acao X sobre o recurso R, ate D, concedido por Q, por esta fonte"). `check` responde com **negacao por padrao**; `use` conta consumo; ha delegacao com escopo menor e revogacao em cascata.
- **Nao e:** sandbox (nao impede um processo de fazer o que o sistema operacional deixa), o `memd` (fatos), o `ledger` (saldos e precos; o `grantd` so conta unidades que o chamador declara), executor, nem autoridade sobre o proprio perfil do `memd`. **Um verificador que ninguem consulta nao protege nada** (secao 9): a garantia vale so onde ha um ponto de integracao que pergunta antes de agir.
- **Escopo da v0:** acoes e recursos da secao 4; `check`, `use`, `explain`, `list`, `show`, `request`, `delegate`, `grant`, `deny_rule`, `revoke`, `approve_request`, `reject_request`, `whoami`, `status`. **Fica para v0.2:** `watch`, `export`, instantaneos, hook empacotado do Claude (`grantd hook claude`); v1: assinatura do dono.

## 1. Problema, exemplo, solucao
Hoje as autorizacoes do dono vivem em prosa (`docs/autorizacoes-jabbur.md` A1..A21, regras 1 a 8 do `CLAUDE.md`, a lista `ALLOWED` do escriba). Os agentes as seguem por disciplina; um agente pode alegar "o Jabbur autorizou" sem prova, e outro pode ignorar uma autorizacao que nao leu. Exemplo (2026-10-16): o codex-blocks bloqueou a rev6 porque a autorizacao nao estava escrita onde ele olha. Com o `grantd`: `G-014` (grantee `codex-blocks`, acao `fs.write`, recurso `/home/USUARIO/Projects/blocks/crates/agentd/src/**`, `until` 2026-10-17T03:00:00Z, fonte "Siga em frente"). O hook ou script pergunta `check` e recebe `allow G-014` ou `deny` com o motivo. Vencido ou revogado, o proximo `check` ja nega.

## 2. Invariantes
1. **Negar por padrao.** 2. **`deny` vence `allow`.** 3. **So o dono (papel `owner`) cria poder novo**; agentes pedem (`request`) ou delegam subconjunto do que tem. 4. **Validade obrigatoria** (`until`), salvo `permanent` (so o dono, destacado em `list`). 5. **Revogar vale na hora.** 6. Decisao pura e explicavel: depende so do log e do relogio. 7. Um dono (`flock`), um ponto de linearizacao (secao 5). 8. Fail-closed (`broken` = somente leitura). 9. Id inexistente e id sem permissao de leitura respondem **igual** (`not_found`).

## 3. Modelo
**Concessao** (`G-nnn`, alocada pelo daemon; campo desconhecido recusado; o cliente **nao** envia `id`, `issuer`, `status`, `seq`, `signed_by`):
- `effect` (`allow|deny`), `grantee` (id de agente `[a-z0-9_-]{1,32}` ou `*`), `action`, `resource` (secao 4), `constraints` (opcional, so em `allow`: `max_uses` inteiro >= 1; `max_amount` numero >= 0 com `unit` string `[a-z0-9._-]{1,24}`; `window_s` 1..31536000, ausente = vida da concessao), `not_before` (RFC 3339 UTC, padrao agora), `until` (RFC 3339 UTC, > `not_before`; **obrigatorio** salvo `permanent:true`), `permanent` (bool, padrao falso), `delegable` (bool, padrao falso, so em `allow`), `source` (obrigatorio: `{"kind":"quote|doc|msg|commit","ref":"ate 200","at":"YYYY-MM-DD"}`), `task_ref` (opcional, ate 64 caracteres, id de pendencia do `memd`), `parent` (id da concessao pai, se delegada), `issuer` (`owner` ou o id do agente que delegou).
- **`status` derivado** (nunca gravado): `active` (agora em `[not_before, until)` ou `permanent`, e nao revogada), `expired`, `revoked`, `future`. `Pedido` (`request`) tem status proprio: `pending|approved|rejected`.
- **Constantes da v0:** `max_depth` = 2 (dono -> delegado -> delegado), `max_children` = 3 por concessao (a cascata de revogacao cabe em um `tx` de 13 eventos, <= `tx_max` 16).

## 4. Acoes e recursos (lista fechada; gramatica exata)
| `action` | `resource` (em `check`/`use`: valor concreto; em concessao: padrao) | risco |
| --- | --- | --- |
| `fs.read`, `fs.write` | caminho absoluto canonico (sem `.`/`..`, sem `//`, sem `/` final, exceto `/`); no padrao: `*` = um segmento, `**` = zero ou mais segmentos | `fs.write` = alto |
| `exec` | caminho absoluto canonico do binario | alto |
| `net` | `host` ou `host:porta`, minusculas ASCII | alto |
| `spend` | `[a-z0-9._:/-]{1,80}` (ex.: `openai:gpt-6-astra`) | alto |
| `git.commit`, `git.push` | caminho absoluto do repositorio | `push` = alto |
| `install`, `service.restart` | `[a-z0-9._-]{1,64}` | alto |
| `delegate` | id de agente | alto |
Acao fora da lista = `invalid_action` (erro de uso, nao `deny`). `resource` nao canonico, com glob em `check`/`use` ou fora da gramatica da acao = `invalid_params`. **O chamador canonicaliza** (o daemon nao resolve symlink nem toca o disco). Casamento: por segmentos, sem regex.

## 5. Mutacoes e avaliacao
Todas as mutacoes exigem `request_id` (mesma regra do `memd`, secao 6 dele) e rodam sob **um mutex**: validar, alocar id, checar precondicao e `commit` do `blocklog`; a projecao so muda depois de o `commit` ter sucesso.
| Metodo | Quem | Precondicao | Evento(s) no `tx` | Erros |
| --- | --- | --- | --- | --- |
| `grant` | `owner` | campos validos; `until` ou `permanent`; fonte | `{"e":"grant",...}` | `forbidden`, `missing_until`, `missing_source`, `invalid_params` |
| `deny_rule` | `owner` | idem (`effect:deny`) | `{"e":"grant","effect":"deny",...}` | idem |
| `revoke` | `owner` | concessao existe e nao revogada | `{"e":"revoke","id","by","reason"}` + um `revoke` por filho ativo, em cascata (ate 12) | `forbidden`, `not_found` |
| `request` | agente com `can_request` | acao/recurso validos; `until` pedido; ate 5 pedidos `pending` por agente | `{"e":"request",...}` | `quota`, `invalid_params` |
| `approve_request` | `owner` | pedido `pending`; `until` aprovado <= pedido (opcional estreitar) | `{"e":"approve","req","by"}` + `{"e":"grant",...}` (issuer `owner`, fonte = o pedido) | `not_found`, `forbidden` |
| `reject_request` | `owner` | pedido `pending` | `{"e":"reject_req","req","by","reason"}` | `not_found` |
| `delegate` | agente com `can_delegate` | tem `allow` ativa `delegable` que casa; a nova tem `action` igual, `resource` **contido**, `until` <= o do pai (pai `permanent` -> `until` finito obrigatorio), `constraints` <= as do pai, profundidade < `max_depth`, filhos < `max_children`; `delegable` do filho so se profundidade restante > 0 | `{"e":"grant",...,"parent":P,"issuer":agente}` | `exceeds_parent`, `forbidden`, `quota` |
| `use` | agente com a concessao | = `check` allow **e** consome (`max_uses`/`max_amount` na janela) | `{"e":"use","agent","grant","action","resource","qty"}` | `denied` (tambem em `check`) |
**`check(agent, action, resource, qty?)`** (somente leitura; **nao grava, nao tem `request_id`**): (1) reune as concessoes `active` com `grantee` igual ao agente ou `*`, mesma `action` e `resource` casando; (2) qualquer `deny` casando -> `deny` (`denied_by`: id); (3) senao uma `allow` casando e com folga nas `constraints` (para `qty` informado: `max_amount` na janela suficiente) -> `allow` com o id da de menor `until`; (4) senao `deny`: `exhausted` se casou mas sem folga, senao `no_grant`. Resultado: `{"decision":"allow|deny","grant":"G-014"|null,"reason":"ok|no_grant|denied_by|exhausted","until":...,"signed":false}`. **Consumo so por `use`.** `explain(agent, action, resource)`: o mesmo, mais a lista das concessoes que casaram e a regra aplicada (agente: so o proprio; owner: qualquer).
**Relogio:** o daemon usa o relogio do `blocklog` (`max(agora, ultimo ts)`); relogio para tras nao reviva concessao vencida nem reabra janela. **Janela (`window_s`):** deslizante, soma dos `use` com `ts` >= agora - `window_s`.

## 6. Autorizacao e protocolo
- `grants.json`/perfil e token como no `memd` (token `gt_` + 43 base64url; so o SHA-256 no disco; por requisicao; `SIGHUP` recarrega; nenhuma API altera perfis). Perfil: `{agent, token_sha256, role (owner|agent), can_request, can_delegate, expires}`. **Quem concede, nega, revoga e aprova e so o `owner`.**
- **Leitura:** `list` (agente: so concessoes onde ele e `grantee`/`*`, e os pedidos dele; owner: todas), `show ID` (idem; fora do alcance = `not_found`, identico a inexistente). `status`: `head_seq` e contagens por status, sem conteudo.
- Protocolo: socket `grantd.sock` (0700/0600), JSON por linha, envelope igual ao do `memd` (secao 6 dele). Respostas <= 65536 bytes. Erros -> saida da CLI: `unauthorized`(3), `forbidden`(3), `not_found`(4), `request_conflict`(4), `exceeds_parent`(4), `denied`(3 em `check`/`use`), `invalid_params`(2), `invalid_action`(2), `missing_source`(2), `missing_until`(2), `quota`(5), `overloaded`(5), `broken`(5); sucesso e `allow` = 0.
- **Limite honesto:** agentes do mesmo usuario Unix que leem o token de outro se passam por ele, e qualquer processo do mesmo usuario pode editar `log.jsonl`. O `grantd` v0 detecta incoerencia por `validate`, nao a impede. Isolamento real: contas Unix por agente ou sandbox. A v1 (assinatura do dono; cadeia de hash) fecha "concessao falsa".

## 6A. Campos por metodo e decisoes de contrato (normativo; fecha as lacunas achadas pelos testes independentes)
- **Envelope e erros:** como o `memd` (secao 6 dele). Metodo desconhecido = `invalid_params`. Codigos adicionais: `secret_detected` (2; varredura de segredos do `memd` aplicada a `reason` e `source.ref`). Pedido de `check`/`explain` de um agente sobre **outro** agente: `forbidden` para `role: agent`; o `owner` pode consultar qualquer um.
- **Parametros** (`request_id` obrigatorio nas mutacoes): `check|use|explain`: `agent`, `action`, `resource`, `qty?`, `unit?` (`use` tambem `request_id`; `use` sem `qty` em concessao com `max_amount` = `invalid_params`). `grant|deny_rule`: `effect` (`deny_rule` fixa `deny`), `grantee`, `action`, `resource`, `until` ou `permanent:true`, `not_before?`, `constraints?`, `delegable?`, `source`, `request_id`. `revoke`: `id`, `reason`. `delegate`: `parent`, `grantee`, `action`, `resource`, `until?`, `constraints?`, `delegable?`, `source?`. `request`: `action`, `resource`, `until`, `reason?`. `approve_request`: `req`, `until?`. `reject_request`: `req`, `reason`. `show`: `id`. `list`, `whoami`, `status`: `{}`.
- **Resultados:** `grant|deny_rule|delegate`: `{id, seq}`; `request`: `{id, status:"pending", seq}`; `approve_request` -> `{id (a concessao), req, seq}`; `reject_request` e `revoke` -> `{id, seq}`. `check`/`explain`: `{decision, grant, reason, until, signed:false}` (`denied_by` com o id quando `reason` = `denied_by`); `explain` acrescenta `matched`: lista de `{id, effect, active, why}`. `use`: `{...check, replayed}`; negado = erro `denied`. `list`: `{grants:[...], requests:[...]}` (cada item com `id, effect, grantee, action, resource, until, permanent, status, parent`); `show`: um item. `status`: `{head_seq, counts, broken, uptime_s}`. `check` negado **e resultado** (`ok:true`, `decision:"deny"`); o erro `denied` existe so em `use`.
- **Codigos nos casos antes omissos:** filho de pai `permanent` sem `until` = `missing_until`; `until` aprovado maior que o pedido = `invalid_params`; `delegable:true` no filho sem profundidade restante = `invalid_params`.
- **Profundidade (corrige G24):** a concessao do dono tem profundidade 0; o filho 1; o neto 2 (`max_depth`). Delegar a partir de uma concessao que **nao e `delegable`** (inclui toda de profundidade 2) = `exceeds_parent`; o "profundidade 3" so e alcancavel por essa regra.
- **Fronteiras:** `not_before` inclusivo, `until` exclusivo; janela de `max_amount` com `ts >= agora - window_s`.
- **`grants.json`:** lista na raiz; `expires` = data `YYYY-MM-DD` UTC (vale ate o fim desse dia). **CLI:** o `check` exige token (`--token-file`) como qualquer metodo; sem autenticacao nao ha oraculo; `token-hash` le o token da entrada padrao; o log de diagnostico e o `stderr`; `validate` com inconsistencia sai com 4.

## 7. CLI (`--json` em todos)
`grantd serve --state-dir D --config C --grants G [--socket S]`; `grantd init DIR` (estado 0700, modelo sem tokens, `LEIA-ME.md` generico; idempotente); `grantd check|use|explain --agent A --action X --resource R [--qty N --unit U --request-id R]`; `grantd grant|deny-rule|revoke|approve-request|reject-request ...` (owner); `grantd request|delegate|list|show|whoami|status --token-file F [--params JSON|-]` (**mesma regra do `memd` para `--params`**; `check|use|explain` mantem as flags acima, e `grant|deny-rule|revoke|approve-request|reject-request` tambem aceitam `--params`); `grantd validate --state-dir D` (offline: log, `delegate` dentro do pai, nenhuma `active` sem `until` nem `permanent`, cascata consistente); `grantd token-hash`. Modo texto de `check`: `allow G-014 ate 2026-10-17T03:00:00Z` ou `deny no_grant`.

## 7A. API de biblioteca (normativa; mesmo molde do `memd`, secao 8A dele)
O crate `grantd` e **biblioteca + binario**; as regras vivem em `Service`, testadas com relogio e disco injetados (`blocklog`); socket, CLI, `SIGHUP`, `init`, `token-hash`, `validate` e concorrencia sao testados pelo binario.
```rust
pub struct Config { /* max_pending_requests (5), line_max, response_max, max_connections */ }
impl Config { pub fn from_json(s: &str) -> Result<Config, String>; }  impl Default for Config { /* padroes */ }
pub struct Grants { /* perfis da secao 6 */ }
impl Grants { pub fn from_json(s: &str) -> Result<Grants, String>; }
pub fn token_sha256_hex(token: &str) -> String;

pub struct Service { /* privado */ }
impl Service {
    pub fn new(cfg: Config, grants: Grants, log: blocklog::Log, replay: Vec<blocklog::Commit>,
               clock: Box<dyn blocklog::Clock>) -> Result<Service, String>;
    pub fn handle_line(&mut self, line: &str) -> String;   // uma requisicao -> uma resposta, sem '\n'
    pub fn reload_grants(&mut self, grants: Grants);
    pub fn head_seq(&self) -> u64;
    pub fn is_broken(&self) -> bool;
}
```
`handle_line` nunca entra em panico com entrada arbitraria. `check`, `explain`, `list`, `show`, `whoami` e `status` nao gravam; as demais mutacoes passam por `blocklog::Log::commit`.

## 8. Seguranca
Sem rede. Token e `reason`/`resource` com segredo nunca em log de diagnostico (so ids e codigos). A varredura de segredos do `memd` se aplica a `reason` e `ref` (mesma lista). `secret`-como-acao (`read_secret`) nao existe: o que o dono nunca quer fazer vira `deny_rule` permanente em `fs.read` sobre os caminhos de credencial, **como conteudo**, nao mecanismo.

## 9. Pontos de integracao (sem eles, o `grantd` nao protege)
- **Scripts nossos** (`enfileira.sh`, `instala.sh`, wrappers de git): chamam `grantd check`/`use` antes de gastar, instalar ou enviar. A lista `ALLOWED` do escriba migra para concessoes `spend`.
- **Claude Code:** hook `PreToolUse` (existe; recebe `tool_name` e `tool_input`, medido em 2026-10-16) via cliente futuro (v0.2) que mapeia **ferramentas de arquivo** (`Write`, `Edit`) para `fs.write`. **Limite:** mapear comandos de shell (`Bash`) por padrao de texto e contornavel; nao prometemos isso.
- **Codex:** a politica de aprovacao dele continua sendo o freio; o `grantd` entra por wrappers.
- **Politica do chamador:** `check` indisponivel (saida 5) = **negar** para `risk` alto; para baixo o chamador decide e registra. Na v1 o chamador pode exigir `signed:true`.
- **Nada e automatico na v0:** cada ponto de integracao e uma fatia propria.

## 10. Contrato testavel (testes independentes; regras por `Service` (secao 7A) com `blocklog::Storage`/`Clock` falsos; socket, CLI, `SIGHUP`, `init`, `token-hash`, `validate` e concorrencia pelo binario; sem rede)
Formato: **pre-condicao -> acao -> oraculo exato.** O motor esta em `blocklog-spec-v0.md` (B1-B28).
- **Decisao:** G1 sem concessao -> `check` = `deny no_grant`; G2 `allow` casando -> `allow` com o id; G3 `deny` casando + `allow` casando -> `deny denied_by`; G4 `until` no passado -> `deny no_grant`; G5 `not_before` futuro -> `deny no_grant`; G6 glob `*` casa um segmento, `**` varios (tabela de 12 caminhos); G7 `resource` com `..`, `//`, relativo ou glob em `check` = `invalid_params`; G8 `grantee:*` vale para qualquer agente; G9 `invalid_action` nao e `deny`; G10 relogio regressivo nao reativa concessao expirada nem reabre janela; G11 `explain` lista as que casaram.
- **Constraints:** G12 `max_uses` = 2: `use` 1 e 2 `allow`, terceiro `deny exhausted`; `check` nao consome; G13 `max_amount` 10 `unit usd` com `window_s` 60: `use` qty 6 e 4 allow, `qty` 1 deny, apos 61 s (relogio injetado) volta a permitir; G14 `use` com o mesmo `request_id` consome uma vez (`replayed:true`) e carga diferente = `request_conflict`.
- **Poder:** G15 `grant`/`deny_rule`/`revoke`/`approve_request` por agente = `forbidden`; G16 sem `until` e sem `permanent` = `missing_until`; G17 `permanent` so owner e `list` o destaca; G18 `request` cria `pending` e nao concede (`check` nega); G19 `approve_request` gera a concessao no mesmo commit e `until` aprovado nunca excede o pedido; G20 `request` acima de 5 pendentes = `quota`; G21 agente nao ve concessao alheia: `show`/`list` identicos a inexistente.
- **Delegacao:** G22 `delegate` acima do pai (acao diferente, recurso fora, `until` maior, `constraints` maiores) = `exceeds_parent`; G23 pai `permanent` exige `until` finito no filho; G24 profundidade 3 = `exceeds_parent`; G25 quarto filho = `quota`; G26 `revoke` do pai revoga filhos e netos no **mesmo** commit (cauda cortada: nenhum ou todos); G27 `delegable` do filho respeita a profundidade restante.
- **Durabilidade/seguranca:** G28 estado reconstruido do log inteiro igual ao vivo (150 mutacoes pseudoaleatorias com semente fixa, reinicio, `list` completo igual); G29 token errado/expirado/removido apos `SIGHUP` = `unauthorized` mesmo em conexao aberta; G30 nenhuma API altera perfis; G31 token/`reason` em sentinela nunca aparecem em log de diagnostico, `status`, `whoami` ou erro; G32 `validate` pega `active` sem `until`, delegacao fora do pai e cascata inconsistente; G33 `broken` (falha injetada): mutacoes `broken`, `check` continua; G34 `check` indisponivel (daemon fora) -> cliente sai 5.

## 11. Fases e aceite
- **v0** (esta spec) -> **v0.2:** `watch`, `export`, `grantd hook claude`, instantaneos -> **v1:** concessoes do dono assinadas (`ssh-keygen -Y`; verificacao na entrada, `signed_by` gravado, cadeia de hash do log) -> **v2:** fora de maquina e cifra com construcao revisada.
- **Piloto:** migrar 10 autorizacoes reais de `autorizacoes-jabbur.md` (ex.: executor da v1.7; commits e push dos `blocks`; Haiku 5.5 e `gpt-6-luna` dispensados de ok por uso; `push` negado a todos os agentes) e ligar **um** ponto de integracao (`enfileira.sh` perguntando `spend`). Metas: nenhuma divergencia entre a tabela e as respostas de `check`; zero concessao sem validade; um agente novo (Haiku sem contexto) consulta e pede uma concessao sem ser ensinado.

## 12. Em aberto
1. Chave de assinatura do dono (v1). 2. Contas Unix por agente. 3. Como as regras 1 a 8 do `CLAUDE.md` viram `deny_rule` permanentes (conteudo). 4. Se o `memd` deve consultar o `grantd` para `promote` (hoje, perfil proprio). 5. Frequencia e custo de `check` em hooks por chamada de ferramenta (latencia do socket) a medir no piloto.
