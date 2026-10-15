# agentd v1: estado dos agentes (contrato testavel) | RASCUNHO rev2 (revisao do codex-blocks incorporada, 2026-10-15); falta revisao Opus + Sol 6.1

Origem (decisoes do Jabbur, 2026-10-15): "creio que devemos pensar em um outro daemon que faca o que o herdr e luvus fazem para ler o estado dos agentes"; **daemon** (nao CLI), codigo em `crates/agentd` do `blocks`, implementacao pelo `codex-blocks`; spec acordada entre o Sextante e o `codex-blocks` e revisada por Opus e Sol 6.1; filosofia unix (uma tarefa, uma ferramenta). Estudo-base: `~/Projects/agentd/docs/estudo-2026-10-15.md`; convergencia: `~/Projects/agentd/docs/convergencia-claude-codex-2026-10-15.md`. Os testes falam com o **binario** e com o **protocolo no socket**, nunca com a API interna.

## 1. Problema, exemplo, solucao
Hoje o `chatd` pergunta ao herdr "o agente esta ocioso?" antes de tocar a campainha, e o herdr decide **raspando a tela**; quando nenhum padrao casa (por exemplo, um dialogo de permissao) ele cai em `idle`. Exemplo: o Codex aguarda uma aprovacao, o herdr diz `idle`, a campainha digita "[chat] 1 pendente(s)..." em cima do dialogo. Alem disso, qualquer mudanca no estado de um agente passa despercebida (a politica de rede do sandbox do `codex-blocks` mudou as 14:26 e ninguem viu).
Solucao: um daemon pequeno que **so observa e publica** o estado dos agentes, com a **evidencia** e a **frescura** de cada informacao, e que na duvida diz `unknown`, nunca `idle`.

## 2. Escopo
**Faz:** manter em memoria o estado de cada agente configurado; ler fontes somente leitura; publicar por `agent.get`, `agent.list`, `events.resume`, `status`; CLI fina.
**Nao faz (e e teste):** entregar mensagem, digitar em terminal, autorizar, ler prompt/resposta/ambiente/token, ler o conteudo da tela, falar com o multiplexador alem de consultar estado, persistir estado (reinicio = nova `epoch`), falar com outras maquinas, hospedar PTY, tokens e custo.

## 3. Modelo
- **Agente:** `id` `[a-z0-9_-]{1,32}` (o mesmo nome do `chatd`) e `kind` `claude | codex`. Os agentes sao **declarados na configuracao**; o daemon nao os cria nem os descobre sozinho.
- **Estado:** `unknown | idle | working | blocked | gone`. Nao existe `done` (e `idle`).
- **Marcas** (lista, pode ser vazia): `stale` (a informacao esta velha), `error` (a fonte falhou na ultima leitura), `idle_unconfirmed` (houve leitura `idle` ainda nao confirmada pela regra 6; o estado publicado e o anterior), `background` (reservada; so fontes futuras).
- **Evidencia:** `declared` (o proprio agente declarou: hook ou API; fontes futuras), `observed` (inferida por quem observa: herdr), `none`.
- **Campos de `Agente`:** `{v, id, kind, state, marks, evidence, source, last_seen, observed_at, freshness_ms, last_known, cwd, branch, provider, model, task_id, state_seq, source_seq, source_completion_seq}`; campo sem valor vem `null`. `source` e o tipo da fonte que produziu o estado (`herdr`). `last_known` so quando `state` foi rebaixado a `unknown` por `stale` (o estado anterior). `cwd` vem da fonte; `branch`, `provider`, `model` e `task_id` so quando a fonte os declara (herdr nao: `null`). `state_seq` e o `seq` do ultimo `state_changed` desse agente. `source_seq` e `source_completion_seq` sao contadores **opacos** da fonte (herdr: `state_change_seq`, `completion_seq`), `null` quando a fonte nao os tem; so servem para comparar igualdade e ordem, nunca para contar passos.

## 4. Regras de estado (invariantes testaveis)
0. **Precedencia (da mais forte para a mais fraca), avaliada a cada ciclo por agente, com relogio monotonico:** (a) **leitura falhou** (processo nao executou, tempo esgotado, saida invalida ou maior que o limite, valor fora do mapeamento, `kind` divergente) => `unknown` com `error`, **nunca `gone`**, `last_known` guarda o estado anterior; (b) **leitura bem-sucedida e o painel ausente da lista** => ausencia (`agent list` e completo; lista vazia por erro do herdr e o caso (a)): `unknown` com `error` ate `now - last_seen >= gone_after_s`, entao `gone`; (c) **leitura bem-sucedida, painel presente, mas `now - observed_at > stale_after_ms`** (ciclo atrasado) => `unknown` com `stale`; (d) senao, o mapeamento do estado. Limites: `stale` e **estrito** (`>`), `gone` e **inclusivo** (`>=`).
1. **Na duvida, `unknown`.** Agente sem fonte, fonte com `error`, valor de fonte fora do mapeamento ou informacao mais velha que `stale_after_ms` => `unknown`. **`idle` nunca e deduzido de ausencia de informacao.**
2. **`stale`:** se `now - observed_at > stale_after_ms`, o estado vira `unknown`, `marks` ganha `stale` e `last_known` guarda o anterior. A proxima leitura valida limpa a marca.
3. **`gone`** so quando a fonte afirma que o painel/processo nao existe **e** `now - last_seen >= gone_after_s`; antes disso `unknown` com `error`. `last_seen` e o instante da ultima leitura que mostrou o agente vivo.
4. **Varias fontes (v1 tem uma; regra para as futuras):** a prioridade **so resolve conflito entre evidencias de postos diferentes**: um `declared` fresco vence um `observed` fresco; um `declared` velho (`stale`) nao vence nada. Duas fontes **do mesmo posto**, frescas e em desacordo, nao se fundem nem se desempatam por `observed_at`: o estado e `unknown` com `error` (conflito) ate uma voltar a concordar. Sem fonte futura na v1, nao ha teste executavel; fica como contrato.
5. **Monotonia do `seq` e o que gera evento:** cada mudanca de `state`, `marks`, `evidence` ou `source` de um agente gera um evento `state_changed` com `seq` inteiro crescente sem lacunas dentro de uma `epoch`. **Nao geram evento:** `observed_at`, `freshness_ms`, `last_seen`, `source_seq`, `source_completion_seq`, `cwd` e demais campos informativos (quem precisa deles faz `agent.get`). O evento traz `previous_state` e `previous_marks`.
6. **Confirmacao de `idle` observado (anti-oscilacao):** um `idle` com `evidence: observed` so e publicado depois de **duas condicoes juntas**: `idle_confirm_polls` leituras consecutivas `idle` (padrao 2) **e** `idle_confirm_ms` de quietude desde a primeira delas (padrao 2500, por causa do Luvus, que exige 2,5 s). Enquanto nao confirmado, o estado publicado e **o anterior** com a marca `idle_unconfirmed`. Entrar em `working` ou `blocked` e imediato e cancela a espera. Motivo: herdr e Luvus relatam `idle` por padrao quando nenhuma regra casa e passam por `working -> idle -> working` em transicoes curtas (herdr confirma 3 vezes ou 700 ms; Luvus exige 2,5 s quieto). Um `idle` falso e o caso perigoso (campainha sobre um dialogo). Com `poll_ms` 200 e `idle_confirm_polls` 2 so a quietude de 2500 ms garante a duracao.
7. **Relatos declarados (CONTRATO FUTURO: nao exercitavel na v1, que nao tem metodo de escrita; entra com a primeira fonte `declared`):** um relato `declared` traz `sequence` crescente e `ttl_s`; sequencia que nao cresce e ignorada (`stale_report`); passado o prazo o relato **expira para `unknown`** em vez de ficar valendo; uma fonte `declared` por agente (conflito entre duas e `authority_conflict`, estado `unknown`). (Modelo: `agent.report {source, status, sequence, ttl_s}` do Luvus, `src/app/dispatch/agents.rs:407-520`.) **A definir antes da primeira fonte `declared`:** escopo da `sequence` (por agente e fonte), limites do `ttl_s` (5 a 3600), inicio da contagem (recepcao, relogio monotonico), canal e identidade da fonte (mesmo uid nao e isolamento entre agentes) e invalidacao do relato (sequencia nova, mudanca de sessao).

## 5. Protocolo (mesmas convencoes do `chatd`)
JSON-RPC 2.0, **uma requisicao por linha** UTF-8, socket Unix, diretorio 0700 e socket 0600, `"v": 1` em todo `params`, erros com codigo **string**, limite de 1 MiB por linha, lote nao suportado, **campo desconhecido em `params` => `invalid_envelope`** com `data.path`. Autenticacao v1: **so o mesmo uid** (`SO_PEERCRED`), sem token (todos os metodos sao somente leitura e nao ha segredo); outro uid => `forbidden`.
- **`agent.get`** `{v, id}` -> `{v, agent: Agente, epoch, head_seq}`; id inexistente => `unknown_agent`.
- **`agent.list`** `{v, kind?}` -> `{v, agents: [Agente...], epoch, head_seq}` ordenado por `id`.
- **`events.resume`** `{v, epoch, last_seq}` -> `{v, epoch, head_seq}` e depois, **na mesma conexao**, notificacoes `{"jsonrpc":"2.0","method":"event","params":Evento}` com todo evento `seq > last_seq` em ordem e os ao vivo. `Evento = {v, seq, epoch, ts, type:"state_changed", id, state, marks, evidence, source, previous_state}`. **Anel de 4.096 eventos**: seja `oldest` o `seq` do evento mais antigo retido; `last_seq + 1 >= oldest` retoma (inclusive `last_seq == oldest - 1`, o evento seguinte ao ultimo visto ainda esta no anel); `last_seq + 1 < oldest` => erro `resume_gap` com `data.oldest_seq` (o cliente refaz `agent.list`); com anel vazio so `last_seq == head_seq` retoma. A captura de `head_seq`, o replay e a inscricao nos eventos ao vivo ocorrem sob um unico lock: nenhum evento e perdido nem duplicado entre eles; `epoch` diferente da atual => erro `epoch_mismatch`; `last_seq` > `head_seq` => `invalid_envelope`. Uma conexao so chama `events.resume` uma vez (a segunda e `invalid_request`).
- **`status`** `{v}` -> `{v, epoch, head_seq, uptime_s, agents:{id:{state, marks, source, last_seen}}, sources:{tipo:{ok, last_ok_at, last_error}}}`.
- **Nao existe `agent.bind` nem metodo de escrita na v1.** A associacao agente-fonte (painel, pasta) e **configuracao**: um cliente local nao pode redirigir o estado de um agente.
- `epoch`: inteiro aleatorio de 64 bits gerado a cada partida; muda se o daemon reinicia (o anel nao e persistido).
- Erros: `parse_error`, `invalid_request`, `method_not_found`, `unsupported_version`, `invalid_envelope`, `unknown_agent`, `epoch_mismatch`, `resume_gap`, `forbidden`, `too_many_connections` (teto 64), `internal_error`. Erro de uma requisicao nunca derruba o daemon.

## 6. Fonte `herdr` (a unica da v1)
- Consulta **somente** `herdr agent list` (uma chamada por ciclo, sem `--wait`, no maximo um processo em voo), por `Command` com `argv`, **sem shell**, no binario `herdr_bin` da configuracao, com tempo maximo `source_timeout_ms` (padrao 3000; ao estourar, o processo e morto e a leitura e erro). **Lista fechada: apenas `agent list`**; `agent get` **nao e usado** na v1 (um unico snapshot por ciclo e consistente; decisao apos a revisao do `codex-blocks`). Qualquer outro subcomando (`get`, `read`, `send-keys`, `prompt`, `rename`, `focus`, `wait`, `attach`) e proibido e coberto por teste com um herdr falso que registra as chamadas.
- Ciclo a cada `poll_ms`; saida limitada a 1 MiB; so os campos `pane_id`, `agent`, `agent_status`, `cwd`, `state_change_seq`, `completion_seq`, `revision`, `terminal_id` sao lidos; **`terminal_title*` e qualquer texto de tela sao ignorados e nunca guardados**.
- A associacao e por `pane_id` (configuracao). Se o `agent` do painel nao for o `kind` esperado => `unknown` com `error` (nunca se atribui o estado de outro agente).
- **Mapeamento de `agent_status`:** `working` -> `working`; `blocked` -> `blocked`; `idle` -> `idle`; `done` -> `idle`; `unknown` ou qualquer outro valor -> `unknown`+`error`. Toda saida do herdr tem `evidence: observed`.
- **Semantica confirmada no codigo (herdr 0.9.5):** `done` e `idle` **ainda nao visto pela UI** (`(Idle, seen=false) -> Done`, `src/app/api_helpers.rs:100-102`), entao mapear `done -> idle` e correto e a diferenca nao importa para o `agentd`. `state_change_seq` e **um contador global** do herdr, incrementado a cada mudanca de estado de **qualquer** painel (`src/app/actions.rs:1753-1758`); logo, **saltos sao normais** e o `agentd` nao pode assumir passo 1. `completion_seq` so e atribuido numa transicao `working/blocked -> idle` nao suprimida (`actions.rs:1759-1761`). Se `source_seq` diminuir, o herdr reiniciou: o `agentd` recalcula o estado da leitura atual, sem tratar como erro.
- **Limite honesto (nao e defeito do `agentd`):** o herdr (e o Luvus, `src/detect.rs:1301-1305`) devolve `idle` quando um agente conhecido nao casa nenhuma regra, por exemplo, um dialogo de permissao novo (`src/detect/manifest.rs:550-553`). `observed idle` **nao e prova de ociosidade**; so uma fonte `declared` e. Quem precisa de seguranca (a campainha do `chatd`) deve exigir `declared` quando existir, e hoje fica com `observed` + a regra 6.
- **Fora do alcance de qualquer fonte que nao seja a tela:** saber se a caixa de entrada tem rascunho humano. Fica fora da v1.
- Falha ao executar, saida invalida ou painel ausente => `error` e `unknown` (e `gone` so pela regra 3).

## 7. Configuracao (JSON, campo desconhecido recusado)
```json
{
  "herdr_bin": "herdr",
  "poll_ms": 2000,
  "stale_after_ms": 30000,
  "gone_after_s": 120,
  "idle_confirm_polls": 2,
  "idle_confirm_ms": 2500,
  "source_timeout_ms": 3000,
  "ring": 4096,
  "agents": {
    "claude": {"kind": "claude", "sources": [{"type": "herdr", "pane": "w1:p1"}]},
    "codex-blocks": {"kind": "codex", "sources": [{"type": "herdr", "pane": "w3:p1"}]}
  }
}
```
Limites: `poll_ms` 200 a 60000; `stale_after_ms` >= 2 x `poll_ms`; `gone_after_s` 5 a 86400; `idle_confirm_polls` 1 a 5; `idle_confirm_ms` 0 a 30000; `source_timeout_ms` 200 a 30000; `ring` 64 a 65536; ate 32 agentes; `id` repetido ou `pane` repetido para dois agentes recusados. Valor invalido ou campo desconhecido => nao sobe (saida 1).

## 8. CLI (`agentd`)
`agentd serve --config F [--socket P] [--state-dir D]` | `agentd get ID` | `agentd list [--kind K]` | `agentd watch [--since SEQ] [--epoch E]` | `agentd health` | `agentd status`.
- Socket: `--socket`, depois `AGENTD_SOCKET`, depois `<state-dir>/agentd.sock`; **sem diretorio de estado padrao** (como o `chatd`). `--json` em todos os comandos de leitura: `{"schema_version":1,"ok":bool,"code":str,"message":str,"data":...,"next_cursor":...}`.
- Sem `--json`, uma linha por agente: `ID ESTADO[,MARCAS] EVIDENCIA FONTE idade=Ns`. Todo campo vindo de fonte e escapado contra caracteres de controle.
- Saidas: 0 ok; 1 falha/erro do servidor; 2 uso incorreto; 3 agente inexistente; 4 daemon indisponivel. `agentd health` sai 0 so se o daemon responde e **todas** as fontes configuradas estao `ok`.
- `watch` imprime uma linha por `state_changed`, reconecta com backoff e, se a `epoch` mudar, refaz `agent.list` e avisa em stderr.

## 9. Seguranca e privacidade
Nao le credenciais, prompt, resposta, ambiente nem conteudo de tela; nao ha escrita em nenhum agente nem no multiplexador; o log do daemon nao contem texto vindo de fonte alem do estado e dos identificadores; `agentd` nao abre rede. Um agente ou processo do mesmo uid **nao consegue declarar estado** (nao ha metodo de escrita); a unica entrada e a fonte configurada.

## 10. Contrato testavel (testes independentes; o `herdr` e FALSO, um script que devolve JSON controlado e registra as chamadas)
- **G1** mapeamento: `working|blocked|idle|done` e valor desconhecido (`xyz`) -> estados esperados; desconhecido vira `unknown`+`error`, nunca `idle`.
- **G2** `unknown` por padrao: agente configurado sem painel na saida do herdr; herdr que falha; saida invalida; `kind` divergente do painel.
- **G3** `stale`: a fonte para de responder; apos `stale_after_ms` o estado vira `unknown`, com `stale` e `last_known`; volta a `idle` com a leitura seguinte e a marca some.
- **G4** `gone` so apos `gone_after_s` com painel ausente; antes, `unknown`+`error`.
- **G5** eventos: `seq` crescente sem lacunas; `previous_state`; mudanca de `marks` tambem gera evento; sem mudanca, sem evento.
- **G6** `events.resume`: retomada com `last_seq` valido, `resume_gap` (anel pequeno), `epoch_mismatch` apos reinicio, `last_seq` > `head_seq`, segunda chamada na conexao.
- **G7** lista fechada: o herdr falso registra **apenas** `agent list` (nem `agent get`); nenhuma chamada a `read`, `send-keys`, `prompt` etc. durante 10 ciclos.
- **G8** sem shell: `pane` com `;`, `$()`, espacos e aspas nao executa nada (nome de painel e dado, nao comando).
- **G9** protocolo: campo desconhecido, linha de 1 MiB, JSON invalido, lote, metodo inexistente, versao inesperada; o daemon continua vivo.
- **G10** seguranca: permissao 0700/0600; conexao de outro uid recusada (quando o ambiente permite; senao `#[ignore]`).
- **G11** privacidade: saida do herdr falso com `terminal_title` contendo um segredo sentinela; o sentinela nao aparece em `agent.*`, `status`, eventos, CLI nem log.
- **G12** CLI: `get/list/watch/health/status` com e sem `--json`, codigos de saida 0 a 4, escape de caracteres de controle, `health` falha se uma fonte estiver em erro.
- **G13** configuracao: campo desconhecido, `poll_ms` fora do limite, `pane` repetido para dois agentes, mais de 32 agentes.
- **G15** confirmacao de `idle`: uma leitura isolada `idle` entre `working` nao muda o estado publicado e deixa a marca `idle_unconfirmed`; duas consecutivas **com** `idle_confirm_ms` decorrido mudam; `working` e `blocked` sao imediatos e cancelam a espera; com `idle_confirm_polls: 1` e `idle_confirm_ms: 0` o `idle` e imediato; com `poll_ms: 200`, duas leituras em 400 ms **nao** confirmam antes de 2500 ms.
- **G16** contadores opacos: `state_change_seq` com saltos (global) nao gera erro nem lacuna de `seq` do `agentd`; `source_seq` que diminui nao e erro; `source_completion_seq` repassado como veio.
- **G14** reinicio: o daemon sobe de novo e a `epoch` muda; nenhum estado persiste.
- **G17** precedencia (regra 0): erro de leitura + painel ausente => `unknown`+`error`, nunca `gone`; ausencia ate `gone_after_s` => `unknown`; ciclo atrasado com painel presente => `unknown`+`stale`.
- **G18** limites: `stale` so acima de `stale_after_ms` (no limite exato, nao); `gone` no limite exato `gone_after_s` (inclusivo).
- **G19** `resume_gap` na fronteira: `last_seq == oldest - 1` retoma; `oldest - 2` e `resume_gap` com `data.oldest_seq`; anel vazio so aceita `last_seq == head_seq`.
- **G20** corrida: inscricao em `events.resume` enquanto o herdr falso alterna estados em alta frequencia: nenhum `seq` perdido nem repetido entre replay e ao vivo.
- **G21** subprocesso: herdr falso que trava (excede `source_timeout_ms`), imprime mais de 1 MiB ou nada: o daemon continua, a leitura e `error`, a proxima recupera, e nunca ha mais de um processo `herdr` em voo.
- **G22** eventos por campo: mudar so `evidence` ou so `source` gera evento com `previous_marks`; mudar so `observed_at`, `freshness_ms`, `source_seq`, `cwd` nao gera.
- **G23** configuracao: `id` repetido, `pane` repetido, `kind` fora de `claude|codex`, `idle_confirm_ms` fora do limite.

## 11. Revisao do `codex-blocks` (chatd seq 240, 2026-10-15): resolucao por item
| # | Pergunta | Resposta dele | Decisao na spec |
|---|---|---|---|
| 1 | `done -> idle`? | Confirma pela semantica descrita; nao conferiu o codigo do herdr | Mantido; confirmado no codigo pelo Sextante (`api_helpers.rs:100-102`) |
| 2 | `completion_seq`/`state_change_seq` | `state_change_seq` opaco; `completion_seq` so informativo: sem evento proprio, nao e contador de transicoes | **Informativo**: repassado como veio, sem evento (regra 5) |
| 3 | `agent list` ou `agent get` | Prefere um `list` por ciclo; esclarecer a contradicao com G7 | **So `agent list`**; `get` sai da lista fechada (secao 6, G7) |
| 4 | epoch 64 bits e anel 4096 | Suficiente; definir `resume_gap` na fronteira e testar | Definido na secao 5; **G19** |
| 5 | `evidence` | Campo proprio | Mantido |
| 6 | Esqueleto do crate | Reusar convencoes do `protocol.rs`; **sem crate comum agora**; `store`/`server` do `chatd` nao sao compartilhaveis | Aceito (secao 5 ja alinhada) |
| 7 | app-server do Codex | Nao confirma nomes sem fonte externa; adaptador deve normalizar eventos, sintetizar `sequence`/`ttl_s` e decidir se estado reportado e `declared` | Regra 7 marcada **contrato futuro** com os itens a definir |
Ajustes aceitos: precedencia erro/stale/ausencia (regra 0, **G17**); limites inclusivos (**G18**); conflito entre fontes so no mesmo posto (regra 4); eventos por campo (regra 5, **G22**); confirmacao de `idle` por tempo e por contagem com marca `idle_unconfirmed` (regra 6, **G15**); timeout e saida excessiva (secao 6, **G21**); corrida replay/ao vivo (secao 5, **G20**); configuracao (**G23**). Ressalva dele: relatos `declared` nao sao exercitaveis na v1.

## 12. Fora desta spec (v1.1 em diante)
Fonte app-server do Codex; fontes de hook do Claude e deteccao de interrupcao pela transcricao; modo sombra contra o herdr; integracao com o `chatd` (assinar `events.resume` em vez de `herdr agent get`; `bell_adapter`); suporte a tmux como fonte; persistencia do anel; Pi como terceiro agente.
