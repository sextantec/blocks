# tmux: patch candidato para o agentd (nao adotado; reavaliar com dados)

Estado: **registrado, nao adotado** (decisao do Jabbur, 2026-10-16: "Siga com o tmux padrao, como se trata de um programa interativo, performance importa e o patch ainda nao esta descartado"). O agentd rev5 funciona no tmux padrao; este patch so otimiza. Reavaliar quando a medicao em modo sombra ou o perfil de CPU/latencia apontar custo.

## O que o patch faz (3 arquivos, ~30 linhas, nenhum protocolo ou comando novo)

Arquivo: [`patches/tmux-3.7c-pane-signals.patch`](patches/tmux-3.7c-pane-signals.patch), sobre a tag `3.7c` (commit `e476c1230b958df0cb12977517d24b3dc931375b`). Aplica limpo (`patch -p1`, verificado em copia). **NAO foi compilado**: a maquina nao tem `autoconf` nem `libevent-devel`, e instalar pacotes e do Jabbur. Antes de adotar, compilar e rodar `regress/` do tmux.

| Variavel de formato nova | Origem | Uso no agentd |
| --- | --- | --- |
| `#{pane_last_output}` (tempo, por painel) | `gettimeofday` em `window_pane_read_callback` (`window.c`), o ponto por onde passa toda saida do painel; campo `last_output_time` em `struct window_pane` (`tmux.h`) | silencio por painel sem consumir `%output`: no lugar de `window_activity`, que e da janela (`input.c:1034`) |
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
