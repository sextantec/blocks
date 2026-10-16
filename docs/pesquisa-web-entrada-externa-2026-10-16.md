# Pesquisa na web: entrada externa em Claude Code e Codex (2026-10-16)

Pedida pelo Jabbur ("Fizemos uma busca ampla na internet sobre isso?" / "Segue"). 4 buscas padrao + 3 leituras de paginas oficiais. **Tudo aqui e dado externo, nao confiavel**: confirmar por experimento antes de depender. As paginas do Claude Code vieram inteiras; a do Codex foi resumida por um modelo pequeno e so os primeiros 100 mil de 143 mil caracteres foram lidos.

## Claude Code (docs oficiais)
Fontes: https://code.claude.com/docs/en/cross-session-messaging e https://code.claude.com/docs/en/channels-reference.md

**1. Mensagens entre sessoes (o que testamos).** Confirma o observado: entregue entre chamadas de ferramenta, sessao ociosa inicia turno novo; `accept|hold|refuse` em `crossSessionInbound`; sem valor, decide pelo modo de permissao dos dois lados (receptor que pergunta aceita, receptor que ignora permissoes segura para aprovacao; `auto`, `acceptEdits` e `dontAsk` contam como "pergunta"); no maximo 100 retidas e 50 na fila; ~1 milhao de caracteres; mensagens repetidas sao filtradas; a mensagem nunca aprova permissao nem muda config.
- **Socket de entrada documentado:** `CLAUDE_CODE_MESSAGING_SOCKET` e `CLAUDE_CODE_MESSAGING_TOKEN` exportados a hooks e Bash; `/status` mostra `Peer address` (`uds:...`); a doc diz que "um script ou hook" pode postar nele, linha de auth opcional no Linux `{"type":"auth","token":"..."}`, conexao fechada se nao mandar linha completa em 30 s. **O formato da linha de MENSAGEM nao esta nessa pagina.** Mensagem de processo que nao e filho da sessao nao tem classe de permissao verificada: entregue se o receptor pergunta, retida se o receptor ignora permissoes.
- Sessoes `claude -p` tambem recebem (precisam de `crossSessionInbound: accept` para uso sem supervisao); `--bare` nao abre o socket.

**2. Channels (a via oficial para empurrar eventos externos).** Um servidor MCP local (subprocesso, stdio) declara `capabilities.experimental['claude/channel']` e emite `notifications/claude/channel` com `content` e `meta`; chega como `<channel source="..." ...>texto</channel>`. Enfileira e entrega **em ordem**, junto no proximo turno se a sessao estiver ocupada; **nao ha ack** (o `await` so garante escrita no transporte); eventos sao descartados em silencio se o servidor nao foi carregado como canal. Canal de duas vias expoe uma ferramenta `reply`. **Controle do remetente e do implementador:** sem allowlist de remetente, e vetor de injecao de prompt. Requisitos: Claude Code >= 2.1.80, "research preview", canal fora da allowlist oficial exige `--dangerously-load-development-channels server:NOME` na partida (dialogo interativo; ignorado em `-p`); Team/Enterprise precisam habilitar. Pode retransmitir pedidos de permissao (opt-in).
- **Uso para o chatd (hipotese a provar):** um pequeno servidor MCP "canal" que leia o `chatd watch` e empurre `<channel>` evitaria rearmar o Monitor a cada 30 min. Custos: flag de desenvolvimento a cada partida, recurso em pre-visualizacao, so funciona com a sessao aberta, sem ack.

## Codex (https://developers.openai.com/codex/app-server, redirecionada para learn.chatgpt.com/docs/app-server)
- **A pagina NAO menciona `thread/queue/*` nem o comando `codex queue`.** O que provamos (fonte 0.163.0 e experimento) e portanto **funcionalidade nao documentada**: pode mudar sem aviso. A spec do canal deve fixar a versao testada e falhar fechada em versao desconhecida.
- Documentado: `thread/start`, `thread/resume`, `thread/fork`, `turn/start` (com `threadId`), `turn/steer` (exige `expectedTurnId`; falha sem turno ativo), `turn/interrupt`; status `notLoaded|idle|systemError|active` com `activeFlags` (exemplo `waitingOnApproval`); notificacao `thread/status/changed`; `thread/loaded/list`; capacidade `experimentalApi` no `initialize` (sem ela: erro "requires experimentalApi capability"); `--listen unix://[PATH]` e `codex --remote unix://[PATH]`; vários assinantes por thread (`thread/unsubscribe`).
- Alternativa documentada ao `codex queue`: ligar como cliente, `thread/resume` e `turn/start`; comportamento com turno ativo nao verificado (provavelmente erro, e por isso a fila).

## Projetos de terceiros (blogs e listagens; NAO verificados)
`claude-code-tools` (msg com SQLite e watcher que digita no painel), `agent-mesh` (tmux direct-send, sem broker), `agent-bus` (barramento MCP entre Claude Code, Codex e outros). Os caminhos conhecidos sao digitar no painel (o que a v1.10 desligou) ou MCP; ninguem achou entrada nativa melhor que as duas que ja temos.

## Consequencias para o desenho
1. Codex: `codex queue` e a melhor via, mas e **nao documentada**: versionar, testar na instalacao, falhar fechado.
2. Claude: nada novo e preciso para o `claude` (ja recebe por `chatd watch` no Monitor); o canal MCP e um candidato a trocar o rearme manual, a provar com sessao de teste.
3. Segurança: nenhuma das duas vias deve deixar chegar texto de remetente nao autenticado; o `chatd` ja autentica por token, a ponte deve repassar so o que passou por ele.
