# Medidas no tmux isolado: Claude Code 2.1.301 (Haiku 5.5), 2026-10-16

Laboratório: tmux 3.7c, servidor próprio (`-S /work/proj`, `-f /dev/null`), painel 200x50, diretório de trabalho temporário, hooks de laboratório por `--settings`. Amostrador a cada 0,5 s: `scratchpad/lab-tmux/{meta.log,frames/,hooks.log}` (113 telas distintas, 1248 amostras). Nada do Jabbur foi tocado. Só Claude; Codex ainda não.

## Estados provocados e o que se viu

| Estado | Tela (capture-pane -p -J) | Hooks | Metadados tmux |
| --- | --- | --- | --- |
| Diálogo de confiança na pasta | `Quick safety check: Is this a project you created or one you trust?` / `❯ No, exit` / `Yes, I trust this folder` / `Enter to confirm · Esc to cancel` | nenhum (SessionStart só depois) | título = nome do host; `pane_current_command=claude` |
| Ocioso (modo auto) | caixa `❯ Try "..."` entre duas réguas `─`, rodapé `⏵⏵ auto mode on (shift+tab to cycle) · ← for agents` | SessionStart | título `✳ Claude Code` |
| Ocioso (modo padrão) | idem, rodapé `⏸ manual mode on · ? for shortcuts · ← for agents` | SessionStart | idem |
| Trabalhando (geração) | caixa `❯ ` **continua visível e vazia**; rodapé ganha `· esc to interrupt`; ao fim aparece `✻ Cooked for 10s · done 6:03 AM` | UserPromptSubmit, depois Stop (10 s) | **título não mudou** (`✳ História do tmux` o tempo todo); `window_activity` avança enquanto há saída |
| Permissão de Bash | `Bash command` / comando / `Do you want to proceed?` / `❯ 1. Yes` ... `4. No` / `Esc to cancel · Tab to amend` | PreToolUse, PermissionRequest (imediato), Notification `permission_prompt` (**+6 s**) | sem mudança visível |
| Pergunta ao usuário (AskUserQuestion) | `☐ Cor` / `Qual cor você prefere?` / `❯ 1. Azul` ... `Enter to select · ↑/↓ to navigate · Esc to cancel` | PreToolUse, PermissionRequest, Notification `permission_prompt` (+6 s): **mesmos hooks da permissão** | idem |
| Esc no diálogo de permissão | volta à caixa vazia, `✻ Worked for 2s` | **nenhum** (sem Stop) | |
| Esc na pergunta | volta à caixa vazia | **nenhum** | |
| Esc durante geração (antes de sair texto) | o texto do pedido **volta para a caixa** (`❯ Escreva um ensaio...`), rodapé sem `esc to interrupt` | UserPromptSubmit **sem Stop** | |
| Rascunho humano | `❯ rascunho que o humano esta digitando`, rodapé `⏸ manual mode on` | nenhum | idêntico ao ocioso por metadados |
| Seletor `/model` | lista numerada, `❯ 5. Haiku 5.5 ✔`, `Enter to set as default · s to use this session only · Esc to cancel` | nenhum | |
| `/compact` (conversa curta) | `⎿  Compacted (ctrl+o to see full summary)` e caixa vazia | SessionStart `source=compact` | |
| `/exit` | processo termina; **sem `remain-on-exit` o painel e a sessão somem** | | servidor tmux encerra |
| Processo morto (teste com `sleep; exit 7`) | | | com `remain-on-exit on`: `pane_dead=1`, `pane_dead_status=7` |

## Conclusões firmes

1. **Hooks não cobrem interrupção nem cancelamento.** Esc em geração, em diálogo de permissão e em pergunta deixam `UserPromptSubmit` sem `Stop` e sem outro hook. Só hooks como fonte deixariam o agente `working` para sempre. (Confirma o abandono dos hooks pelo herdr.)
2. **Hook de bloqueio é atrasado (+6 s)** e a permissão e a pergunta ao usuário são indistinguíveis por hook (`notification_type=permission_prompt` nos dois; `PermissionRequest.tool_name` distingue: `Bash` vs `AskUserQuestion`).
3. **Título do painel não serve** nesta versão: igual em ocioso e trabalhando. As regras de título OSC do herdr/Luvus não se aplicam aqui.
4. **A caixa `❯` aparece também durante o trabalho e dentro de diálogos** (`❯ 1. Yes`, `❯ 1. Azul`). Presença de `❯` não indica ocioso.
5. **Sinais de tela que funcionaram nos estados vistos:** `esc to interrupt` no rodapé (working); `Do you want to proceed?` + `Esc to cancel` (permissão); `Enter to select · ↑/↓ to navigate · Esc to cancel` (pergunta); `Esc to cancel` + lista (seletor de modelo); `Quick safety check` + `Enter to confirm · Esc to cancel` (confiança). `strings` do binário falhou em achar `Tab to amend`, que está na tela: **só tela real vale como prova**.
6. **Metadados do tmux isolados não separam ocioso, trabalhando e bloqueado**: `pane_current_command=claude` e `tpgid` constantes; `window_activity` só indica saída recente. Servem para vivo/morto (`pane_dead`, `pane_dead_status` com `remain-on-exit on`) e para saber se o processo é o agente.
7. **Rascunho humano é visível só na tela** (texto depois de `❯ ` na caixa). Por isso a entrega segura de prompt precisa ler a caixa.

## Ainda sem medir (Claude)

Erro/limite de cota, diálogo de plano, compactação longa, subagente em segundo plano, tela estreita/resize, caixa com `capture-pane` sem `-J`, `-e` (atributos), cursor (`#{cursor_x},#{cursor_y}`), `monitor-silence`.

## Codex 0.162.1 (gpt-6-luna, `--no-daemon`, `-s read-only -a on-request`), mesmo laboratório

Autorizado pelo Jabbur (opção B, 2026-10-16): o próprio Codex escreveu 1 arquivo em `~/.codex/sessions`; eu não li `auth.json` nem editei `config.toml`. Subido com `--no-daemon` e sem variáveis `HERDR_*/CHATD_*/CLAUDE*/CODEX*` para não tocar o `app-server` compartilhado do Jabbur (intacto ao fim). Sem hooks (não mexi na configuração do Codex).

| Estado | Tela | Título do painel (`#{pane_title}`) |
| --- | --- | --- |
| Ocioso | `› Ask Codex to do anything` / `GPT-6-Luna high · <dir>` / `? for shortcuts`; banner de atualização é só um quadro, não bloqueia | `work-codex` (nome do diretório) |
| Enviou o pedido (0,5 s) | eco do pedido, **sem** indicador de trabalho ainda | sem spinner por ~0,5 s |
| Trabalhando, antes do texto | `• Working (0s • esc to interrupt)` acima da caixa `›` | **`⠼ Escreva ensaio sobre tmux \| work-codex`** (spinner braille na frente) |
| Trabalhando, texto em fluxo (~5+ s) | **o indicador `Working ... esc to interrupt` some**; só o texto `• ...` e a caixa `› Ask Codex to do anything` vazia: **a tela é idêntica à de ocioso** | **continua com spinner braille** |
| Fim do turno | caixa vazia | título sem spinner |
| Esc durante o trabalho | `■ Conversation interrupted - use /feedback if something went wrong` | spinner some |
| Aprovação de comando | `Would you like to run the following command?` / `› 1. Yes, proceed (y)` / `2. ...(p)` / `3. No, and tell Codex what to do differently (esc)` / `Press enter to confirm or esc to cancel` | **`[ ! ] Action Required \| Escreva ensaio sobre tmux \| work-codex`** |
| Esc na aprovação | `• Failed (exit 1) ...` e `■ Conversation interrupted` | volta ao normal |
| Rascunho humano | `› rascunho do humano` (visível só na tela) | sem diferença |
| `/model` | `Select Model and Effort`, linha selecionada com `›`, rodapé `enter select · esc back` (**sem** "esc to cancel") | sem diferença |
| Visualizador (Ctrl+T) | rodapé `Browsing transcript · ↑↓/jk scroll · ←→/hl prompts · ^t details · ↵ rewind · esc back` (**não** casa com as frases do herdr 0.9.5: `↑/↓ to scroll`, `q to quit`) | sem diferença |

Conclusões firmes do Codex:
1. **Só tela dá falso `idle` no Codex durante o fluxo do texto** (~5 s a cada resposta): sem `esc to interrupt`, a tela é igual à de ocioso. O **título do painel cobre exatamente essa janela** (spinner braille).
2. **Título do Codex é um sinal forte e confiável nos 3 estados vistos** (working = spinner; blocked = `[ ! ] Action Required`; idle = sem nenhum dos dois), incluindo o retorno após `Esc`. No Claude 2.1.301 o título **não** mudou. Cada agente precisa do seu conjunto de sinais.
3. O `›` aparece em linhas de seleção dentro de diálogos e do seletor de modelo; presença de `›` não prova ocioso.
4. Frases do visualizador de transcrição do Codex 0.162.1 diferem das do herdr 0.9.5: regras copiadas ficam velhas entre versões. As regras precisam ser dados (arquivo editável) e vir com testes de tela real.
5. Hooks do Codex: não medidos (exigem configuração em `~/.codex`, fora do que foi autorizado).

## Pi 1.3.0 (openai/gpt-6-luna, `--offline --thinking low --no-context-files --session-dir <lab>`), mesmo laboratório

Autorizado pelo Jabbur (Pendência 7, opção A, 2026-10-16). O Pi usa a credencial dele por conta própria; eu não li `~/.pi` nem chaves. Sessões apontadas para a pasta do laboratório. Sem permissão/aprovação de ferramentas por padrão (Pi roda ferramentas direto).

| Estado | Tela | Título do painel |
| --- | --- | --- |
| Primeira tela após atualizar | changelog longo ("What's new") empurra tudo; aviso `Warning: tmux extended-keys is off. Modified Enter keys may not work. Add set -g extended-keys on...` | `π - work-pi` (π + nome do diretório) |
| Ocioso | caixa vazia **entre duas réguas `─`**, sem texto; abaixo a linha do diretório e a linha de custo/contexto (`↑4.1k ↓577 $0.001 (sub) 1.7%/272k (auto)` e o modelo à direita) | `π - work-pi` |
| Trabalhando (geração e ferramenta) | borda superior da caixa vira `── ⠧ Working ───────...` (spinner braille na régua), **presente durante todo o turno, inclusive no fluxo do texto**; na ferramenta mostra `$ sleep 8; echo pronto (timeout 15s)` e `Elapsed 4.0s` | **não muda** (`π - work-pi`) |
| Fim do turno | régua volta a `─────` lisa; ferramenta mostra `Took 8.0s` | idem |
| Esc durante geração | `Operation aborted` abaixo do pedido; sem `Working` | idem |
| Rascunho humano | texto dentro da caixa entre as réguas | idem |
| `/model` | lista com `[provedor]`, `(1/446)`, `Model Name: GPT-6 Luna`, rodapé `Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel` | idem |

Conclusões firmes do Pi:
1. **Um único sinal de tela cobre o Pi inteiro:** linha de régua com `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ Working` = trabalhando; ausente = não trabalhando. Sem janela de falso `idle` como a do Codex.
2. O título do Pi **não** distingue estados (igual ao Claude).
3. **Sem diálogo de permissão** por padrão: `blocked` do Pi só vem de seletores/extensões (ex.: `/model`) ou de instalações com extensões de aprovação; não medido.
4. O manifesto do herdr 0.9.5 para o Pi (`^── [⠋…⠏] Working ─+$`) **casou com a tela real do Pi 1.3.0**.
5. Aviso útil para a configuração do tmux da migração: **`set -g extended-keys on`**, senão o Pi avisa que teclas modificadoras podem falhar.

# Rodada B (2026-10-16): medidas extras com amostragem fina (0,15 s), mesmo laboratório

Harness: `scratchpad/lab-tmux/lib.sh`, `sampler2.sh`, `analyse.py`, `analyse-pi.py`; runs em `scratchpad/lab-tmux/run-cl1`, `run-cx1`, `run-pi1` (telas distintas em `frames/`, 1 linha por amostra em `meta.log`, fase em `ph=`). Claude 2.1.301 (252 telas, 1.472 amostras), Codex 0.162.1 (77 telas), Pi 1.3.0.

## Claude Code 2.1.301
1. **Mensagem digitada durante o trabalho entra numa fila**: a tela mostra `ctrl+x ctrl+s to send now` e a caixa vira `❯ Press up to edit queued messages`; o rodapé mantém `esc to interrupt`. Hooks: novo `UserPromptSubmit` ao enfileirar e outro ao ser consumida; um `Stop` por turno.
2. **Sugestão fantasma na caixa** (ocioso, depois de um turno): `❯ ls | wc -l` aparece na caixa **sem o humano ter digitado nada**. Com `capture-pane -e` ela vem em atributo dim (`ESC[2m ... ESC[0m`), o rascunho humano vem em atributo normal. `cursor_x` fica **no início** da caixa (x=2) na sugestão e **depois do texto** no rascunho (x=14 para `texto humano`). `Tab` na caixa vazia aceita a sugestão; digitar qualquer tecla a apaga. **Consequência: "caixa vazia" não pode ser "sem texto depois de `❯`" em texto plano; precisa de `-e` (dim) ou do cursor.**
3. **Menu de slash** (`/mo`): lista abaixo da caixa; caixa mostra `❯ /mo`; rodapé some o `? for shortcuts`. É rascunho humano.
4. **Resize** (100x30, 60x20): a tela reflui; o rodapé e a régua permanecem; `esc to interrupt` permanece no rodapé em 60 colunas; sem perda dos sinais. Saída longa (80 linhas): `alternate_on=1`, `history_size=0`: o Claude usa tela alternativa, o `capture-pane` só vê a janela visível (sem `-S` útil).
5. **Diálogo de escrita (Write)**: `Create file` / diff / `Do you want to create ola.txt?` / `❯ 1. Yes` / `Esc to cancel · Tab to amend`; hooks PreToolUse + PermissionRequest (tool `Write`), Notification +6 s. Mesma família do Bash.
6. **Ciclo de modos** (Shift+Tab): rodapés `⏸ manual mode on`, `⏸ plan mode on`, `⏵⏵ auto mode on`, todos com `(shift+tab to cycle)` e `· ← for agents`. O modo muda o rodapé, não o estado.
7. **Subagente em segundo plano**: o agente principal fica **sem `esc to interrupt`** (caixa livre, `✻ Waiting for 1 background agent to finish`) enquanto o subagente roda; abaixo da caixa aparece uma lista (`● main` / `◯ general-purpose  <tarefa>`) e o rodapé ganha `· ↓ to manage`/`/tasks to see subagents`. Hooks: SubagentStart, SubagentStop (vários, inclusive depois do `Stop`). **Principal "ocioso" com trabalho pendente em segundo plano**: a regra deve poder marcar isso (marca, não estado).
8. **Aprovação de plano (ExitPlanMode)**: `Claude has written up a plan and is ready to execute. Would you like to proceed?` / `❯ 1. Yes, and use auto mode` / `2. Yes, manually approve edits` / `3. Tell Claude what to change`; rodapé do diálogo `ctrl+g to edit in Nvim · ~/.claude/plans/<nome>.md`. **NÃO tem `Esc to cancel`**: a regra de bloqueio por "Esc to cancel" **perde este diálogo**. Hooks: PreToolUse + PermissionRequest com `tool_name=ExitPlanMode`. Esc devolve a caixa sem hook. O Claude gravou um arquivo de plano em `~/.claude/plans/` por conta própria (comportamento do produto, no modo plano).
9. Notification `idle_prompt` chega ~60 s depois do ocioso (hooks.log), útil só como confirmação tardia.

## Codex 0.162.1
1. **Janela do falso idle medida:** em um turno de 15 s, o título tem spinner braille de +0,5 s a +15,5 s; a tela tem `esc to interrupt` só de +0,5 s a +13,7 s: **11 de 101 amostras (≈1,8 s) com spinner no título e sem sinal na tela**. O título cobre toda a janela; **tela ociosa é estável** (1 hash em 6 s de silêncio): sem relógio nem animação no estado ocioso.
2. **O título pisca**: `[ ! ] Action Required | …` alterna com `[ . ] Action Required | …` (visto os dois). A regra deve casar `\[ [!.] \] Action Required`, não só `[ ! ]`.
3. **Título não segue o tópico do turno seguinte** (ficou `Escreva ensaio sobre pipes Unix | work-cx1` depois de novos pedidos); só o prefixo importa (spinner, `Action Required`).
4. **Largura 60x20:** `Working (2s • esc to interrupt)` e a caixa mantidos; caminho do diretório truncado com `…`; título intacto. Mensagem digitada durante o trabalho aparece como `› mensagem` acima do `Working` (o Codex a incorpora ao turno); título segue com spinner.
5. **Aprovação:** título muda para `[ . ] Action Required` ~0,5 s depois do diálogo na tela; transição working→blocked com 1 amostra de tela sem sinal (3,5 s): regras por **tela de diálogo** (`Would you like to run the following command?` + `Press enter to confirm or esc to cancel`) têm que vencer as de `Working`.
6. **Placeholder vs rascunho:** `Ask Codex to do anything` é dim (`ESC[2m`), o rascunho é atributo normal; `cursor_x` = 2 no placeholder, 10 com `rascunho`, 7 depois de 3x seta esquerda (**cursor movido para dentro do texto: o cursor sozinho não prova caixa vazia**; o dim sim). Multilinha (Shift+Enter) vira duas linhas `› linha1` / `  linha2`.
7. Aviso de atualização na partida (`Update available 0.162.1 → 0.163.0`, `Update now` / `Skip`): é um **diálogo bloqueante** com `enter continue · esc skip`; `Esc` pula.

## Pi 1.3.0
1. **`Working` contínuo**: 82 amostras em turno de 11 s, `Working` presente de +0,5 s a +11,4 s sem lacuna; também em 50x20. Fila: mensagem durante o trabalho vira `Steering: <texto>` + `↳ Alt+Up to edit all queued messages` acima da régua.
2. **Rascunho:** texto direto entre as duas réguas, sem prefixo; cursor em bloco invertido (`ESC[7m `) depois do texto, `cursor_x` no fim. Caixa vazia = nada entre as réguas.
3. **Esc** durante geração: volta ao ocioso (`Operation aborted`).
4. O Pi carrega a extensão `herdr-agent-state.ts` do Jabbur na partida; sem variáveis `HERDR_*` ela ficou inerte (nenhum processo do herdr tocado).
5. Largura 50x20: rodapé truncado (`...`), régua e `Working` mantidos.

## Conclusões para a rev5 (acréscimos à lista anterior)
- **"Caixa ocupada" exige atributos**: sugestão (Claude) e placeholder (Codex) são texto dim; só o rascunho humano é atributo normal. `capture-pane -p -e` é necessário para a entrega segura de prompt (a fonte de estado pode usar texto plano).
- Claude `blocked` precisa de **mais de uma frase** (`Esc to cancel` não aparece no diálogo de plano): lista de regras por diálogo, cada uma com sua tela.
- **Marca `background`** (subagente ativo com principal livre) no Claude; o estado do agente principal continua `idle`.
- Cursor e fila: mensagem enfileirada não é rascunho; ter regra para `Press up to edit queued messages` (Claude), `Steering:` (Pi).
- Tela ociosa estável no Codex e no Pi; no Claude o rodapé muda por modo, mas não por tempo.


## Eventos do modo de controle (2026-10-16 tarde)
Cliente de teste em Python (`tmux -C attach -f ignore-size`, stdin so com `refresh-client -B` e `capture-pane`), 1 turno por agente em pasta ja confiavel, debounce 150 ms. Resumo e numeros: secao 4A da `spec-rev5.md`. Detalhes que nao cabem la:

| Agente | Boot | Ocioso 45 s | Turno (prompt "crie ola.txt") | Pico |
|---|---|---|---|---|
| Claude | 11 eventos, 2 KB | 3 eventos, 439 B (6,8 s, 14,8 s, 22,8 s: troca do `Try "..."`) | 34 eventos, 5,9 KB em 3,6 s; dialogo `Do you want to create ola.txt?` visto 4,04 s depois do prompt | 3,4 KB/s |
| Codex | 38 eventos, 20 KB | 0 | 226 eventos, 10,7 KB em 3,5 s (titulo com spinner: 52 OSC 0/2); aprovacao vista 3,9 s depois | 11,2 KB/s |
| Pi | 14 eventos, 6,6 KB | 0 | 96 eventos, 67 KB em 7,2 s (resposta sem ferramenta) | 9,4 KB/s |

- Assinatura de formato (`refresh-client -B`, no maximo 1 por segundo) entregou titulo, comando e `pane_pb_state`; no Codex o titulo com spinner ora aparece, ora nao (a subscricao amostra a 1 s e perderia a lacuna; o ciclo com `list-panes` apos `%output` e mais fiel).
- Armadilha: cliente anexado com `-r` => `send-keys` de outro processo falha com `client is read-only` (o tmux escolhe o ultimo cliente da sessao como alvo). Com `-f ignore-size` funciona e a janela mantem o tamanho.
- Cliente `-r` executou `set-option` e `kill-pane` (so teclas e troca/anexo/desanexo sao bloqueados; `CMD_READONLY` no codigo).
- Variavel de formato desconhecida devolve vazio; `synchronized_output_flag` existe (valor 0 nas amostras).
- Limitacoes: um turno por agente, Haiku/Luna, tmux local, laco de 50 ms do cliente de teste soma ao debounce; CPU do cliente nao medida.
