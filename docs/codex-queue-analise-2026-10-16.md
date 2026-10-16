# `codex queue`: o que o codigo do Codex 0.163.0 garante (leitura do fonte, 2026-10-16)

Fonte lido: `ref/codex` (clone raso da tag `rust-v0.163.0` de `github.com/openai/codex`, fora do git do blocks; ver `.git/info/exclude`). Pedido do Jabbur: "temos que parar de confiar em readme e documentacao e passar a ler o codigo" (AGENTS.md, regra 13). Caminhos abaixo relativos a `ref/codex/codex-rs/`. Contexto e consulta ao Sol: bancada `docs/consulta-codex-canal-sol-2026-10-16.md`.

## Cadeia, do comando ate a execucao
1. `cli/src/queue_cmd.rs:52` chama `codex_tui::run_session_queue_command`; a mensagem e obrigatoriamente nao vazia (`NonEmptyStringValueParser`) e imagens sao recusadas.
2. `tui/src/session_queue_commands.rs:27-80`: **recusa `--no-daemon`** (linha 32) e recusa enfileirar por um app-server embutido se ha daemon local rodando; resolve `--thread` como UUID ou como **nome exato** entre sessoes **ativas** (linhas ~96-108) e envia `ClientRequest::ThreadQueueAdd` (metodo `thread/queue/add`, linha 114).
3. `app-server/src/request_processors/thread_queue_processor.rs:78-95` (`add`): valida, exige a thread (carregada ou lida do armazenamento, **recusa arquivada**; **recusa efemera**, linha 263) e chama `service.enqueue`. **So enfileira; nao inicia turno.** Resposta: id do item na fila.
4. `ext/queue/src/lib.rs:1`: "Durable, storage-neutral user-message queue and idle dispatch". A fila e **duravel** (armazenamento da thread).
5. `ext/queue/src/service.rs:96-246` (`watch_external_messages`): a cada **10 s** (linha 96) confere a versao do banco; para cada thread **carregada** com fila alterada, abre uma tarefa de despacho que repete a cada 10 s (linha 239) enquanto a fila nao esvazia. **Nao despacha** se `agent_status` e `Running`, `Interrupted`, `Shutdown` ou `NotFound` (linhas 218-225). Caso contrario chama `wake_if_loaded` (linhas 472-483), que emite o evento de ociosidade (exceto se `Interrupted`).
6. O despacho vira `start_if_idle` (`core/src/session/turn_input.rs:461-`): sob o mesmo trinco, se ha **turno ativo** (`active_turn.is_some()`, linha 519) devolve `NotSubmitted { NotIdle }` e **nao inicia**. Tambem recusa com mailbox de gatilho pendente e com o servidor drenando.

## Consequencias (leitura do codigo; o que NAO foi provado esta marcado)
- **A mensagem enfileirada inicia um turno NOVO quando a thread esta ociosa; nao entra no meio de um turno, nao responde aprovacao, nao usa `turn/steer`.** Um dialogo de aprovacao e parte de um turno ativo, entao a fila espera o turno acabar (inferencia direta de 5 e 6; **confirmar em experimento**).
- **Atraso de ate ~10 s** entre `queue add` e o inicio (intervalo do vigia). Aceitavel para lembrete.
- **Custo:** cada mensagem enfileirada que dispara um turno consome uma chamada de modelo. O `chatd` precisa de rearme/deduplicacao no lado dele (a v1.10 torna o rearme duravel).
- **Thread interrompida pelo usuario (`Interrupted`) nao recebe despacho automatico**; o vigia espera. Uma sessao que o dono interrompeu nao e "acordada" a forca.
- Falha clara: UUID/nome inexistente -> "No active session found"; arquivada -> instrui `codex unarchive`; sem daemon -> erro. Nada disso cai em teclado.
- **Seguranca do socket:** o diretorio de controle e criado `0o700` e o socket `0o600` (`app-server-transport/src/transport/unix_socket.rs:31,56,436`): so o mesmo UID. Qualquer processo do mesmo usuario que fale o protocolo pode enfileirar (e, pelo mesmo protocolo, muito mais que isso); o texto fixo nao reduz essa autoridade. Nada de TCP.
- **Nao verificado no codigo/por experimento:** (a) como a TUI trata um **rascunho** no compositor quando o turno enfileirado inicia (o rascunho e estado local da TUI; `ThreadQueueChanged` so troca o alvo, `tui/src/app/app_server_event_targets.rs:80`); (b) o comportamento real com aprovacao aberta; (c) coalescencia de varias mensagens iguais enfileiradas. Experimentos: ver a bancada, `docs/consulta-codex-canal-sol-2026-10-16.md`, secao 5 (adaptados: lab com `CODEX_HOME` proprio nao tem credencial; usar uma sessao Luna nova num tmux de laboratorio, cwd temporario).

## Encaixe no chatd (proposta, ainda sem codigo)
Canal opt-in `codex-queue` (ao lado de arquivo e barra do tmux): vinculo explicito destinatario -> UUID/nome da sessao; `codex queue --thread X --message "<texto fixo da v1.1>"` por `argv`, sem shell, com timeout e saida limitada; falha fechada; **nunca fallback para teclado**; timeout apos aceitar = resultado incerto (nao reenviar de imediato); registro de tentativa no log; testes automaticos com executavel falso. Substitui a campainha por digitacao para o Codex.

---
## Experimento na sessao real de teste (2026-10-16 15:30, sessao Luna aberta pelo Jabbur em `Projects/tmp`, thread `01a145f9-...`, ligada ao daemon compartilhado 0.163.0)
Ferramentas: `docs/experimentos-codex/ws_codex.py` (cliente WebSocket minimo, stdlib, so o caminho `/`; **nunca** `/daemon/shutdown`) e `ensaio_fila.py`. O estado da thread vem do protocolo (`thread/read` -> `status`: `{"type":"idle"}` ou `{"type":"active","activeFlags":[...]}`, flags `waitingOnApproval` e `waitingOnUserInput`), sem ler tela. `thread/loaded/list` achou a thread pelo `cwd`.

| caso | resultado medido |
| --- | --- |
| ocioso | `codex queue` voltou em **0,2 s** (rc 0, id do item); a thread ficou `active` em **0,2 s** (nao esperou o vigia de 10 s); turno completo em ~3 s; a mensagem chegou como **mensagem de usuario normal** e a resposta foi a pedida |
| ocupado (turno rodando `sleep 15`) | a 2a mensagem foi aceita (rc 0) e **esperou**: o turno A terminou inteiro (comando + `A-ok`) e so entao a B rodou, em turno proprio (`B-ok`); nada interrompido nem misturado |
| por nome exato (`--thread "Testar canal codex queue"`) | resolveu para a mesma thread e funcionou |
| nome inexistente | `Error: No active session found matching '...'` (sem cair em teclado) |
| aprovacao aberta | **nao reproduzido**: esta sessao nao chegou a pedir aprovacao (o `curl` falhou no sandbox e o agente respondeu `negado`). Coberto pelo codigo: aprovacao pendente e parte de um turno ativo e `start_if_idle` recusa com turno ativo (`turn_input.rs:519`), o mesmo mecanismo do caso ocupado |
| rascunho no compositor da TUI | **provado (15:40, painel herdr `w8:p1`, rascunho "Estou escrevendo algo muito importante e editando o texto..." sem Enter)**: o `codex queue` voltou em rc 0, a thread entrou em turno em 0,1 s e terminou em ~1 s com a resposta pedida; a tela depois mostra o **rascunho intacto** no compositor. A mensagem NAO espera o usuario terminar de digitar: o turno roda no meio da escrita. Nao e canal de teclado, entao nao corrompe o texto; o custo e um turno nao pedido naquele momento. O canal independe do multiplexador (a sessao estava no herdr, nao no tmux) |

Correcao de metodo: `thread/queue/list` exige a capacidade `experimentalApi` no `initialize` (`capabilities.experimentalApi = true`); sem ela devolve erro -32600, e a primeira rodada leu isso como "fila vazia" por engano. As conclusoes de ordem e tempo vem dos turnos (`thread/read` com `includeTurns`), nao da lista da fila.

---
## Ponta a ponta da v1.11 com o Codex real (2026-10-16 16:37, daemon `chatd` ISOLADO em diretorio temporario, sessao Luna de teste, thread `01a145f9-...`)
Config isolada: `codex_bin` real, `codex_versions: ["0.163.0"]`, destinatario `codex-test` com `delivery: codex-queue` e `thread_file`. `chatd send` registrou a mensagem as 16:37:46; o log do daemon isolado mostrou `bell_sent` as 16:37:47; a sessao iniciou o turno, tentou rodar `chatd inbox --take` (texto fixo) e avisou que lhe falta identidade do chatd (a sessao de teste nao tem). Rascunho do Jabbur no compositor intacto. O daemon vivo (pid 381001) nao foi tocado; o isolado foi encerrado e os temporarios removidos.
