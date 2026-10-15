# `chatd`: caixa de correio durável entre agentes (spec v1)

Origem: decisão do Jabbur: "vamos para o chatd"; e (~03:45) "se possível deveria ser em Rust para um eventual aproveitamento lá [no Kelpie Drover] e o trabalho está autorizado". Pesquisa e justificativa: `docs/protocolo-chat-herdr-2026-10-15.md`, seções 1 a 12. **Este documento é o contrato** para quem implementa e para quem testa de forma independente: os testes falam com o **binário** e com o **protocolo no socket**, nunca com a API interna.

## 1. Problema, exemplo, solução
Hoje o Claude e o Codex se falam colando texto no painel um do outro (`herdr agent prompt`) e lendo a tela de volta. Exemplo real: o Codex terminou a T6 e escreveu o aviso no painel; o monitor de tela estava rolado e entregou o aviso 30 minutos depois. Não há registro durável, ordem, recibo nem portão ("só se o destinatário estiver ocioso").

Solução: um daemon pequeno, **escritor único**, que guarda cada mensagem num log append-only com três recibos (`registrado`, `aceito`, `resultado`) e só **toca a campainha** do destinatário (uma frase fixa, sem o conteúdo) quando ele está `idle` ou `done`. O conteúdo da mensagem nunca vira texto de terminal nem comando.

## 2. Escopo da v1 e o que não faz
Faz: log durável, ordem por `seq`, idempotência, 3 recibos, consulta da caixa, campainha com portão por estado, retomada por `events.resume`.
Não faz (registrado, fora da v1): chat livre, criptografia, autenticação por token (o socket é 0600 do mesmo usuário; **não há barreira contra código local malicioso do mesmo usuário**), retenção/rotação do log, detecção de rascunho humano na caixa do destinatário (risco residual: a campainha é uma frase curta e pode se juntar a texto digitado), entrega a vários hosts.

## 3. Estilo e compatibilidade com o Kelpie
Rust, edition 2024, toolchain 1.99.0, **síncrono com threads** (sem tokio), dependências já usadas pelo Kelpie: `serde`, `serde_json`, `libc`, `thiserror`, `chrono` (feature `clock`); `tempfile` só em testes. Transporte e convenções iguais às do Kelpie (`docs/spec.md` dele): JSON-RPC 2.0 por linha em socket Unix, campo `v`, chave de idempotência, `seq`, `events.resume`, erros com código **string**, diretório 0700 e socket 0600, trava exclusiva. Objetivo: o Kelpie poder ler e adaptar este código.

## 4. Linha de comando (contrato dos testes)
Binário `chatd`. Códigos de saída: 0 sucesso, 1 falha de execução ou erro do servidor, 2 uso incorreto. Erros do servidor saem em `stderr` como `chatd: <code>: <message>`.

| Comando | Efeito |
| --- | --- |
| `chatd serve --state-dir DIR --config FILE [--socket PATH]` | Sobe o servidor em primeiro plano. Cria `DIR` (0700), `DIR/log.jsonl` (0600), `DIR/chatd.lock`; socket padrão `DIR/chatd.sock`. Escreve `chatd: ready` em `stdout` depois de escutar. Encerra limpo em SIGTERM/SIGINT |
| `chatd send --from NOME --to NOME --kind KIND --subject TXT (--body TXT \| --body-file ARQ \| --body-file -) [--thread ID] [--id ID]` | Envia. Imprime uma linha JSON `{"id","seq","state":"registrado","duplicate"}`. `--id` ausente: gera um id aleatório de 32 hex |
| `chatd inbox --as NOME [--after SEQ] [--all] [--json]` | Lista mensagens **para** `NOME`; sem `--all` só as não resolvidas (`registrado` ou `aceito`). Sem `--json`: uma linha por mensagem `seq id state from kind subject` |
| `chatd ack --id ID --as NOME --stage aceito\|resultado [--result done\|refused\|failed] [--note TXT]` | Confirma recibo. `--result` só vale com `resultado` (padrão `done`) |
| `chatd status [--json]` | Por destinatário: pendentes (`registrado`), `aceito`, campainha (`none`, `idle_wait`, `outstanding`), último `bell_sent` |

Resolução do socket (servidor e clientes): `--socket`, depois `CHATD_SOCKET`, depois `<state-dir>/chatd.sock` com `--state-dir` ou `CHATD_STATE_DIR`. Sem nenhum dos três: código 2. **Não há diretório de estado padrão** (nada é escrito fora do que o chamador indicar).

## 5. Protocolo no socket
Uma requisição JSON-RPC 2.0 por linha UTF-8, terminada em `\n`, até **1 MiB**; lotes não são suportados; `id` da requisição é número ou string. Toda `params` carrega `"v": 1`. **Campo desconhecido em `params` de qualquer método é `invalid_envelope` com `data.path` = `/<campo>`.** Requisição sem `"jsonrpc": "2.0"`, com `id` nulo ou ausente, ou com `method`/`params` de tipo errado é `invalid_request` (e a resposta leva `id: null` quando não há `id` válido a ecoar). Uma linha com exatamente 1 MiB (1.048.576 bytes sem o `\n`) ainda é **dentro** do limite (só acima disso é `invalid_request`). Códigos de erro em `error.code` como **string**, `error.data.path` aponta o campo em `invalid_envelope`.

### 5.1 `message.send`
`params`: `{v, id, from, to, kind, subject, body, thread?}`.
- `id`: chave de idempotência da mensagem, 1 a 128 caracteres de `[A-Za-z0-9._:/-]`.
- `from`, `to`: nomes de destinatário `[a-z0-9_-]{1,32}`; **`to` precisa existir na configuração** (`unknown_recipient`); `from` precisa ser nome válido (não precisa ser destinatário).
- `kind`: `AVISO`, `PEDIDO`, `PERGUNTA`, `RESPOSTA`, `ACK`.
- `subject`: 1 a 200 caracteres sem quebra de linha; `body`: 0 a 16.384 bytes UTF-8; `thread`: id de mensagem já registrada (senão `unknown_message`).
- Resposta: `{v, id, seq, state:"registrado", duplicate, registered_at}`. A resposta só sai **depois** do `fsync` da linha no log.
- Reenvio do mesmo `id` com envelope idêntico (hash SHA-256 do JSON canônico de `from,to,kind,subject,body,thread`): devolve o estado atual com `duplicate:true` e o mesmo `seq`, sem nova linha. Com envelope diferente: `envelope_mismatch` e nada muda.

### 5.2 `message.list`
`params`: `{v, to, after_seq?, limit?, unresolved_only?}` (`after_seq` padrão 0; `limit` 1 a 200, padrão 50; `unresolved_only` padrão false). Resposta `{v, messages:[Mensagem...], head_seq}` ordenada por `seq` crescente (o `seq` é o do evento `registered`). `Mensagem` = `{v, seq, id, from, to, kind, subject, body, thread, state, registered_at, accepted_at, resulted_at, result, note}` (campos ainda sem valor vêm `null`).

### 5.3 `message.ack`
`params`: `{v, id, by, stage, result?, note?}`; `stage` = `aceito` ou `resultado`; `by` deve ser igual ao `to` da mensagem (`forbidden` se não). `result`: `done`, `refused` ou `failed` (só com `resultado`; padrão `done`). `note`: até 1.024 caracteres.
- Transições: `registrado → aceito → resultado`; `resultado` também vale direto de `registrado`; repetir o mesmo estágio **com o mesmo resultado** (ou sem `result`, que vale `done`) é idempotente (`duplicate:true`, estado inalterado); repetir `resultado` com `result` **diferente** do gravado é `invalid_transition` (conflito, não duplicata; o primeiro vale); `aceito` depois de `resultado` é `invalid_transition`.
- Resposta: a `Mensagem` atualizada mais `duplicate`.

### 5.4 `events.resume` e notificações
`params`: `{v, last_seq}`. Resposta `{v, last_seq, head_seq}`; depois dela o servidor envia, **na mesma conexão**, todo evento com `seq > last_seq` em ordem e segue entregando ao vivo, como notificações `{"jsonrpc":"2.0","method":"event","params":Evento}`. Uma conexão que nunca chamou `events.resume` não recebe notificações; uma segunda chamada na mesma conexão é `invalid_request`; `last_seq` maior que o `head_seq` é `invalid_envelope` (`/last_seq`). Entrega "pelo menos uma vez": o receptor deduplica por `seq`.
`Evento` = `{v, seq, ts, type, id, ...}` com `type` em `registered` (traz a `Mensagem` completa), `accepted`, `resulted`, `bell_sent`, `bell_failed` (traz `recipient` e, em falha, `reason` curto sem saída de comando).

### 5.5 `status`
Sem parâmetros além de `v`. Resposta `{v, head_seq, recipients:{NOME:{pending, accepted, bell:"none"|"idle_wait"|"outstanding", last_bell_seq}}}`.

### 5.6 Erros
`parse_error`, `invalid_request` (inclui mensagem acima de 1 MiB e lote), `method_not_found`, `unsupported_version`, `invalid_envelope`, `unknown_recipient`, `unknown_message`, `envelope_mismatch`, `forbidden`, `invalid_transition`, `too_many_connections` (teto 64, a conexão é fechada), `internal_error`. Erro de uma requisição **nunca** derruba o servidor nem outras conexões.

## 6. Persistência
- Um único arquivo `log.jsonl`: uma linha JSON por evento (`Evento` do 5.4), `seq` inteiro monotônico a partir de 1, sem lacunas. O estado de cada mensagem é a dobra dos eventos.
- Gravação: append da linha completa com `\n` e `sync_data` **antes** de responder ou notificar.
- Partida: reler o log inteiro. Última linha truncada (sem `\n` final ou JSON inválido **somente nela**): descartar com aviso em `stderr` e seguir. O `seq` seguinte continua do último evento **válido** (sem lacuna): a linha descartada não consome número. Linha inválida no meio, `seq` fora de ordem (para trás ou repetido) ou com lacuna, ou `id` de mensagem registrado duas vezes: **recusar subir** (saída 1, mensagem clara). Nunca reescrever linhas antigas.
- Trava exclusiva em `DIR/chatd.lock`: segundo servidor no mesmo diretório recusa subir (saída 1).
- Reinício: mensagens e recibos sobrevivem; o estado da campainha volta a `idle_wait` (uma campainha pode ser repetida, o que é inofensivo porque só leva a contagem).

## 7. Configuração (JSON, campos desconhecidos recusados)
```json
{
  "herdr_bin": "herdr",
  "bell_poll_ms": 5000,
  "bell_rearm_s": 600,
  "recipients": {
    "codex-web": {"bell": {"kind": "herdr", "pane": "w2:p1"}},
    "claude-main": {"bell": "none"}
  }
}
```
`bell: "none"` = nunca toca (por exemplo, o painel do Jabbur `w1:p1`). `herdr_bin` é caminho ou nome resolvido no `PATH`; usado **sem shell** (`Command` com `argv`). Padrões: `bell_poll_ms` 5000 (mínimo 50, para teste), `bell_rearm_s` 600.

## 8. Campainha (adaptador `herdr`)
Para cada destinatário com campainha e pelo menos uma mensagem em `registrado`, a cada `bell_poll_ms`:
1. Se já há campainha `outstanding` e `bell_rearm_s` não passou, nada.
2. Executa `herdr_bin agent get PANE` (limite de 5 s). Lê o JSON de `stdout`; o estado está em `result.agent.agent_status`. Só `idle` ou `done` passam; qualquer outro valor, erro, saída inválida ou tempo esgotado = não toca (e, em erro, registra `bell_failed` no máximo uma vez por minuto por destinatário).
3. Executa `herdr_bin agent prompt PANE "TEXTO"` (sem `--wait`, limite de 5 s). Saída 0: evento `bell_sent` e estado `outstanding`. Saída diferente: `bell_failed`.
4. `TEXTO` é **fixo e literal** (sem pluralizar; "(ns)" e "(s)" ficam como escritos): `[chat] N mensagem(ns) pendente(s) para NOME. Rode: chatd inbox --as NOME` (`N` e `NOME` apenas; **nunca** remetente, assunto ou corpo). A v1.1 substitui esta frase (ver `spec-v1.1-agentes.md`).
5. A campainha volta a `idle_wait` quando o destinatário aceita ou resolve qualquer mensagem, ou quando todas as suas mensagens saem de `registrado`.
Garantias: no máximo uma campainha por destinatário por `bell_rearm_s`; o conteúdo das mensagens nunca é passado a processo algum; falha da campainha não afeta o recibo `registrado`.

## 9. Segurança e limites
Diretório de estado 0700 e socket 0600 (recusar subir se não consegue); log 0600; nomes e ids validados antes de uso (nenhum vira caminho de arquivo nem argumento de comando exceto `PANE` e `NOME` fixos da configuração e o `NOME` já validado); corpo nunca interpretado; teto de 64 conexões; leitura de linha com limite de 1 MiB e tempo de ociosidade de 30 s por conexão; sem `unsafe` fora de chamadas diretas a `libc` (`flock`, `chmod`/`umask`).

## 10. Critérios de aceite
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes; testes independentes (escritos só a partir deste documento) cobrindo cada regra das seções 4 a 8; **mutantes mortos** (portão `scripts` do repositório ou lista manual); nenhuma rede, nenhum `herdr` real nos testes (o `herdr_bin` de teste é um script falso que grava o `argv` e imprime o JSON de estado); auditoria independente de segurança antes de qualquer uso com o Codex real.
