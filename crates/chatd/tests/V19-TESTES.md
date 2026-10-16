# v1.9 Parte A (sink `tmux` do `stale`): testes independentes

Arquivo: `tests/v19_sink_tmux.rs` (K1 a K14). Contrato: `docs/spec-v1.9-sink-tmux.md`, Parte A. A Parte B (portao
do agentd) e proposta e nao e testada aqui. Os testes usam um `tmux` falso (script Python 3 `-I`, em diretorio
temporario) que grava cada argv em JSON Lines. Nenhum tmux real, nenhum agente real, nenhuma rede.

## Cobertura

| Id | O que prova | Resultado hoje |
|---|---|---|
| K1 | nada parado: `set-option` com valor vazio, nenhum `display-message`, saida 0 | falha (`--tmux-bin` e `--notify tmux` ainda nao existem: saida 2) |
| K2 | um parado: valor exato `claude 7p 329m`; `list-clients`; um `display-message -c C -d 10000 -l --` com a linha exata; saida 6 | falha (mesma causa) |
| K3 | segunda execucao no mesmo episodio: `set-option` de novo, sem `display-message`, saida 0 | falha (mesma causa) |
| K4 | dois parados: ordem por nome e ` \| ` | falha (mesma causa) |
| K5 | texto acima de 200: <= 200 caracteres, entradas inteiras, cauda ` +K`, mostradas + K = total | falha (mesma causa) |
| K6a | nove clientes validos geram oito `display-message` | falha (mesma causa) |
| K6b | nomes de cliente invalidos nao recebem chamada | falha (mesma causa) |
| K7 | assunto e corpo com `#{`, `#(`, `;` e uma sentinela nunca aparecem em argumento | falha (sem chamadas: sink ausente) |
| K8 | `tmux` inexistente: `spawn_failed` em stderr, saida igual ao controle (so `file:`), arquivo gravado | falha (opcao/sink ausentes) |
| K9 | `tmux` sai 1: `exit_nonzero`, idem K8 | falha (mesma causa) |
| K10 | `tmux` trava: `timeout` em stderr, execucao < 12 s (o falso dorme 30 s) | falha (mesma causa); leva ~5 s quando o sink existir |
| K11a | `file:` e `tmux` agem juntos | falha (mesma causa) |
| K11b | quatro sinks: saida 2; controle: tres sinks distintos saem 6 | falha: o controle falha ("--notify repetido" hoje) |
| K11c | sink repetido: saida 2; controle: dois sinks distintos saem 6 | falha: o controle falha ("--notify repetido" hoje) |
| K11d | `--tmux-bin` sem o sink `tmux`: saida 2; controle: `file:` sozinho sai 6 | **passa**, mas por acidente: `--tmux-bin` e opcao desconhecida hoje (saida 2) |
| K11e | `--tmux-socket` sem o sink `tmux`: saida 2 | **passa**, por acidente (mesma razao de K11d) |
| K12 | so `set-option`, `list-clients` e `display-message` sao chamados | falha (sem chamadas: sink ausente) |
| K13 | `--tmux-socket P`: toda chamada leva `-S P` antes do subcomando | falha (mesma causa) |
| K14a | sem pendencia, sem `--notify tmux`: saida 0, sem linha, nenhum `tmux` no PATH e chamado | **passa** (regressao) |
| K14b | parado sem `--notify tmux`: saida 6 com a linha de texto fixo; nenhum `tmux` chamado | **passa** (regressao) |
| K14c | segunda execucao no mesmo episodio (com `file:`) sai 0 e nao repete a linha | **passa** (regressao) |

Resultado da rodada final: 5 passam (K11d, K11e, K14a, K14b, K14c), 16 falham. Os falhos saem por opcao ausente
(`--tmux-bin`, `--tmux-socket`) ou por `--notify` repetido (multiplos sinks), que sao o comportamento novo.

## Ambiguidades (marcadas `AMBIGUO` no arquivo ou so afirmadas no garantido)

1. **Campainha no texto da linha**: a linha do `display-message` e a linha de texto fixo da v1.8 (regra 6). A spec
   nao fixa se leva `campainha K vez(es)`. O K2 compara com a linha que o proprio `stale` imprime, e so exige o
   prefixo `[chat] parado: claude 7 pendente(s), mais antiga 329 min`.
2. **Apos falha de spawn**: a spec so manda pular o resto no timeout. Nao diz se as chamadas seguintes sao tentadas.
   K8 nao afirma o numero de chamadas.
3. **Aviso uma vez ou por chamada**: a spec diz "um `chatd: aviso: sink tmux: MOTIVO`", mas nao fixa se repete. Os
   testes exigem a presenca, nao a contagem.
4. **Uma linha por cliente ou por execucao**: nao afirmado. K2 exige um `display-message` por cliente, e nada mais.
5. **Truncamento (K5)**: a spec nao diz se ` +K` conta entradas (usado aqui) ou caracteres.
6. **Trava (K10)**: o total de 10 s e tratado como limite maximo; o teste aceita < 12 s, sem exigir ~5 s exatos.
7. **`--notify tmux` na v1.7 (S9)**: `tests/v17_stale.rs` ainda afirma que `--notify tmux` sai 2. A v1.9 substitui
   isso. Esse teste conflita com o contrato novo e **nao e tocado aqui**; K14 nao o inclui como regressao.
8. **Opcoes sem o sink (K11d/K11e)**: a spec diz "saida 2". Os testes exigem 2 e tem controle positivo, mas hoje o 2
   vem de opcao desconhecida; so K11a prova que a opcao existe quando o sink for implementado.

## O que ficou de fora

- Parte B inteira (portao do agentd, P1 a P13): proposta, nao implementada.
- Comportamento de `tmux` real (servidor ausente, `-S` apontando para servidor de verdade): fora de teste automatico
  (sem tmux real, sem agentes reais).
- Concorrencia entre execucoes do `stale` no mesmo stamp: fica na v1.8 (C12).
- Limites de bytes de saida (64 KiB): nao afirmado; nenhum teste gera saida desse tamanho.
- A regra "sem `tmux` no PATH quando nao ha `--notify tmux`" (K14a/K14b) e verificada colocando um falso no PATH;
  o teste prova so que nenhuma chamada chegou a ele.
