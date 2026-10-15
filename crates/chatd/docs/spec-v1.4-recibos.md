# chatd v1.4: correcao do `bind` no herdr e recibos visiveis ao remetente (contrato testavel)

Origem: pergunta do Jabbur (2026-10-15): "Seria importante ter no chatd algum tipo de ack sobre a mensagem?"; desenho aprovado ("Aprovado"). Contrato escrito pelo Sextante; implementacao pelo `codex-blocks` **depois** do v1.3 (`bind`); testes independentes por outro agente que nao le `src/`. Protocolo v1.1/v1.2/v1.3 intacto para quem nao usa as novidades.

## Problema (exemplo)
O Claude envia o PEDIDO B-093 ao Codex. O daemon registra, toca a campainha e o Codex aceita 7 s depois, mas o Claude so descobre lendo `chatd log` no diretorio de estado. O `send` devolve apenas `registrado`; `inbox` e `status` falam das mensagens **para** quem pergunta; `watch` imprime so `registered`. Sem recibo visivel o remetente nao distingue "ainda nao leu" de "leu e esta trabalhando", e nao sabe se a mensagem foi recusada (`refused`/`failed`).

## O que ja existe (nao muda)
- Tres recibos por mensagem: `registrado`, `aceito` (automatico no `inbox --take`), `resultado` (`done|refused|failed`, por `ack`).
- `events.resume` ja entrega `accepted` e `resulted` ao **remetente** (regra v1.1: eventos em que a identidade e `from` ou `to`). Nenhuma mudanca de visibilidade.

## 0. Correcao ao v1.3: tipo do agente no `bind` do herdr (defeito do contrato v1.3)
Problema (exemplo real, achado pelo `codex-blocks` e confirmado em `src/binding.rs`): o v1.3 mandava o tipo do agente do painel (`herdr agent get` -> `agent`) coincidir com o **nome do destinatario**. O painel `w3:p1` e do tipo `codex` e o destinatario e `codex-blocks`; logo `bind` com `delivery: auto` e **sempre recusado** para ele, e a campainha nunca toca. O erro e do contrato, nao da implementacao.
Regra nova:
1. `recipients.<nome>.agent` (opcional, `[a-z0-9_-]{1,32}`): tipo de agente esperado no painel. Ausente: o trecho do nome antes do primeiro `-` (`codex-blocks` -> `codex`; `claude` -> `claude`; `codex` -> `codex`).
2. A verificacao herdr compara o tipo do painel com esse valor (nao mais com o nome inteiro); cwd continua obrigatorio e, se o herdr publicar pid, a ancestralidade tambem.
3. Valor invalido em `agent` e erro de configuracao (saida 1, como os demais campos). Campo desconhecido continua recusado.
Testes (multiplexador falso, como bind_v13): **Z1** nome `codex-blocks`, painel `codex`, cwd igual: ligado; **Z2** painel `claude` para `codex-blocks`: `forbidden` + `denied`; **Z3** `agent: "codex"` para `custom-bot`: ligado; **Z4** `agent` invalido (`"Co dex"`): chatd nao sobe; **Z5** nome `claude` com painel `claude`: ligado (padrao).

## Parte B: recibos visiveis ao remetente
## Regras
1. **Opt-in.** Sem as opcoes novas, `chatd watch` imprime exatamente o que imprimia na v1.1/v1.2 (compatibilidade dos Monitors armados).
2. **`chatd watch --receipts`**: alem de cada `registered` dirigido a identidade, imprime **uma linha por recibo** de mensagem **enviada pela identidade** (`from` = identidade), em ordem de `seq`:
   - `RECIBO SEQ ID aceito TO`
   - `RECIBO SEQ ID resultado:done TO` (ou `refused`, `failed`)
   `SEQ` e o `seq` do evento de recibo; `ID` o da mensagem; `TO` o destinatario. Se este mesmo processo ja viu o `registered` da mensagem, acrescenta ` | SUBJECT` (assunto escapado como em `watch`); caso contrario nao inventa assunto. Linha comeca por `RECIBO ` (7 caracteres) para nao se confundir com linha de mensagem (que comeca por inteiro).
3. **Recibos de mensagens alheias nunca aparecem** (a identidade so e `from` ou `to`; o `watch --receipts` filtra tambem `to` = identidade: o recibo do proprio aceite de quem tomou nao e repetido a ele).
4. **`--resume-file`, `--exit-after`, `--reconnect`** (v1.2) valem iguais: o cursor grava o `seq` do ultimo evento **recebido**, recibo inclusive; reconexao nunca imprime o mesmo recibo duas vezes.
5. **Novo metodo `message.get`** `{v:1, id}`: devolve a `Mensagem` (formato 5.2 da spec) **se a identidade e `from` ou `to`**; caso contrario `unknown_message` (nao distingue "nao existe" de "nao e sua", sem evento `denied`). Resposta `{v, message, head_seq}`.
6. **`chatd show --id ID [--as N] [--json]`**: usa `message.get`.
   - Sem `--json`, texto fixo (campos escapados contra controle, como `inbox`):
     ```
     ID ESTADO FROM>TO KIND | SUBJECT
     registrado  <registered_at>
     aceito      <accepted_at> [note]
     resultado   <resulted_at> <result> [note]
     ```
     (linhas dos estagios ainda nao atingidos sao omitidas; corpo **nao** e impresso sem `--json`).
   - `--json`: uma linha com a `Mensagem` completa.
   - Saida 0 achou; 1 `unknown_message` ou erro do daemon; 2 uso.
7. **`chatd send` nao muda** (continua devolvendo `registrado`).
8. **Nenhum recibo carrega corpo.** `note` e exibido apenas em `show` e ja e escapado (spec v1.1, secao F).

## Regra de uso (fora do codigo; vai para `AGENTS.md`)
Toda RESPOSTA a uma mensagem leva, do mesmo agente, `chatd ack --id <ID da original> --stage resultado --result done|refused|failed`. Sem isso o remetente fica com `aceito` para sempre (hoje: os 4 itens da caixa do Claude). Alternativa mecanica (**nao** faz parte deste contrato; decisao do Jabbur depois): `send --kind RESPOSTA --thread ID` fechar sozinho o `resultado` da original.

## Contrato testavel
- **A**: `watch` sem `--receipts` nao imprime nenhuma linha `RECIBO` mesmo com `aceito` e `resultado` ocorrendo (diferenca de saida zero contra a v1.2).
- **B**: com `--receipts`, `A` envia a `B`; `B` faz `inbox --take`: `A` recebe `RECIBO SEQ ID aceito B`; `B` faz `ack resultado done`: `A` recebe `RECIBO SEQ ID resultado:done B`. Ordem por `seq`, uma linha por recibo, sem duplicata.
- **C**: `resultado refused` e `failed` aparecem como `resultado:refused` / `resultado:failed`.
- **D**: isolamento: recibos de mensagens entre `B` e `C` nao chegam a `A`; `B` (destinatario) nao recebe linha `RECIBO` do proprio aceite.
- **E**: `--resume-file` + `--exit-after`: recibos ocorridos entre duas execucoes sao entregues uma unica vez na segunda; reiniciar o daemon durante `--reconnect` nao duplica.
- **F**: `show` da propria enviada e da recebida mostra estagios e horarios; de mensagem alheia e de id inexistente da a mesma saida (`unknown_message`, saida 1, texto identico); `show --json` bate com `message.list`.
- **G**: controles (ESC, BEL) em `subject` e `note` nunca chegam crus ao terminal em `watch --receipts` e `show`.
- **H**: sem regressao: suites v1.1, v1.2, v1.3 e a auditoria F passam; `message.get` sem autenticacao e recusado (`unauthenticated`); campo desconhecido em `message.get` e `invalid_envelope`.

## Fora de escopo
Alarme por falta de aceite apos N minutos e nova campainha (junto da logica de `idle`/`bind`, v1.5); `chatd sent` (lista de enviadas); resultado automatico por RESPOSTA; recibo de "lido na tela".
