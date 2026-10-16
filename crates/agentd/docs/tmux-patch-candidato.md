# tmux: patch candidato para o agentd (nao adotado; reavaliar com dados)

Estado: **registrado, nao adotado** (decisao do Jabbur, 2026-10-16: "Siga com o tmux padrao, como se trata de um programa interativo, performance importa e o patch ainda nao esta descartado"). O agentd rev5 funciona no tmux padrao; este patch so otimiza. Reavaliar quando a medicao em modo sombra ou o perfil de CPU/latencia apontar custo.

## O que o patch faz (3 arquivos, ~30 linhas, nenhum protocolo ou comando novo)

Arquivo: [`patches/tmux-3.7c-pane-signals.patch`](patches/tmux-3.7c-pane-signals.patch), sobre a tag `3.7c` (commit `e476c1230b958df0cb12977517d24b3dc931375b`). Aplica limpo (`patch -p1`, verificado em copia). **Compilado e testado em 2026-10-16** (apos o Jabbur instalar `autoconf` e `libevent-devel`): build na copia `scratchpad/tmux-patch/build` (`sh autogen.sh && ./configure && make`), sem instalar; servidor isolado em socket proprio. Resultado: `cursor_attrs` = `none` no inicio, `dim` com o cursor no inicio de texto esmaecido (`x=2`), `none` depois de texto normal; `pane_last_output` vazio antes da primeira saida e atualizado a cada saida; `list-panes -a -F` aceita as duas. **Com os agentes reais** (laboratorio isolado, tmux com patch em socket proprio, 2026-10-16; Claude 2.1.301/Haiku 5.5, Codex 0.162.1/gpt-6-luna, Pi 1.3.0), `cursor_attrs|cursor_x` com a caixa vazia, com rascunho `abc` e depois de `C-u`:

| Agente | Caixa vazia (placeholder) | Rascunho `abc` | Depois de `C-u` |
|---|---|---|---|
| Claude | `dim`, x=2 | `none`, x=5 | `dim`, x=2 |
| Codex | `dim`, x=2 | `none`, x=5 | `dim`, x=2 |
| Pi | `reverse`, x=0 | `reverse`, x=3 | `reverse`, x=0 |

Claude e Codex: o atributo sozinho separa placeholder (`dim`) de rascunho (`none`). Pi desenha o proprio cursor como celula invertida, entao o atributo e sempre `reverse`; ali o sinal e `cursor_x` (0 vazio, >0 rascunho; o Pi nao tem prefixo). Um turno e uma sessao por agente; sugestao fantasma do Claude (alem do placeholder `Try "..."`) nao reproduzida aqui. **Nao feito:** `regress/` do tmux e comparacao de custo.

| Variavel de formato nova | Origem | Uso no agentd |
| --- | --- | --- |
| `#{pane_last_output}` (tempo, por painel; **em segundos Unix**, como `window_activity`: medido, 1792152491 e nao ms) | `gettimeofday` em `window_pane_read_callback` (`window.c`), o ponto por onde passa toda saida do painel; campo `last_output_time` em `struct window_pane` (`tmux.h`) | silencio por painel sem consumir `%output`, mas com resolucao de **1 s**: serve ao fallback (no lugar de `window_activity`, que e da janela, `input.c:1034`), nao substitui a quietude em ms do `%output` |
| `#{cursor_attrs}` (ex.: `dim`, `none`, `dim,reverse`) | `grid_view_get_cell` na posicao do cursor, `attributes_tostring` (`format.c`), copia de `format_cb_cursor_character` | distinguir sugestao fantasma de rascunho sem `capture-pane -e`: na sugestao do Claude o cursor fica no inicio do texto dim (medido: `x=2`, `ESC[2m`); em rascunho o cursor fica depois do texto |

Combinadas com uma subscricao (`refresh-client -B`), as duas variaveis dariam, sem capturar a tela, o par "painel quieto ha N ms" + "caixa vazia ou com sugestao". **Nao verificado**: se o cursor do Codex fica no inicio do placeholder (o Claude fica).

## Por que nao agora

- O `%output` do modo de controle ja da a atividade por painel, e o `capture-pane -e` ja da o atributo; o patch troca chamadas por variaveis, nao cria informacao nova.
- O que e dificil nao muda com o patch: a janela de ~0,5 s depois do Enter no Claude (tela igual a de rascunho) e as frases de dialogo que mudam a cada versao dos agentes.
- Custo: binario proprio ao lado do `/usr/bin/tmux` (instalacao e do Jabbur; dois tmux no mesmo socket recusam-se por versao de protocolo), reaplicar a cada versao, e o `blocks` publico passaria a exigir um fork.

## Quando reavaliar

1. O `agentd` real (Rust) mostrar custo relevante de CPU ou de captura. **Linha de base medida** (secao 4A da `spec-rev5.md`): latencia de deteccao ~0,17 s, `capture-pane` 0,4 a 1 ms, servidor tmux 0,1 a 0,2 % de um nucleo, ocioso silencioso em Codex e Pi; o patch economizaria so a captura de telas sujas.
2. O modo sombra no sabado mostrar latencia de deteccao acima do aceitavel por causa da captura.
3. O tmux do upstream trouxer algo equivalente (checar `CHANGES` a cada versao).

Se adotado: enviar ao upstream como proposta isolada (as duas variaveis sao candidatas realistas), manter o agentd funcionando em tmux padrao (cair para `%output` e `capture-pane -e` quando a variavel nao existir: `list-panes -F` devolve vazio para formato desconhecido; testar).

Fonte consultada: `ref/tmux` (nao comitado; `ref/` esta em `.git/info/exclude`), lida com `ripwire`: `window_pane_read_callback` (`window.c:1139`) -> `control_write_output` e `input_parse_pane`; `input_parse_buffer` (`input.c`) chama `window_update_activity`.
