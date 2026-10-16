# agentd v1 rev6: estado estruturado como fonte primaria, a tela como segunda opiniao | RASCUNHO rev6 (2026-10-16)

**Estado: PROPOSTA (spec).** Origem: o Jabbur, ao ver o modo sombra (2026-10-16 ~17h50): "Me preocupa o fato da raspagem de tela. Olha a mudanca que tivemos em uma versao pequena do claude" e, depois, "Siga em frente" (para uma rev6 com estados estruturados). Escrita pelo Sextante. **Nao ha codigo nesta revisao**; a implementacao e os testes independentes dependem de ok do Jabbur sobre esta spec. Evidencias: secao 13.

## 0. Relacao com a rev5 (leia primeiro)
A rev6 **emenda** a `spec-rev5.md` (e, por ela, a `spec.md` rev4); o que nao e dito aqui vale como esta. Muda: (a) o principio 5 da rev5 ("a tela e a unica fonte de estado; hooks nao entram"); (b) a regra "uma fonte por agente"; (c) acrescenta duas fontes estruturadas (Codex e Claude) e uma regra de fusao. **Nao muda:** o protocolo, o arquivo de regras de tela, a confirmacao de `idle` (regra 5'), a privacidade, o fato de o daemon nunca digitar.

## 1. Problema, exemplo, solucao
**Problema.** A tela de um agente e um detalhe de implementacao que muda sem aviso. Medido hoje no modo sombra (87 amostras, `docs/estado-vivo.md`): o Claude 2.1.302 trocou o rodape de trabalho (some `esc to interrupt`; aparece `✢ Cogitating… (1m 7s · ↓ 5.3k tokens)`) e o modo vi do Jabbur poe `-- INSERT --` antes da linha de modo, que impede a regra de `idle`. Resultado: o `claude` ficou `unknown` o tempo todo, **inclusive ocioso**. Falhou para o lado seguro (zero falso `idle`), mas a deteccao ficou inutil depois de uma versao patch (2.1.301 -> 2.1.302). A raspagem so avisa quando quebra se alguem olhar.

**Exemplo.** O `wakerd` (futuro) precisa saber se o `codex-blocks` esta ocioso antes de entregar uma mensagem. Hoje isso vem de um regex na tela. O app-server do Codex **ja diz** `thread/status/changed {"status":{"type":"idle"}}`, e o Claude Code tem os hooks `UserPromptSubmit`, `Stop`, `PermissionRequest`. Ler o estado de quem o produz e mais barato, mais rapido (milissegundos em vez de 2 s de ciclo) e nao quebra quando o rodape muda.

**Solucao.** Cada agente ganha, alem da fonte `tmux` (identidade, presenca e segunda opiniao), **uma fonte estruturada**: `codex_app_server` (Codex) ou `claude_hooks` (Claude). O estado publicado vem da **fusao** (secao 6), com uma assimetria de seguranca: **qualquer fonte pode ligar `working`/`blocked` na hora; so `idle` exige a fonte estruturada** (ou a tela, em um caso limitado e marcado). Divergencia entre as fontes vira marca visivel (`diverged`), e a deriva das regras de tela vira alarme, nao silencio.

## 2. Principios (substituem o 5 da rev5; os demais valem)
5'. **A fonte estruturada manda; a tela confirma e cobre as lacunas.** Hooks e app-server dizem a intencao do agente; a tela mostra o que realmente aparece. Nenhuma das duas e confiavel sozinha: os hooks nao cobrem `Esc` (o `Stop` **nao dispara** em interrupcao do usuario, documentacao oficial dos hooks) e o app-server pode estar desligado; a tela muda com a versao.
7. **Assimetria de seguranca:** transicao **para** `working` ou `blocked` vale com **qualquer** fonte, de imediato; transicao **para** `idle` so com a fonte estruturada dizendo `idle` **e** a tela nao contradizendo (secao 6), ou pela excecao `screen_override`.
8. **Nenhum dado da conversa sai do daemon:** das fontes novas entram so **nomes de evento e estados enumerados** (nunca o texto do prompt, `last_assistant_message`, nomes de ferramenta, comandos ou caminhos).

## 3. Modelo (acrescimos)
- **`sources` por agente:** a tela (`tmux`, **obrigatoria**, exatamente a da rev5) **mais no maximo uma** estruturada: `{"type":"codex_app_server","thread_file":"/caminho/absoluto"}` (so `kind: codex`) ou `{"type":"claude_hooks"}` (so `kind: claude`). Outras combinacoes: config recusada.
- **`evidence`:** passa a `observed` (tela), `reported` (fonte estruturada) ou `both` (concordam na ultima leitura valida). **`source`:** `"tmux" | "app_server" | "hooks"` = a fonte que decidiu o estado publicado.
- **Marcas novas:** `diverged` (as fontes discordam ha mais de `diverge_after_ms`), `structured_lost` (a fonte estruturada configurada nao esta ativa: sem conexao, sem relato recente conforme a secao 5, ou thread desconhecida), `screen_override` (o `idle` publicado veio da tela porque a estruturada ficou presa em `working`, secao 6). Todas **geram evento** quando entram ou saem, como as outras marcas.
- **Contadores de saude da tela** em `status` (nao em `agent.*`): `screen.reads`, `screen.unmatched` e `screen.unmatched_ratio` por `kind` nos ultimos 5 min (**alarme de deriva**: razao alta com a estruturada ativa indica regras velhas; nao muda estado).

## 4. Fonte `codex_app_server`
- **Identidade da thread:** o `thread_file` (mesmo arquivo do `chatd`, v1.11: UUID minusculo, sem symlink, do usuario do daemon, sem escrita de grupo/outros, <= 64 bytes, relido a cada reconexao e a cada 30 s; validacao identica a do `chatd`). Ausente ou invalido => `structured_lost` (sem erro fatal).
- **Conexao:** WebSocket sobre o socket Unix de controle (`~/.codex/app-server-control/app-server-control.sock`, o padrao do Codex; configuravel por `codex_socket`), caminho **`/`** (jamais `/daemon/shutdown`). Sem dependencia nova de rede: cliente WebSocket minimo proprio (handshake HTTP Upgrade, quadros mascarados de texto) ou uma crate aprovada pelo Jabbur.
- **Lista fechada de chamadas:** `initialize` (com `capabilities` sem `experimentalApi`), a notificacao `initialized`, `thread/loaded/list` e `thread/read` (com `includeTurns: false`, so o `status`) para o estado inicial; depois **apenas escuta** `thread/status/changed` da thread configurada. **Proibido e coberto por teste:** `thread/resume`, `turn/*`, `thread/queue/*`, `thread/unsubscribe`, `thread/archive`, qualquer escrita, e qualquer outro caminho que nao seja `/`.
- **Mapeamento (documentado na referencia oficial do app-server; `ThreadActiveFlag` em `app-server-protocol/src/protocol/v2/thread.rs:1698-1701`):** `{"type":"idle"}` => `idle`; `{"type":"active","activeFlags":[]}` => `working`; `activeFlags` com `waitingOnApproval` => `blocked` com `blocked_reason: permission`; com `waitingOnUserInput` => `blocked` com `blocked_reason: user_input`; `notLoaded` e `systemError` => `unknown` (marca `structured_lost` para `notLoaded`); qualquer outro tipo ou campo inesperado => `unknown` (nunca `idle`).
- **Entrega observada:** a notificacao e transmitida a **todas** as conexoes (fonte: `app-server/src/thread_status.rs:246` com `outgoing_message.rs:773-784`, lista vazia = `Broadcast`); confirmado por experimento em 2026-10-16: um ouvinte passivo, sem `thread/resume`, recebeu `active` e `idle` da thread do `codex-blocks`.
- **Reconexao:** quando o socket cai ou o `initialize` falha, `structured_lost` e nova tentativa com recuo (1 s, 2 s, 4 s ... ate 30 s); nunca pendura o ciclo da tela. Ao reconectar, relê o estado inicial por `thread/read`.

## 5. Fonte `claude_hooks`
- **Como o relato chega:** o Claude Code executa um comando de hook (config em `settings.local.json`/`settings.json`, secao 10). O comando e `agentd report EVENTO` (secao 8): lê `$TMUX_PANE` do ambiente do hook (herdado do Claude; medido: `TMUX_PANE=%0`), conecta ao socket do `agentd` (modo 0600) e envia **so** `{"event": EVENTO, "pane": "%N"}`. O daemon aceita o relato **somente** se `pane` e o painel configurado do agente `kind: claude` (resolvido pela fonte `tmux` do mesmo agente); relato de painel nao configurado => descartado e contado (`reports.rejected`), nunca vira estado.
- **Eventos aceitos (lista fechada) e mapeamento:** `UserPromptSubmit` => `working` (tambem cobre turnos que o Claude Code inicia sozinho); `PermissionRequest` => `blocked`/`permission`; `Elicitation` => `blocked`/`user_input`; `Stop` => `idle`; `StopFailure` => `idle` (o erro foi do turno, nao do agente); `SessionStart` => `idle`; `SessionEnd` => `gone` (confirmado pela ausencia do painel, regra 1b). Evento fora da lista => descartado. O texto do prompt, `last_assistant_message` e demais campos do JSON do hook **nao sao lidos pelo `agentd report`** (o comando ignora o stdin).
- **O que os hooks nao cobrem (e a tela cobre):** `Esc` (interrupcao nao dispara `Stop`): o estado fica `working`; resolvido pela excecao `screen_override` (secao 6). Retomada de permissao: depois de `PermissionRequest` nao ha evento de "aprovado"; o `PostToolUse`/`Stop` seguinte (ou a proxima mudanca de tela) resolve; enquanto isso vale a regra de `blocked` da tela. Subagentes em segundo plano: a marca `background` continua vindo da tela.
- **Relato velho:** `structured_lost` quando nao ha relato desde `hooks_silence_s` (padrao 600) **e** a tela mostra o agente trabalhando (um agente ocioso e silencioso e normal; um agente claramente ativo sem relatos indica hook desinstalado ou quebrado).
- **Falha do hook nunca derruba o Claude:** o comando sai sempre com 0 em menos de 1 s, mesmo com o daemon fora do ar, e roda como `async`.

## 6. Regras de fusao (por agente, a cada leitura e a cada relato)
Seja `E` o estado da fonte estruturada (mais recente, valido), `T` o estado da tela (regras da rev5, ja com a confirmacao do `idle` da rev5), e `idade(E)` o tempo desde o ultimo relato de `E`.
1. **Sem estruturada ativa (`structured_lost` ou nao configurada):** vale exatamente a rev5 (a tela decide, `idle` por prova positiva).
2. **`E` e `working` ou `blocked`:** publica `E` (de imediato). A tela nao rebaixa por conta propria, exceto a regra 5.
3. **`T` e `working` ou `blocked` e `E` e `idle`:** publica `T` (a tela liga o nao-ocioso: seguranca) e marca `diverged` se durar mais de `diverge_after_ms` (padrao 5000).
4. **`E` e `idle` e `T` nao contradiz** (`T` e `idle` ou `unknown`/`unmatched`): publica `idle`. Aqui `unknown` da tela **nao** impede o `idle` (e o caso do Claude 2.1.302: a tela nao casa nada, a estruturada manda). Exige tambem a confirmacao da rev5 sobre o `terminal` (mesmo par `pane_id`/`pane_pid`), mas **nao** as duas leituras de tela: o relato estruturado ja e discreto.
5. **`screen_override` (o caso do `Esc`):** `E` e `working`, `idade(E) >= override_after_s` (padrao 15), e `T` e `idle` por regra positiva confirmada (as 3 condicoes da regra 5' da rev5) por esse tempo todo: publica `idle` com a marca `screen_override`. Sem regra de tela que case `idle` (`unknown`), **nao ha override**: permanece `working` (falso `working` e seguro; o proximo relato corrige). Em `blocked` nunca ha override.
6. **Painel ausente / gone:** igual a rev5 (a fonte `tmux` e a unica que decide presenca); relatos de um painel ausente sao ignorados.
7. **Eventos:** continuam `state_changed` com `seq` (rev4 regra 4), agora tambem com `source` e `evidence` do estado novo; a entrada e saida de `diverged`, `structured_lost` e `screen_override` geram evento.

## 7. Configuracao (acrescimos)
```json
{
  "diverge_after_ms": 5000, "override_after_s": 15, "hooks_silence_s": 600,
  "codex_socket": "/home/USUARIO/.codex/app-server-control/app-server-control.sock",
  "agents": {
    "claude":       {"kind":"claude","sources":[{"type":"tmux","pane":"%0"},{"type":"claude_hooks"}]},
    "codex-blocks": {"kind":"codex","sources":[{"type":"tmux","pane":"%1"},{"type":"codex_app_server","thread_file":"/home/USUARIO/.local/state/chatd/threads/codex-blocks"}]}
  }
}
```
Campo desconhecido e chave duplicada continuam recusados. `codex_socket` e opcional (padrao: o do Codex). Sem a fonte estruturada o agente funciona como na rev5.

## 8. CLI (acrescimos)
- `agentd report EVENTO` (para hooks): lê `$TMUX_PANE`; ignora o stdin; envia o relato ao socket (`AGENTD_SOCKET`/`--socket`); **sai 0 sempre**, em menos de 1 s, mesmo sem daemon (nao pode atrapalhar o Claude); escreve nada na saida padrao.
- `agentd status` ganha `sources.app_server`, `sources.hooks` (`{ok, last_ok_at, last_error, reports: {accepted, rejected}}`) e `screen` (contadores da secao 3).

## 9. Seguranca e privacidade (acrescimos)
- O socket do daemon continua modo 0600 em diretorio 0700; relatos de outros usuarios nao chegam. Qualquer processo do mesmo usuario poderia relatar: o relato **so pode ligar `working`/`blocked`** sem prova e **so pode produzir `idle` junto com a tela nao contradizendo**; um relato falso de `idle` nao sobrepoe uma tela que mostra trabalho (regra 3). Conferir no teste.
- Do Codex entram so `status.type` e as marcas `activeFlags`; do hook, so o nome do evento e o painel. Nada disso sai alem de classes enumeradas (principio 8).
- O cliente do app-server **so le**; a lista fechada da secao 4 e coberta por teste com um servidor WebSocket falso que registra os metodos recebidos.

## 10. Instalacao (nao normativa; feita pelo Jabbur ou com o ok dele)
Hooks do Claude em `.claude/settings.local.json` da bancada (comando fixo, sem shell variavel): `UserPromptSubmit`, `PermissionRequest`, `Elicitation`, `Stop`, `StopFailure`, `SessionStart`, `SessionEnd`, cada um com `{"type":"command","command":"agentd report NOME","async":true}`. O `agentd` precisa estar no `PATH` (hoje nao esta instalado). O Codex nao precisa de instalacao: ele ja expoe o app-server.

## 11. Contrato testavel (testes independentes; `tmux` FALSO como na rev5; servidor WebSocket FALSO que registra os metodos; relatos enviados por `agentd report` com `TMUX_PANE` controlado)
- **S1** Codex: `idle` -> `active` -> `active+waitingOnApproval` -> `idle` publicam `idle`, `working`, `blocked/permission`, `idle` em menos de 1 s do evento, `source: app_server`. **S2** `waitingOnUserInput` => `blocked/user_input`. **S3** `notLoaded`, `systemError`, tipo desconhecido => `unknown`, nunca `idle`. **S4** lista fechada: o servidor falso so ve `initialize`, `initialized`, `thread/loaded/list`, `thread/read` (com `includeTurns:false`) e caminho `/`; nunca `thread/resume`, `turn/*`, `thread/queue/*`, `/daemon/shutdown`. **S5** queda do socket => `structured_lost`, o agente cai para a rev5 (tela), reconecta com recuo, volta. **S6** `thread_file` ausente/invalido/symlink/gravavel por outros => `structured_lost`, sem travar.
- **H1** Claude: `UserPromptSubmit` -> `working`; `Stop` -> `idle` (com a tela `unknown`, caso 2.1.302) em menos de 1 s. **H2** `PermissionRequest` => `blocked/permission`; `Elicitation` => `blocked/user_input`. **H3** relato de `pane` nao configurado => descartado e `reports.rejected` sobe; o estado nao muda. **H4** evento fora da lista => descartado. **H5** `agentd report` sai 0 e em menos de 1 s sem daemon, sem escrever na saida padrao e ignorando o stdin (sentinela no stdin nunca aparece em `agent.*`/`status`/eventos). **H6** `SessionEnd` + painel ausente => `gone`.
- **F1** relato `idle` com a tela mostrando `working` (regra casa): publica `working`, marca `diverged` depois de `diverge_after_ms`. **F2** relato `working` e tela `idle` positiva: continua `working` ate `override_after_s`, entao `idle` com `screen_override`; com a tela `unknown`: continua `working` indefinidamente. **F3** `blocked` nunca sofre override. **F4** relato falso de `idle` repetido nao tira o agente de `working` quando a tela mostra trabalho. **F5** sem estruturada ativa: comportamento identico ao da rev5 (regressao: toda a suite R da rev5 verde). **F6** `unmatched_ratio` sobe quando a tela nao casa e a estruturada esta ativa; `status` mostra, o estado nao muda.
- **P1** privacidade: sentinela no stdin do hook, no `activeFlags` inesperado e no nome de evento invalido nunca aparece em saida alguma. **P2** o daemon continua sem escrever no tmux (lista fechada da rev5, regressao R3).

## 12. Fora desta spec
Entrega de mensagens (o `wakerd` e o `chatd`); fonte para o Pi (sem sinal estruturado conhecido: continua so tela); detectar a versao do CLI do agente; hooks de `PostToolUse`/`SubagentStop` (a marca `background` continua da tela); reparo automatico das regras de tela (a rev6 so avisa pela razao `unmatched`).

## 13. Evidencias (regra 13: leitura de codigo ou medida; a documentacao e intencao)
- **Modo sombra 2026-10-16:** 87 amostras, `codex-blocks` `idle` correto; `claude` sempre `unknown,unmatched` em trabalho e em ocio. Causas lidas em `crates/agentd/rules/default.json` (`claude.working.footer` = `esc to interrupt` em `tail:8`; `claude.idle.box` exige `^\s*(⏸|⏵⏵) `, a linha real comeca por `-- INSERT --`; `tested_versions: ["2.1.301"]`, versao em uso 2.1.302).
- **Codex (fonte 0.163.0):** transmissao de `thread/status/changed` a todas as conexoes (`app-server/src/thread_status.rs:246`, `outgoing_message.rs:773-784`); `ThreadActiveFlag` com `WaitingOnApproval` e `WaitingOnUserInput` (`protocol/v2/thread.rs:1698-1701`); experimento: ouvinte passivo recebeu `active` (+6,9 s) e `idle` (+12,5 s) do `codex-blocks`. A pagina oficial do app-server documenta `thread/loaded/list`, `thread/status/changed` e `activeFlags` (leitura de 2026-10-16).
- **Claude Code (documentacao oficial dos hooks, leitura parcial 2026-10-16; nao testado):** 33 eventos, entre eles `UserPromptSubmit`, `PermissionRequest`, `Notification` (matchers `permission_prompt`, `idle_prompt`), `Stop` ("quando Claude termina de responder"; **nao dispara em interrupcao do usuario**), `StopFailure`, `SessionStart`, `SessionEnd`; `async`; variaveis do ambiente herdadas do processo pai. **A confirmar por experimento antes de implementar:** o JSON do stdin de `PermissionRequest`/`Notification` (trecho nao lido da pagina), que `UserPromptSubmit` tambem dispara em turnos iniciados pelo proprio Claude Code, e que `TMUX_PANE` chega ao hook (o ambiente da execucao de comandos tem `TMUX_PANE=%0`).
- **Objecoes da rev5 ao uso de hooks e como a rev6 responde:** (1) "nao cobrem Esc" => `screen_override`, so para sair de `working`; (2) "o aviso de bloqueio chega 6 s depois" => `PermissionRequest` dispara no pedido (nao `Notification`); (3) "permissao e pergunta sao indistinguiveis" => `PermissionRequest` x `Elicitation`, e no Codex `waitingOnApproval` x `waitingOnUserInput`.

## Historico
- rev6 (2026-10-16): primeira versao, a pedido do Jabbur.
