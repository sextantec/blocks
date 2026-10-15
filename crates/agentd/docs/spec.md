# agentd v1: observador conservador do estado dos agentes (contrato testavel) | RASCUNHO rev3 (revisoes do codex-blocks, Opus e Sol 6.1 incorporadas, 2026-10-15)

Origem (decisoes do Jabbur, 2026-10-15): "creio que devemos pensar em um outro daemon que faca o que o herdr e luvus fazem para ler o estado dos agentes"; **daemon** (nao CLI), codigo em `crates/agentd` do `blocks`, implementacao pelo `codex-blocks`; filosofia unix (uma tarefa, uma ferramenta). Estudo-base: repositorio privado `agentd` (`docs/estudo-2026-10-15.md`). Os testes falam com o **binario** e com o **protocolo no socket**, nunca com a API interna.

## 0. O que a v1 e, e o que NAO e (leia primeiro)
A v1 e um **observador conservador**: publica o estado que o herdr relata, com a evidencia e a frescura, e na duvida diz `unknown`. **Nao e uma autorizacao segura de digitacao.** Duas revisoes independentes (Opus 5.5 e Sol 6.1) mostraram, e esta spec aceita, que:
1. Um dialogo de permissao que o herdr nao reconhece aparece como `idle` **estavel**; repetir leituras ou esperar 2,5 s nao cria evidencia nova. `idle` com `evidence: observed` significa "o herdr relata ocioso e e estavel", **nunca** "e seguro digitar".
2. Entre a publicacao de `idle` e a escrita de quem consome ha uma corrida que nenhum cache fecha.
**Consequencia contratual:** o `chatd` (ou qualquer consumidor) deve tratar `observed idle` como "permitido com aviso" e **nao** pode deduzir seguranca dele. A integracao com a campainha fica **fora da v1** (secao 12) ate existir fonte `declared` ou mecanismo seguro no destino. O ganho real da v1: elimina a oscilacao `working -> idle -> working`, torna visivel quando o estado nao e confiavel e detecta quando o agente some ou muda.

## 1. Problema, exemplo, solucao
Hoje o `chatd` pergunta ao herdr "o agente esta ocioso?" antes de tocar a campainha, e o herdr decide **raspando a tela**; sem padrao que case ele cai em `idle`. Exemplo: o Codex aguarda uma aprovacao, o herdr diz `idle`, a campainha digita sobre o dialogo. Alem disso, mudancas no estado de um agente (a politica de rede do sandbox do `codex-blocks` mudou as 14:26) passam despercebidas.
Solucao: um daemon pequeno que **so observa e publica**, com evidencia e frescura, e que na duvida diz `unknown`, nunca `idle`.

## 2. Escopo
**Faz:** manter em memoria o estado de cada agente configurado; ler uma fonte somente leitura (herdr); publicar por `agent.get`, `agent.list`, `events.resume`, `status`; CLI fina.
**Nao faz (e e teste):** entregar mensagem, digitar em terminal, autorizar, ler prompt/resposta/ambiente/token, ler o conteudo da tela, falar com o multiplexador alem de `agent list`, persistir estado (reinicio = nova `epoch`), falar com outras maquinas, hospedar PTY, tokens e custo.

## 3. Modelo
- **Agente:** `id` `[a-z0-9_-]{1,32}` (o mesmo nome do `chatd`) e `kind` `claude | codex`, **declarados na configuracao**; o daemon nao os descobre.
- **Estado publicado:** `unknown | idle | working | blocked | gone`. Nao existe `done` (e `idle`).
- **Marcas** (conjunto, pode ser vazio): `error` (a ultima leitura falhou ou o painel esta ausente), `stale` (a informacao esta velha: regra 2), `idle_unconfirmed` (houve leitura `idle` ainda nao confirmada: regra 5). Podem coexistir (`[error, stale]`).
- **Evidencia:** `observed` (relatada pelo herdr) ou `none`. (`declared` fica no Anexo.)
- **`Agente`:** `{v, id, kind, state, marks, evidence, source, last_seen, observed_at, freshness_ms, last_known, cwd}`; campo sem valor vem `null`.
  - `source`: `"herdr"` ou `null`. `cwd`: da fonte, informativo.
  - **Tempo:** `last_seen` e `observed_at` sao **milissegundos Unix (inteiro, hora de parede, so para exibir)**, derivados do relogio monotonico no instante da resposta; todas as decisoes usam o monotonico. `freshness_ms` = idade monotonica de `observed_at` **medida no instante da resposta**, `null` antes da primeira leitura valida. `last_seen`: instante da ultima leitura valida que mostrou o painel presente.
  - `observed_at`: instante em que o **processo `herdr` foi iniciado** (conservador: a idade comeca antes da captura, nao depois).
  - `last_known`: o ultimo estado vindo de leitura valida entre `idle|working|blocked`, mantido enquanto o publicado for `unknown`/`gone` por erro, ausencia ou `stale`; `null` caso contrario; limpo por uma leitura valida confirmada.
- **Leitura valida:** processo saiu 0, stdout dentro do limite e JSON valido no envelope da secao 6, o `pane` configurado aparece **uma unica vez**, o `agent` do painel e o `kind` esperado, `launch_pending` nao e `true` e `agent_status` esta no mapeamento. Qualquer outra e **leitura falha** (a secao 6 lista os codigos).

## 4. Regras de estado (invariantes testaveis; avaliadas a cada ciclo por agente, relogio monotonico)
0. **Na duvida, `unknown`.** Leitura falha, valor fora do mapeamento, agente nunca lido, painel ausente ainda dentro do prazo => `unknown`. **`idle` nunca e deduzido de ausencia de informacao.**
1. **Precedencia do estado:** (a) **leitura falha** => `unknown` com `error` (nunca `gone`); (b) **leitura bem-sucedida e o painel ausente** (a lista e completa por contrato) => `unknown` com `error` ate `now - absent_since >= gone_after_s` (inclusivo), entao `gone`; (c) **painel presente e valido** => mapeamento (`working`, `blocked`, `idle` pela regra 5, `unknown` do herdr => `unknown` **sem** `error`); `launch_pending` ou `screen_detection_skipped` verdadeiros => `unknown` sem `error`. `absent_since` e o instante da **primeira** leitura valida com ausencia desde a ultima presenca (ou desde a partida, para agente nunca visto); uma leitura valida com o painel presente o apaga. Leitura falha **nao** o apaga nem o avanca por si so. `gone` e **reversivel**: leitura valida com o painel presente volta ao mapeamento (por confirmacao se `idle`), com evento.
2. **`stale` e calculado na consulta, nao so no ciclo:** a cada resposta (`agent.get/list/status`), a cada evento e num timer interno de no maximo 1 s, se `now - observed_at > stale_after_ms` (**estrito**) o agente ganha a marca `stale`; se o estado publicado era `idle|working|blocked`, ele vira `unknown` e `last_known` guarda o anterior. Isso vale mesmo com a fonte travada ou a leitura em curso: o `stale` nao depende de o ciclo terminar. A proxima leitura valida limpa `error` e `stale`.
3. **Painel reaproveitado:** o `terminal_id` da ultima leitura valida e lembrado. Se mudar, a janela de confirmacao e zerada, sai um evento e o estado fica `unknown` com `error` ate uma leitura valida do novo terminal (e `idle` volta pela regra 5). Motivo: `w3:p1` pode passar a ser outro Codex depois de o herdr reiniciar.
4. **Eventos:** cada mudanca de `state`, `marks`, `evidence` ou `source` de um agente gera um evento `state_changed` com `seq` inteiro crescente sem lacunas dentro de uma `epoch`, trazendo `previous_state` e `previous_marks`. **Nao geram evento:** `observed_at`, `last_seen`, `freshness_ms`, `cwd`, `last_known`.
5. **Confirmacao de `idle` observado (anti-oscilacao):** **toda** transicao para `idle` publicado, de qualquer estado anterior (inclusive `unknown` da partida, `error`, `stale`, `gone`), exige **tres condicoes juntas** em leituras validas: (i) `idle_confirm_polls` leituras consecutivas `idle` (padrao 2); (ii) o **mesmo `terminal_id` e o mesmo `state_change_seq`** do painel em todas elas (o contador e opaco: so a igualdade e usada); (iii) pelo menos `idle_confirm_ms` entre a primeira e a ultima (padrao 2500). **Qualquer outra leitura** (falha, painel ausente, nao-`idle`, outro `terminal_id` ou `state_change_seq`) **zera** a janela. Enquanto nao confirmado, o estado publicado e o anterior (ou `unknown` se o anterior era `unknown`/`gone`) com a marca `idle_unconfirmed`; uma leitura valida limpa `error` e `stale`. Entrar em `working` ou `blocked` e imediato. Limite honesto: confirma **estabilidade amostrada**, nao quietude comprovada; por isso `poll_ms` tem teto de 10000 e a confirmacao exige igualdade de `state_change_seq`.
6. **Atomicidade:** `agent.get`, `agent.list`, `status` e a captura de `events.resume` (`head_seq` + replay + inscricao) sao feitos sob **um unico ponto linearizavel**: o retrato dos agentes (incluindo `stale` avaliado naquele instante) e o `head_seq` saem do mesmo instante.

## 5. Protocolo (JSON-RPC-like; mesmas convencoes do `chatd`)
Uma requisicao por linha UTF-8, socket Unix, diretorio 0700 e socket 0600, `"v": 1` em todo `params` (objeto), erros com codigo **string** (desvio declarado do JSON-RPC 2.0, que usa inteiros). Todo pedido exige `id`; `params` deve ser objeto; **campo desconhecido em `params` => `invalid_envelope`** com `data.path`. **Limite de 1 MiB por linha contando o LF**; EOF sem LF processa a linha; UTF-8 invalido => `parse_error`. Autenticacao v1: **so o mesmo uid** (`SO_PEERCRED`), sem token; outro uid => `forbidden`.
- **`agent.get`** `{v, id}` -> `{v, agent: Agente, epoch, head_seq}`; `unknown_agent` se inexistente.
- **`agent.list`** `{v, kind?}` -> `{v, agents: [Agente...], epoch, head_seq}` ordenado por `id`.
- **`events.resume`** `{v, epoch, last_seq}` (ambos obrigatorios) -> `{v, epoch, head_seq}` e depois, na mesma conexao, notificacoes `{"jsonrpc":"2.0","method":"event","params":Evento}` com todo evento `seq > last_seq` em ordem e os ao vivo. `Evento = {v, seq, epoch, ts, type:"state_changed", id, state, marks, evidence, source, previous_state, previous_marks}`.
  - **Anel de `ring` eventos** (padrao 4096): seja `oldest` o `seq` do mais antigo retido; `last_seq + 1 >= oldest` retoma (inclusive `last_seq == oldest - 1`); `last_seq + 1 < oldest` => `resume_gap` com `data.oldest_seq`; com anel vazio so `last_seq == head_seq` retoma; `last_seq > head_seq` => `invalid_envelope`; `epoch` diferente => `epoch_mismatch`. Uma conexao so chama `events.resume` uma vez (a segunda e `invalid_request`).
  - **Bootstrap do consumidor:** `agent.list` -> usa **exatamente** o `epoch` e o `head_seq` dele em `events.resume`; em `resume_gap` ou `epoch_mismatch` refaz `agent.list` e abre **nova conexao**.
  - **Assinante lento:** fila de ate 256 eventos por conexao, envio **fora** do lock; ao estourar, a conexao recebe `{"method":"closed","params":{"reason":"slow_consumer","last_seq":N}}` e e fechada; nunca se descarta evento em silencio e o lock nao e mantido durante escrita.
- **`status`** `{v}` -> `{v, epoch, head_seq, uptime_s, agents:{id:{state, marks, source, last_seen}}, sources:{herdr:{ok, last_ok_at, last_error}}}`, onde `last_error` e **um codigo** (secao 6), nunca texto da fonte; `ok` = a **execucao** da ultima leitura (processo, tempo, JSON) deu certo, independente do estado de cada agente.
- **Nao existe `agent.bind` nem metodo de escrita na v1.** A associacao agente-fonte e **configuracao**.
- **`epoch`:** inteiro aleatorio de 64 bits codificado como **string decimal** no JSON (clientes JS perdem precisao acima de 2^53); `seq`/`head_seq`/`last_seq` sao inteiros sem sinal (`last_seq` negativo => `invalid_envelope`); gerada a cada partida.
- Erros: `parse_error`, `invalid_request`, `method_not_found`, `unsupported_version`, `invalid_envelope`, `unknown_agent`, `epoch_mismatch`, `resume_gap`, `forbidden`, `too_many_connections` (teto 64), `internal_error`. Erro de uma requisicao nunca derruba o daemon.

## 6. Fonte `herdr` (a unica da v1)
- **Uma fonte `herdr` por agente**, exatamente (`sources` com um elemento). Consulta **somente `herdr agent list`** (um snapshot por ciclo, sem `--wait`), por `Command` com `argv`, **sem shell**, no binario `herdr_bin`. Lista fechada: apenas `agent list`; `agent get` **nao e usado**; qualquer outro subcomando (`get`, `read`, `send-keys`, `prompt`, `rename`, `focus`, `wait`, `attach`) e proibido e coberto por teste com herdr falso que registra as chamadas.
- **Processo:** no maximo **um** em voo; `source_timeout_ms` (padrao 3000): ao estourar, o grupo de processos recebe `SIGKILL` e o daemon **espera a coleta** antes de iniciar o proximo; stdout limitado a 1 MiB (excesso = leitura falha), stderr lido e **descartado** (ate 64 KiB), nunca guardado nem copiado.
- **Envelope (herdr 0.9.5):** stdout e **um objeto JSON** `{"id": ..., "result": {"agents": [AgentInfo...]}}` com saida 0; o erro do herdr sai em stderr com saida 1. Campos extras sao **ignorados**. Lidos de `AgentInfo`: `pane_id`, `agent`, `agent_status`, `cwd`, `state_change_seq`, `completion_seq`, `revision`, `terminal_id`, `launch_pending`, `screen_detection_skipped`; **`terminal_title*` e todo texto de tela sao ignorados e nunca guardados**. `pane_id` repetido na lista => leitura **toda** falha (`duplicate_pane`); `agent` nulo ou de outro `kind` => falha **so desse agente** (`kind_mismatch`); `state_change_seq` ausente conta como "sem contador" e **impede a confirmacao de `idle`** daquele painel.
- **Codigos de falha (so estes aparecem em `status`/logs):** `source_spawn_failed`, `source_timeout`, `source_exit_nonzero`, `source_bad_json`, `source_too_large`, `duplicate_pane`, `kind_mismatch`, `status_unmapped`.
- **Mapeamento de `agent_status`:** `working` -> `working`; `blocked` -> `blocked`; `idle` e `done` -> `idle` (sujeito a regra 5); `unknown` -> `unknown` **sem** `error`; qualquer outro valor -> `unknown` com `error` (`status_unmapped`). Toda saida tem `evidence: observed`.
- **Semantica conferida no codigo (herdr 0.9.5):** `done` e `idle` ainda nao visto pela UI (`api_helpers.rs:100-102`); `state_change_seq` e um contador global gravado por terminal na ultima mudanca daquele terminal (`actions.rs:1753-1761`), entao igualdade por painel significa "sem mudanca neste painel" e **saltos entre leituras sao normais**; `completion_seq` so em `working/blocked -> idle` nao suprimido. Se `state_change_seq` diminuir, o herdr reiniciou: a janela de confirmacao e zerada.
- **Nao verificado:** a semantica de `launch_pending`, `interactive_ready` e `screen_detection_skipped` (tratados de forma conservadora: verdadeiros => `unknown`); se `agent list` cobre todas as sessoes; o nome do campo-etiqueta do envelope; a reutilizacao de `wN` depois do reinicio (deduzida de `reserve_workspace_ids`).
- **Limite honesto:** o herdr (e o Luvus, `src/detect.rs:1301-1305`) devolve `idle` estavel para agente conhecido sem regra que case (`manifest.rs:550-553`), por exemplo um dialogo de permissao novo. Ver secao 0.
- **Fora do alcance de qualquer fonte que nao seja a tela:** saber se a caixa de entrada tem rascunho humano.

## 7. Configuracao (JSON, campo desconhecido e **chave duplicada** recusados)
```json
{
  "herdr_bin": "herdr",
  "poll_ms": 2000,
  "source_timeout_ms": 3000,
  "stale_after_ms": 30000,
  "gone_after_s": 120,
  "idle_confirm_polls": 2,
  "idle_confirm_ms": 2500,
  "ring": 4096,
  "agents": {
    "claude": {"kind": "claude", "sources": [{"type": "herdr", "pane": "w1:p1"}]},
    "codex-blocks": {"kind": "codex", "sources": [{"type": "herdr", "pane": "w3:p1"}]}
  }
}
```
Limites: `poll_ms` 200 a 10000; `source_timeout_ms` 200 a 30000; **`stale_after_ms` > `poll_ms` + `source_timeout_ms`**; `gone_after_s` 5 a 86400; `idle_confirm_polls` 1 a 5; `idle_confirm_ms` 0 a 30000; `ring` 64 a 65536; ate 32 agentes; `id` repetido, `pane` repetido para dois agentes, `kind` fora de `claude|codex` e `sources` com tamanho diferente de 1 sao recusados. Valor invalido ou campo desconhecido => nao sobe (saida 1).

## 8. CLI (`agentd`)
`agentd serve --config F [--socket P] [--state-dir D]` | `agentd get ID` | `agentd list [--kind K]` | `agentd watch [--since SEQ --epoch E]` | `agentd health` | `agentd status`.
- Socket (todos os comandos): `--socket`, depois `AGENTD_SOCKET`, depois `--state-dir`/`AGENTD_STATE_DIR` + `/agentd.sock`; sem nenhum, saida 2. `--json` em todos os comandos de leitura: `{"schema_version":1,"ok":bool,"code":str,"message":str,"data":...}`.
- Sem `--json`, uma linha por agente: `ID ESTADO[,MARCAS] EVIDENCIA FONTE idade=Ns`. Todo campo vindo de fonte e escapado contra caracteres de controle.
- Saidas: 0 ok; 1 falha do servidor; 2 uso incorreto (inclui `--since` sem `--epoch` e vice-versa); 3 agente inexistente; 4 daemon indisponivel. `agentd health` sai 0 so se o daemon responde e `sources.herdr.ok` e verdadeiro (nivel de **execucao**, nao de estado de agente).
- `watch` imprime uma linha por `state_changed`; reconecta com backoff, refazendo o bootstrap da secao 5 e avisando em stderr quando a `epoch` mudar.

## 9. Seguranca e privacidade
Nao le credenciais, prompt, resposta, ambiente nem conteudo de tela; sem escrita em agente nem multiplexador; **nenhum texto vindo de fonte** (stdout, stderr, titulo, JSON invalido) e copiado para `status`, eventos, CLI ou log alem de identificadores e codigos fixos; `agentd` nao abre rede. Um processo do mesmo uid **nao consegue declarar estado**: a unica entrada e a fonte configurada.

## 10. Contrato testavel (testes independentes; o `herdr` e FALSO: um script controlado que registra chamadas; o relogio e acelerado por `poll_ms`/`stale_after_ms` pequenos, e os limites exatos sao verificados por contagem de leituras e por tolerancia de +-1 ciclo)
- **G1** mapeamento: `working|blocked|idle|done`; `unknown` do herdr => `unknown` sem `error`; valor desconhecido (`xyz`) => `unknown`+`error`, nunca `idle`.
- **G2** `unknown` por padrao e leitura falha: sem painel; saida nao zero; JSON invalido; stdout excessivo; `pane_id` duplicado (leitura toda falha); `agent` nulo/`kind` divergente (so aquele agente).
- **G3** `stale` pela consulta: a leitura trava (herdr falso dorme); antes do `stale_after_ms` o agente e `unknown,error` (timeout); depois ganha tambem `stale` com `last_known`; volta com a leitura seguinte valida e confirmada.
- **G4** `gone`: ausencia valida por `gone_after_s` (inclusivo) => `gone`; antes `unknown,error`; agente nunca visto conta de `absent_since`; **lista vazia apos erros nao produz `gone` imediato**; `gone` volta a `working` com evento.
- **G5** eventos: `seq` sem lacunas; `previous_state` e `previous_marks`; mudar so `evidence`/`source` gera evento (via fonte controlada); mudar so `observed_at`, `freshness_ms`, `cwd` nao gera.
- **G6** `events.resume`: retoma valido; `resume_gap` com `oldest_seq`; **fronteira** (`oldest-1` retoma, `oldest-2` e gap); anel vazio; `epoch_mismatch` apos reinicio; `last_seq > head_seq`; segunda chamada; `last_seq` negativo.
- **G7** lista fechada: o herdr falso registra **apenas** `agent list` por 10 ciclos.
- **G8** sem shell: `pane` e `herdr_bin` com `;`, `$()`, espacos e aspas nao executam nada.
- **G9** protocolo: campo desconhecido; linha de exatamente 1 MiB com e sem LF; EOF sem LF; UTF-8 invalido; `params:null`; pedido sem `id`; lote; metodo inexistente; versao inesperada; o daemon continua vivo.
- **G10** seguranca: permissoes 0700/0600; outro uid recusado (senao `#[ignore]`).
- **G11** privacidade: sentinela em `terminal_title`, em **stderr** do herdr, em JSON invalido e em campo extra: nunca aparece em `agent.*`, `status`, eventos, CLI nem log.
- **G12** CLI: `get/list/watch/health/status` com e sem `--json`; codigos 0 a 4; `--since` sem `--epoch`; socket por `--state-dir`; escape de controle; `health` falha so por erro de **execucao** da fonte (nao por agente `unknown`).
- **G13** configuracao: campo desconhecido, **chave duplicada**, `poll_ms` fora do limite, `stale_after_ms <= poll_ms + source_timeout_ms`, `pane` e `id` repetidos, mais de 32 agentes, `sources` vazio ou com 2.
- **G14** reinicio: sobe de novo, `epoch` muda, nada persiste.
- **G15** confirmacao de `idle`: leitura isolada entre `working` nao muda o publicado e deixa `idle_unconfirmed`; duas consecutivas **com mesmo `terminal_id`/`state_change_seq` e `idle_confirm_ms` decorrido** confirmam; **toda** entrada em `idle` vinda de `unknown`/`error`/`stale`/`gone`/partida passa pela confirmacao; `working` e `blocked` sao imediatos; com `idle_confirm_polls:1` e `idle_confirm_ms:0` e imediato.
- **G16** zeragem da janela: `idle`, erro, `idle`; `idle`, ausente, `idle`; `idle`, `unknown`, `idle`; `idle`, `idle` com `state_change_seq` diferente; `idle`, `idle` com `terminal_id` diferente: nenhuma confirma.
- **G17** precedencia: erro + ausencia => `unknown,error`, nunca `gone`; ausencia ate o prazo => `unknown`; `unknown` do herdr sem `error`.
- **G18** limites: `stale` so acima de `stale_after_ms` (no limite exato, nao); `gone` no limite exato `gone_after_s`.
- **G19** painel reaproveitado: `terminal_id` muda => evento, `unknown,error`, janela zerada; `state_change_seq` diminui => janela zerada.
- **G20** atomicidade: `agent.list`/`get`/`status` concorrentes com atualizacao: o retrato e o `head_seq` sao do mesmo instante (nunca um agente em `idle` com `head_seq` posterior ao evento que o tornou `blocked`); replay + ao vivo sem perda nem duplicata.
- **G21** subprocesso: trava (excede `source_timeout_ms`), imprime >1 MiB, nada, descendente que fica vivo: o daemon continua; nunca ha dois `herdr` em voo; o proximo ciclo so comeca depois da coleta.
- **G22** assinante lento: cliente que nao le recebe `closed(slow_consumer)` e e desconectado; os polls nao atrasam.
- **G23** `launch_pending` ou `screen_detection_skipped` verdadeiros => `unknown` sem `error`.

## 11. Historico das revisoes (resolucao)
| Fonte | Achados principais | Onde ficou |
|---|---|---|
| codex-blocks (chatd seq 240) | precedencia, `agent list` so, `resume_gap`, eventos por campo, confirmacao por tempo, regra 7 futura | regras 1, 4, 5; secoes 5 e 6; Anexo |
| Opus 5.5 (R1 a R16) | `idle` apos `stale` pulava a confirmacao; qualquer nao-`idle` deve zerar; `state_change_seq` igual; `stale` na consulta; `gone` apos lista vazia; `terminal_id`; envelope; `launch_pending`; marcas; `previous_marks`; atomicidade; `epoch` string; formato de tempo; `stale_after > poll+timeout`; `health` | regras 1 a 6; secoes 3, 5, 6, 7, 8 |
| Sol 6.1 (F01 a F24) | `observed idle` nao e portao seguro; estabilidade nao e quietude; reinicio da fonte; ancora de `gone`; lista vazia; reuso de painel; corrida observacao/escrita; assinante lento; bootstrap; framing; configuracao; sanitizacao; coleta; reproducibilidade dos testes | secao 0; regras 2, 3, 5, 6; secoes 5, 6, 9, 10 |
**Nao adotado:** retirar o `watch` reconectavel (mantido: o consumidor precisa; o bootstrap esta definido). **Cortados da v1 (YAGNI):** marca `background`, evidencia `declared`, campos `branch/provider/model/task_id`, `state_seq`, `source_seq`, `source_completion_seq`, `next_cursor`, texto em `last_error`, regras de varias fontes e de relatos `declared` (movidas para o Anexo).

## 12. Fora desta spec (v1.1 em diante)
Fonte app-server do Codex; fontes de hook do Claude e interrupcao pela transcricao; modo sombra contra o herdr; integracao com o `chatd` (assinar `events.resume`; **so com fonte `declared` ou mecanismo seguro**, ver secao 0); suporte a tmux como fonte; persistencia do anel; Pi como terceiro agente.

## Anexo (nao normativo): contratos futuros
- **Varias fontes:** a prioridade so resolve conflito entre postos diferentes (`declared` fresco vence `observed` fresco; `declared` velho nao vence nada); duas fontes do mesmo posto frescas e em desacordo => `unknown` com `error`.
- **Relatos `declared`:** `sequence` crescente por agente e fonte; `ttl_s` de 5 a 3600 contado da recepcao (monotonico); relato expira para `unknown`; sequencia que nao cresce e ignorada; uma fonte `declared` por agente; canal e identidade da fonte a definir (mesmo uid nao e isolamento entre agentes). Modelo: `agent.report` do Luvus (`src/app/dispatch/agents.rs:407-520`). **Nao exercitavel na v1** (sem metodo de escrita).

## 13. Esclarecimentos dos testes independentes (2026-10-15 noite)
1. **Pedido:** o campo `"jsonrpc":"2.0"` e **aceito e opcional** no pedido; a resposta o traz. Primeiro `seq` de uma `epoch` e **1** (`head_seq` 0 = nenhum evento).
2. **`SIGTERM`** encerra o `serve` de forma limpa com saida **0**. Linha acima de 1 MiB: erro `invalid_request` e a conexao e fechada; lote (array): `invalid_request`; `data.path` e a trilha com pontos (`params.campo`).
3. **`agentd health`** com a fonte falha sai **1**; com o daemon fora, **4**. **`observed_at`** e `null` antes da primeira leitura valida (e `freshness_ms` tambem).
4. **Estado inicial** de um agente nunca lido: `state: unknown`, `evidence: none`, `source: null`, `marks: [error]` (a primeira leitura valida preenche).
5. **Anel retido:** os `ring` ultimos eventos; `oldest = head_seq - ring + 1` quando o anel esta cheio.
6. **Partida:** o daemon imprime em stderr uma linha `agentd: pronto` quando o socket aceita conexao.
7. **(rev4, decisao do Jabbur, 21):** `blocked_reason` (classe `permission|user_input|unknown`) e `since` entram em `Agente` e nos eventos; ainda **nao** implementados na v1 (nucleo).

