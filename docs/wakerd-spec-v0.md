# wakerd v0: despertar de agentes (ARQUIVADO em 2026-10-15: nao implementar)

> **Arquivado por decisao do Jabbur (2026-10-15, "P75a: Confirmado")** depois da consulta Opus + Sol 6.1 (`docs/consulta-arquitetura-despertar-2026-10-15.md` da bancada): adotada a combinacao D + E, sem daemon novo; o verificador e `chatd stale` (`crates/chatd/docs/spec-v1.7-stale.md`). Mantido so como registro. A regra (e) `allowed_senders` deste rascunho e impossivel pela ACL do `events.resume`.

(Texto original abaixo.)

Estado: **rascunho de conceito**, escrito antes de o daemon do `agentd` existir. Nao implementar antes de: (1) spec do `agentd` revisada, (2) `agentd` v1 com testes independentes. Linguagem: Rust, no workspace `blocks` (decisao do Jabbur). Autor: Sextante (Claude Sonnet 5.5).

## 1. Problema, exemplo, solucao
**Problema:** uma mensagem chega para um agente que nao esta olhando. Hoje cada caso tem um mecanismo diferente e nenhum fala com o outro: o Codex e tocado pela campainha do `chatd`; eu so recebo dentro de um `Monitor` da sessao; o Pi nao fica ocioso (roda por job). Se a minha sessao fecha, ninguem me acorda.
**Exemplo:** o Codex da Web responde um PEDIDO as 02:10 e minha sessao esta fechada. A mensagem fica `registrado` ate o Jabbur abrir uma sessao e rodar `chatd inbox`.
**Solucao:** um daemon pequeno que **decide** quando e quem acordar e **age por atuadores**, sem falar com multiplexador nenhum.

## 2. O que o `chatd` e o `agentd` ja fazem (verificado nas specs)
| Peca | Ja faz | Consequencia para o `wakerd` |
|---|---|---|
| `chatd` | Campainha para destinatarios `bell` com **portao `idle`/`done`** (`herdr agent get`), texto fixo, 1 toque por `bell_rearm_s`; `status` devolve por destinatario `pending`, `accepted`, `bell`, `last_bell_seq` e `head_seq` | **Nao duplicar** a campainha do Codex. Contagens de pendentes ja existem (sem corpo): nao precisa de metodo novo no `chatd` |
| `agentd` | Estado por agente (`idle` confirmado, `working`, `blocked`, `gone`, `unknown`), `last_seen`, `evidence`, `events.resume`, somente leitura | Fonte unica de "o agente esta onde e como" |

## 3. Escopo
**Faz:** ler `chatd status`/`events.resume` e `agentd agent.list`/`events.resume`; aplicar uma regra por agente; acionar um atuador; manter orcamento e limite de taxa; registrar tudo em log.
**Nao faz (e e teste):** tocar tmux/herdr, digitar em terminal, ler corpo ou assunto de mensagem, ler credencial alem do proprio token, executar texto vindo de mensagem, decidir autorizacao.

## 4. Regra de despertar (por agente)
Acordar X quando **todas**: (a) `pending(X) > 0` ha pelo menos `min_pending_s` (padrao 60); (b) a politica de X nao e `none`; (c) estado de X no `agentd` casa com a politica (ver 5); (d) nao ha despertar de X em curso e o limite de taxa (`min_interval_s`, padrao 600) e o teto do orcamento permitem; (e) o remetente de alguma mensagem pendente esta em `allowed_senders` de X (o `wakerd` ve so `from` via `events.resume`, nunca corpo).
Depois de acordar, nao repete ate o `pending` cair ou `min_interval_s` passar.

## 5. Atuadores (um por tipo, sem trait generico)
| Agente | Estado que autoriza | Atuador | Observacao |
|---|---|---|---|
| **Codex** (painel) | `idle` | **Nenhum no v0**: a campainha do `chatd` ja cobre. O `wakerd` so **observa** e registra `wake_skipped reason=bell_owner` | Pergunta aberta 1 |
| **Pi** (executor na jaula) | qualquer | **Enfileirar um job** na fila do `escriba` | Pergunta aberta 2 (a fila e de outra conta) |
| **Claude** | `gone` | **`claude --resume <sessao>`** em processo filho, sem terminal interativo, com teto de gasto e de duracao | Pergunta aberta 3; so para remetente em `allowed_senders` |

## 6. Orcamento e seguranca
- `dry_run: true` por padrao: registra `would_wake` e nao age.
- Orcamento: `max_wakes_per_hour` por agente e global; `max_claude_wakes_per_day`; teto de duracao do processo do Claude. Estourou = `wake_denied reason=budget`, sem acordar.
- Config JSON fechada (campo desconhecido recusado); socket e estado 0700/0600; `argv` sem shell; nenhum valor vindo de mensagem entra em argumento.
- Identidade propria no `chatd` (`wakerd`), com token proprio e **somente** os metodos `status` e `events.resume`.
- Falha fechada: fonte indisponivel ou `unknown` = nao acorda.

## 7. Contrato testavel (esboco)
- **W1**: `pending>0` ha menos de `min_pending_s`: nao acorda. **W2**: `idle` + remetente permitido + passou o tempo: um `would_wake` (dry-run). **W3**: `working`/`blocked`/`unknown`: nao acorda. **W4**: `min_interval_s`: no maximo um por janela. **W5**: orcamento estourado: `wake_denied`. **W6**: nenhuma chamada a `read`, `send-keys`, `prompt` do herdr nem a `bell`. **W7**: o corpo da mensagem nunca e pedido nem aparece no log. **W8**: `gone` + Claude + `allowed_senders`: atuador chamado uma vez com `argv` fixo. **W9**: reinicio do `wakerd` nao repete despertar ja feito (estado minimo persistido: ultimo `seq` acordado por agente). **W10**: fonte caida: nada acontece, `source_unavailable` no log.

## 8. Perguntas abertas (nao verificadas)
1. A campainha do Codex fica no `chatd` ou migra para o `wakerd` (o `chatd` perderia `herdr`)? Recomendacao: fica, ate o `wakerd` provar a regra.
2. Como um daemon de outra conta enfileira job no `escriba` sem ampliar privilegio? Exige desenho com o dono do `escriba`.
3. `claude --resume` abre sessao sem terminal? Que arquivo/ID de sessao usar quando a sessao fechou? Que permissoes o processo herda? Exige sondagem (F1, so leitura).
4. Teto de gasto do atuador do Claude: decisao do Jabbur no passo 7.
