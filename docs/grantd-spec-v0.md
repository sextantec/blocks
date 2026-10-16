# grantd v0: autorizacoes legiveis por maquina, com validade e revogacao (contrato) | RASCUNHO v0 (2026-10-16)

**Estado: RASCUNHO para revisao. Nao ha codigo.** Origem: roadmap dos `blocks` (item 3: "machine-readable authorisations: may this agent do this, until when"); o Jabbur, 2026-10-16: "Estou convencido de que o memd e grantd ajudam muito no processo" e "vamos fazer o memd e o grantd, specs e depois codigo". Irmao do `memd` (`docs/memd-spec-v0.md`): **mesmo motor de gravacao** (secao 5 do `memd`, A1 a A8), outro dominio.

## 0. O que e, e o que nao e
- **E:** um daemon pequeno, dono de um registro de **concessoes** (grants): "o agente A pode fazer a acao X sobre o recurso R, ate a data D, por esta razao, concedido por Q". Responde `check` com **negacao por padrao**, conta usos e permite delegar e revogar.
- **Nao e:** um sandbox (nao impede um processo de fazer o que o sistema operacional deixa), nem o `memd` (que guarda fatos; aqui so poder), nem o `ledger` (que sabe saldos e precos; o `grantd` so conta unidades que o chamador declara), nem um executor. **Um verificador que ninguem consulta nao protege nada** (secao 9): a garantia vale so onde ha um ponto de integracao que pergunta antes de agir.

## 1. Problema, exemplo, solucao
**Problema.** Hoje as autorizacoes do dono vivem em prosa: `docs/autorizacoes-jabbur.md` (A1..A21), as regras 1 a 8 do `CLAUDE.md` e a lista `ALLOWED` do escriba (modelos que a fila aceita, editada a mao). Os agentes as seguem por disciplina; um agente pode alegar "o Jabbur autorizou" sem prova, e outro pode ignorar uma autorizacao que nao leu.
**Exemplo.** Em 2026-10-16 o codex-blocks bloqueou a rev6 porque a autorizacao nao estava escrita onde ele olha. Com o `grantd`: o Jabbur concede `G-014` (grantee `codex-blocks`, acao `fs.write`, recurso `~/Projects/blocks/crates/agentd/src/**`, `until` 2026-10-17, fonte "Siga em frente", `task_ref` `P-88`). O agente (ou o hook dele) pergunta `check` e recebe `allow` com `G-014`, ou `deny` com a razao. Quando o prazo vence ou o Jabbur revoga, o proximo `check` ja nega.
**Solucao.** Log de eventos de acrescimo, dono unico, token por agente, concessoes com validade obrigatoria, decisao deterministica (negar vence), delegacao com escopo menor e cascata de revogacao, e assinatura do dono na fase v1.

## 2. Principios
1. **Negar por padrao.** Sem concessao `allow` aplicavel, `check` nega. `deny` explicito vence qualquer `allow`.
2. **So o dono concede poder novo.** Agentes nunca criam concessao sozinhos; podem **pedir** (`request`) e **delegar um subconjunto** do que ja tem (se `delegable`).
3. **Validade obrigatoria.** Toda concessao tem `until`, salvo `permanent:true`, que so o dono cria e que aparece destacada em `list`. Sem esquecimento silencioso.
4. **Decisao pura e explicavel.** O resultado depende so do log e do relogio; `explain` mostra quais concessoes casaram e por que a decisao saiu assim.
5. **Revogar vale na hora** (autoridade e o daemon, nada fica em cache de agente alem de `ttl` curto declarado).
6. **Fail-closed para o que e arriscado** (secao 9).

## 3. Modelo
**Concessao** (`Grant`), JSON, campo desconhecido recusado:
- `id` (`G-nnn`, sequencial), `effect` (`allow|deny`), `grantee` (id de agente ou `*`), `action` (secao 4), `resource` (padrao, secao 4), `constraints` (opcional: `max_uses`, `max_amount`+`unit`, `window_s`), `not_before`, `until`, `permanent` (bool, padrao falso), `delegable` (bool, padrao falso), `risk` (derivado da acao, nao informado).
- **Proveniencia:** `issuer` (quem concedeu: `owner` ou o `id` do agente que delegou), `source` (obrigatoria: `{kind:"quote|doc|msg|commit", ref, at}`), `task_ref` (opcional, id de pendencia do `memd`), `parent` (id da concessao de que foi delegada), `signed_by` (v1).
- **`status` derivado** (nao gravado): `active` (agora entre `not_before` e `until`, nao revogada), `expired`, `revoked`, `pending` (so `request`), `denied_request`.

## 4. Acoes e recursos (lista fechada na v0)
| `action` | `resource` casa | risco |
| --- | --- | --- |
| `fs.read`, `fs.write` | caminho absoluto, glob `*`/`**` | `fs.write` fora do workspace = alto |
| `exec` | `argv[0]` do binario (caminho absoluto) | alto |
| `net` | `host` ou `host:porta` | alto |
| `spend` | rota/modelo (`model:gpt-6-astra`), unidade em `constraints` | alto |
| `git.commit`, `git.push` | caminho do repositorio | `push` = alto |
| `install`, `service.restart` | nome do servico/binario | alto |
| `delegate` | id do agente alvo | alto |
| `memd.promote` (atalho para o `memd` consultar) | escopo | medio |
Acao fora da lista = `check` devolve `invalid_action` (nao `deny`: erro de uso). Acao nova = revisao da spec. Casamento de `resource`: caminho normalizado (sem `..`, sem symlink resolvido pelo daemon; **o chamador normaliza**), glob so com `*` (um segmento) e `**` (varios); sem regex.

## 5. Avaliacao (deterministica)
`check(agent, action, resource, qty?)`: (1) reune concessoes `active` com `grantee` igual ao agente ou `*`, mesma `action` e `resource` casando; (2) se houver qualquer `deny` casando -> `deny` (razao = o id); (3) senao se houver `allow` casando e dentro das `constraints` (usos/valores na janela) -> `allow` com o id da concessao mais restritiva (menor `until`); (4) senao `deny` (`no_grant`). `check` e **somente leitura** (nao grava). `use(...)` = `check` + commit atomico de um evento `use` (idempotente por `request_id`) que consome `max_uses`/`max_amount`; so `use` altera contadores. `explain` devolve todas as concessoes que casaram e a regra aplicada.
**Relogio:** `until` e data absoluta; o daemon usa `max(relogio, maior timestamp ja gravado)` para que relogio andando para tras nao reviva concessao vencida.

## 6. Eventos e durabilidade
Mesmo motor do `memd` (secao 5 do `memd-spec-v0.md`): `log.jsonl` 0600, **uma linha = um commit** (`tx`), cauda cortada descartada, linha invalida no meio recusa a partida, gravar antes de aplicar, `sync_data` por commit, `request_id` idempotente, `flock` exclusivo, instantaneo e segmentos. Eventos: `grant`, `deny_rule`, `revoke` (com cascata aos filhos, **no mesmo `tx`**), `delegate`, `request`, `approve_request`, `reject_request`, `use`. `check` nao gera evento. O motor sera implementado **uma vez** num crate compartilhado e usado pelos dois daemons.

## 7. Identidade, papeis e protocolo
- Token por agente (arquivo 0600; o daemon guarda so o hash), perfil em `config.json` editado so pelo dono: `agent`, `role` (`owner|agent`), `can_request`, `can_delegate`. **So o papel `owner` executa `grant`, `deny_rule`, `revoke` e `approve_request`.** Nenhuma operacao da API altera papeis.
- Metodos (JSON por linha em `grantd.sock`, convencoes do `chatd`): `whoami`, `check`, `use`, `explain`, `list`, `show`, `request`, `delegate`, `grant`, `deny_rule`, `revoke`, `approve_request`, `reject_request`, `watch`, `status`. Erros fechados: `unauthorized`, `forbidden`, `invalid_params`, `invalid_action`, `missing_source`, `missing_until`, `exceeds_parent`, `request_conflict`, `not_found`, `broken`.
- **Delegacao:** o delegado nunca recebe mais do que o pai: acao igual, `resource` contido, `until` <= o do pai, `constraints` <=; `delegable` do filho so se o pai permitir e nunca alem de uma geracao a mais (profundidade maxima `max_depth`, padrao 2). Revogar o pai revoga os filhos.
- **Limite honesto:** agentes do mesmo usuario Unix que leem o token de outro se passam por ele, e qualquer processo do mesmo usuario poderia editar `log.jsonl` diretamente. O `grantd` v0 detecta adulteracao so por `validate` (consistencia), nao a impede. Isolamento real exige contas Unix distintas ou sandbox, e a fase v1 (assinatura do dono; cadeia de hash do log) fecha a parte "concessao falsa".

## 8. CLI (`--json` em todos; codigos 0 permitido, 2 uso, 3 negado, 4 conflito, 5 indisponivel)
`grantd serve --state-dir D --config C`; `grantd init DIR` (estado 0700, `config.json` de modelo sem tokens, pagina `LEIA-ME.md` que ensina o agente; idempotente); `grantd check|use|explain AGENTE ACAO RECURSO [--qty N --unit U --request-id R]`; `grantd grant|deny-rule|revoke ...` (so owner); `grantd request|delegate|list|show|watch|status|whoami`; `grantd validate` (offline); `grantd export DIR` (markdown derivado, somente leitura, por concessao). Saida de `check` em modo texto: uma linha `allow G-014 ate 2026-10-17` ou `deny no_grant`.

## 9. Pontos de integracao (sem eles, o `grantd` nao protege)
- **Scripts nossos** (`enfileira.sh`, `instala.sh`, wrappers de git): chamam `grantd check` antes de gastar, instalar ou enviar. A lista `ALLOWED` do escriba migra para concessoes `spend`.
- **Claude Code:** hook `PreToolUse` (existe; recebe `tool_name` e `tool_input`, medido em 2026-10-16) via um cliente `grantd hook claude` que mapeia **ferramentas de arquivo** (`Write`, `Edit`) para `fs.write` com o caminho. **Limite:** mapear comandos de shell (`Bash`) para acoes por padrao de texto e contornavel; nao prometemos isso. Para shell, valem so concessoes sobre binarios invocados por nossos scripts.
- **Codex:** politica de aprovacao do proprio Codex continua sendo o freio dele; o `grantd` entra por wrappers e pelo `chatd`.
- **Politica do chamador:** `check` indisponivel (exit 5) = **negar** para `risk` alto; para `risk` baixo o chamador pode seguir e registrar. `require_signed` por acao (v1): o chamador pode exigir `signed:true` na resposta.
- **Nao ha integracao automatica na v0:** cada ponto e uma fatia propria.

## 10. Contrato testavel (testes independentes; daemon real em diretorio temporario, sem rede)
- **Decisao:** G1 sem concessao = `deny no_grant`; G2 `allow` casando = `allow` com o id; G3 `deny` vence `allow`; G4 `until` vencido nega; G5 `not_before` futuro nega; G6 glob `*` x `**` e caminho com `..`; G7 `grantee: *`; G8 `invalid_action` nao e `deny`; G9 relogio para tras nao revive concessao vencida; G10 `explain` lista as concessoes que casaram.
- **Constraints:** G11 `max_uses` esgota apenas por `use`; G12 `check` nao consome; G13 `max_amount` na janela; G14 `use` idempotente por `request_id`.
- **Poder:** G15 so `owner` concede, nega e revoga; G16 agente nao cria concessao; G17 `request` cria `pending` e nao concede; G18 `approve_request` so owner; G19 sem `until` e sem `permanent` = `missing_until`; G20 `permanent` so owner e aparece destacado.
- **Delegacao:** G21 delegar acima do pai = `exceeds_parent`; G22 `until` do filho <= pai; G23 profundidade maxima; G24 revogar o pai revoga os filhos no mesmo commit (sem estado parcial com cauda cortada).
- **Durabilidade (iguais as do `memd`):** G25 cauda cortada; G26 linha invalida no meio recusa subir; G27 `flock`; G28 falha de escrita = `broken`; G29 `seq` sem lacunas; G30 instantaneo + resto = log inteiro.
- **Seguranca:** G31 token errado/expirado; G32 nenhuma API altera papeis; G33 token e segredo nunca em log de diagnostico (sentinela); G34 `validate` pega log adulterado/inconsistente.

## 11. Fases
- **v0:** secoes 3 a 10 (daemon, tokens, decisao, uso, delegacao, `init`, `export`). **v1:** concessoes do dono **assinadas** (`ssh-keygen -Y`, como no `memd`), `signed:true` na resposta, cadeia de hash do log. **v2:** integracoes empacotadas (`grantd hook claude`, wrapper de git/escriba), fora de maquina e cifra.
- **Piloto:** migrar 10 autorizacoes reais (ex.: executor da v1.7; commits e push dos `blocks`; Haiku 5.5 e `gpt-6-luna` dispensados de ok por uso; `push` negado a todos os agentes) e ligar **um** ponto de integracao (`enfileira.sh` perguntando `spend`). Metas: nenhuma divergencia entre a tabela de `autorizacoes-jabbur.md` e as respostas de `check`; zero concessao sem validade; um agente novo (Haiku sem contexto) consulta e pede uma concessao sem ser ensinado.

## 12. Em aberto
1. Onde mora a chave de assinatura do dono (v1). 2. Lista de acoes: falta algo (ex.: `read_secret`, que deveria ser sempre negado)? 3. Como as regras 1 a 8 do `CLAUDE.md` viram concessoes `deny` permanentes do dono (conteudo, nao mecanismo). 4. Se `memd.promote` deve consultar o `grantd` ou manter o proprio perfil (hoje, perfil proprio, decisao de acoplamento). 5. Contas Unix por agente.

## 13. Fora desta spec
Sandbox, precos e saldos (`ledger`), execucao, cifra e rede (v2), UI, concessao por papel/grupo alem de `*`.
