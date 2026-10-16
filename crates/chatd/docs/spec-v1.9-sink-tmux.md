# chatd v1.9: sink `tmux` do `stale` (Parte A) e portao do agentd na campainha tmux (Parte B)

Origem: decisao do Jabbur (2026-10-15): canal de aviso `tmux`, depois do `agentd`; **Parte A aprovada em 2026-10-16** ("Esta aprovado"). **Parte B: PROPOSTA, nao implementar antes da aprovacao do Jabbur** (muda o daemon e o comportamento da campainha tmux). Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes pelo Haiku. Emenda a regra 9 da v1.8 (sink) e a campainha da v1.1/v1.3.

## Problema (exemplo)
Hoje o `chatd stale` detecta `[chat] parado: claude 7 pendente(s), mais antiga 329 min` e escreve num arquivo que ninguem le; a RESPOSTA do `codex-blocks` chegou e o `Monitor` nao a entregou, e so foi vista por acaso. Alem disso, a campainha em painel **tmux** (`bell.rs`, `send_prompt`, ramo `Mux::Tmux`) digita `send-keys` **sem checar se o agente esta ocioso**, ao contrario do ramo herdr: poderia digitar no meio de um turno ou por cima de um rascunho do Jabbur.

## Parte A: sink `tmux` (cliente puro; nao altera o daemon)
1. **`--notify` pode repetir** (ate 3, distintos): `stdout` (padrao se nenhum), `file:CAMINHO`, `tmux`. Repetido, quarto sink ou desconhecido = saida 2. Opcoes novas, validas **so com** o sink `tmux` (senao saida 2): `--tmux-bin ARQ` (padrao `tmux`) e `--tmux-socket CAMINHO` (vai como `-S CAMINHO` antes do subcomando).
2. **A cada execucao** (haja ou nao aviso novo), o sink `tmux` chama, nesta ordem, por `Command` com `argv`, **sem shell**, 5 s por chamada e 10 s no total (estourou: pula o resto, `timeout`), saida limitada a 64 KiB:
   1. `tmux set-option -g @chatd_stale -- TEXTO`. `TEXTO` = entradas dos destinatarios **parados agora** (todos os episodios ativos, nao so os novos), na ordem do nome, separadas por ` | `: `NOME Np Mm` (parado: N pendentes, M minutos) ou `NOME Nr Mm` (sem resultado: N pedidos). Vazio (`""`) se nenhum. No maximo 200 caracteres; se passar, corta em fronteira de entrada e acrescenta ` +K` (K entradas omitidas). Nome fora de `[a-z0-9_-]{1,32}` e ignorado e contado em `skipped`.
   2. So se houve aviso **novo** nesta execucao: `tmux list-clients -F '#{client_name}'`; cada nome valido por `^[A-Za-z0-9/_.:@-]{1,64}$`, no maximo 8 clientes; para cada um `tmux display-message -c CLIENTE -d 10000 -l -- LINHA`, onde `LINHA` e a linha de texto fixo da regra 6 da v1.8 (`-l` = literal, sem expansao de `#`, verificado no fonte do tmux 3.7c).
3. **Subcomandos permitidos do tmux: so `set-option`, `list-clients`, `display-message`.** Nunca `send-keys`, `run-shell`, `new-*`, `start-server`. Se nao ha servidor, o tmux sai com erro e o `stale` nao o inicia.
4. **Falha do sink nao perde o aviso nem muda o codigo de saida** (0/6 da regra 4): escreve em stderr `chatd: aviso: sink tmux: MOTIVO` (`spawn_failed|timeout|exit_nonzero|bad_output`) e os demais sinks rodam normalmente.
5. **Privacidade:** o texto vem so do nome, de contagens e de minutos; nunca remetente, assunto ou corpo (S6). A deduplicacao (stamp, janela C12) nao muda: o `display-message` sai uma vez por episodio; o `@chatd_stale` e reescrito sempre (idempotente; limpa sozinho quando ninguem esta parado).
6. **Instalacao (nao normativa; feita pelo Jabbur):** acrescentar `--notify tmux` a unidade `chatd-stale.service` e, no `tmux.conf`, algo como `set -g status-right '#{?@chatd_stale,#[fg=red]#{@chatd_stale} ,}%H:%M'`. O binario novo e instalado pelo Jabbur; o daemon **nao** reinicia por causa da Parte A.

### Contrato testavel (A; `tmux` FALSO que registra cada chamada)
- **K1** nada parado: `set-option` com valor vazio; nenhum `display-message`; saida 0. **K2** um parado (`claude`, 7, 329 min): valor exato `claude 7p 329m`; um `display-message -c C -d 10000 -l --` por cliente com a linha exata; saida 6. **K3** segunda execucao no mesmo episodio: `set-option` de novo com o mesmo valor, nenhum `display-message`, saida 0. **K4** dois parados: ordem por nome e ` | `. **K5** texto acima de 200: corta em fronteira e termina em ` +K`. **K6** nomes de cliente invalidos ignorados; 9 clientes = 8 chamadas. **K7** assunto/corpo com `#{`, `#(` e `;` no log nunca aparecem em nenhum argumento (varrer o registro do falso). **K8** `tmux` inexistente: aviso `spawn_failed`, saida inalterada, sink `file:` gravou. **K9** `tmux` sai nao zero: idem `exit_nonzero`. **K10** `tmux` trava: `timeout`, execucao total < 12 s. **K11** `--notify file:X --notify tmux`: ambos agem; 4 sinks, duplicado ou `--tmux-bin` sem o sink: saida 2. **K12** propriedade: o registro do falso so contem `set-option`, `list-clients`, `display-message`. **K13** `--tmux-socket P`: todas as chamadas levam `-S P` antes do subcomando. **K14** `chatd stale` sem `tmux` nao muda nada (regressao S1 a S11 e v1.8).

### Esclarecimentos da Parte A (testes independentes, 2026-10-16)
1. **Linha do `display-message`** = exatamente a linha de texto fixo que o `stale` imprime para aquele episodio (v1.8 regra 6), inclusive `campainha K vez(es)` quando for o caso.
2. **Apos `spawn_failed`** o sink `tmux` **para**: nao tenta as chamadas seguintes (so `timeout` tambem pula o resto; `exit_nonzero` de uma chamada nao impede as demais, exceto que `list-clients` falho dispensa os `display-message`).
3. **Aviso de falha:** uma linha `chatd: aviso: sink tmux: MOTIVO` **por execucao** e por motivo (a primeira falha que parou ou o primeiro `exit_nonzero`), nao por chamada.
4. **Uma linha de `display-message` por cliente e por aviso novo**; avisos novos diferentes geram linhas diferentes para o mesmo cliente.
5. **` +K`** conta entradas omitidas, nao caracteres.
6. **Trava:** limite de 5 s por chamada e 10 s no total; o teste aceita < 12 s.
7. **S9 da v1.7 e revogado:** `--notify tmux` deixa de sair 2; o teste `s9_notify_tmux_sai_2_e_nao_executa_tmux` foi removido (cobertura em K1 a K14); `s9b` (sink fora da lista = 2) continua.
9. **Nome de cliente** alem da regex nao pode ter segmento `..` (separado por `/`): endurecimento aceito (K6b).
10. **`stdout`** so e padrao sem `--notify`; com `--notify tmux` sozinho nada vai a stdout (K2 pede os dois sinks para comparar a linha).
11. **`due_expired`** (prazo vencido) aparece no `@chatd_stale` como `NOME Nr Mm`.
8. **`--tmux-bin`/`--tmux-socket` sem o sink `tmux`** saem 2 (K11d/K11e); a prova de que as opcoes existem quando o sink esta ligado e K11a/K13.

## Parte B (PROPOSTA): portao do `agentd` na campainha tmux (muda o daemon)
1. **Pre-requisito no `agentd` (rev5.4, aditivo):** `Agente` ganha `pane` (id do painel observado, ex.: `%7`, ou `null`). Backlog do agentd.
2. **Config** do `chatd` (campo desconhecido segue recusado): `"agentd": {"bin": "agentd", "socket": "/abs"}` e, por destinatario, `"agentd_id"` opcional (padrao: o nome do destinatario). `"bell_tmux_dry_run"` (padrao **true**).
3. **Regra** (so para `Mux::Tmux`; herdr inalterado): antes do `send-keys`, o daemon roda `agentd get ID --json --socket S` (`argv`, sem shell, 5 s, 64 KiB) e so digita se **todas**: `state == "idle"`; `input == "empty"`; `origin` em `configured|mark`; sem marcas `stale`, `error`, `idle_unconfirmed`; e `pane` igual ao painel **ligado e verificado** por `chatd bind` (a ligacao prova o dono pelo `pid`; o `agentd` prova o estado do mesmo painel).
4. **Falha fechada:** sem a secao `agentd`, `agentd` ausente, timeout, saida invalida, `unknown_agent` ou `pane` diferente = **nao digita** e registra `bell_failed` com `reason` = `agentd_not_configured|agentd_unavailable|agentd_bad_output|unknown_agent|pane_mismatch` (tipo de evento ja existente; **nenhum tipo novo**, para nao travar a volta de versao). Estado diferente de `idle`/`empty` = nao digita, sem evento, como o herdr (a campainha tenta no proximo ciclo).
5. **`bell_tmux_dry_run: true`:** com o portao aberto, nao digita; escreve em stderr do daemon `chatd: bell tmux dry-run: NOME portao=ok`. O Jabbur liga `false` depois do modo sombra do `agentd`.
6. O texto digitado continua o fixo da v1.1 (`[chat] N pendente(s). Rode: chatd inbox --take`); nunca remetente, assunto ou corpo.
7. **Mudanca de comportamento (declarada):** campainha tmux **deixa de digitar** em quem nao tem `agentd` configurado. Hoje ninguem depende disso (os agentes estao no herdr).
8. **Instalacao:** binario novo do `chatd` e reinicio do `chatd.service` sao do Jabbur; a restauracao do backup e o ponto sem volta so existem se um tipo de evento novo for gravado (esta parte nao grava).

### Contrato testavel (B; `tmux` e `agentd` FALSOS)
- **P1** idle + empty + `configured` + `pane` igual + dry_run false: `send-keys -l TEXTO` e `Enter`, uma vez. **P2** `working`/`blocked`: nada. **P3** `input: draft`: nada. **P4** `input: unknown` ou `null`: nada. **P5** `origin: discovered`: nada. **P6** marca `stale`/`error`/`idle_unconfirmed`: nada. **P7** `pane` diferente: nada e `bell_failed pane_mismatch`. **P8** `agentd` ausente, trava (5 s), saida invalida, `unknown_agent`: nada e o `reason` certo. **P9** sem a secao `agentd`: nada e `agentd_not_configured`. **P10** dry_run padrao: portao aberto, nenhum `send-keys`, linha em stderr. **P11** herdr: comportamento da v1.1 inalterado (regressao). **P12** o `argv` do falso so tem `get ID --json --socket S`; `ID` vem de `agentd_id` validado por `[a-z0-9_-]{1,32}`. **P13** nenhum evento de tipo novo no log em todos os casos.

## Fora desta versao
Acordar o Claude por `claude --resume`; despertar do `codex` por outros caminhos; barra de status do `agentd` (sera outro consumidor do `@chatd_stale`/agentd).
