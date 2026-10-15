# chatd v1.2: `watch` robusto (contrato testavel)

Ideia do Claude, aprovada pelo Jabbur ("Siga a vida", 2026-10-15). Muda so o cliente `chatd watch`; o protocolo do daemon nao muda. Tudo e opcional: sem as novas opcoes o `watch` se comporta exatamente como na v1.1.

## Sintaxe
`chatd watch [--as N] [--from-start] [--resume-file ARQ] [--exit-after DUR] [--heartbeat-file ARQ] [--reconnect]`
`DUR` = inteiro de 1 a 86400 seguido de `s`, `m` ou `h` (ex.: `30s`, `29m`, `2h`). Qualquer outra forma, `0`, ou valor acima de 24 h e erro de uso (codigo 2, nada e conectado).

## A. `--resume-file ARQ`
1. Mutuamente exclusivo com `--from-start` (erro de uso, codigo 2).
2. Ao iniciar, se `ARQ` existe e contem uma linha com um inteiro decimal `N` (>= 0), o `watch` retoma de `last_seq = N`: entrega os eventos com `seq > N` aos quais a identidade tem direito, em ordem, e depois segue ao vivo. Conteudo invalido ou `N` maior que o `head_seq` do daemon: erro de uso (codigo 2), sem entregar nada (nunca cai em silencio para "do inicio").
3. Se `ARQ` nao existe, o `watch` comeca no `head_seq` atual (como hoje, sem historico) e cria `ARQ` com esse valor antes de entregar o primeiro evento novo.
4. Depois de **imprimir e descarregar (flush)** cada linha de evento, o `watch` grava em `ARQ` o `seq` desse evento (atomico: temporario 0600 no mesmo diretorio e `rename`). Semantica: pelo menos uma vez (uma queda entre imprimir e gravar pode repetir o ultimo evento, nunca perde). O cursor gravado e o `seq` do ultimo evento **recebido** (eventos que a identidade nao pode ver nao chegam ao cliente: o daemon os filtra; ao retomar, o daemon filtra de novo, entao nao ha perda nem repeticao visivel).
5. `ARQ` e criado com permissao 0600.

## B. `--exit-after DUR`
Depois de `DUR` desde o inicio, o `watch` termina com codigo 0 sem imprimir nada alem das linhas de evento ja devidas. Uma linha de evento em andamento e concluida antes de sair. Combinado com `--resume-file`, o proximo `watch` retoma sem perder eventos.

## C. `--heartbeat-file ARQ`
Enquanto conectado e autenticado, o `watch` reescreve `ARQ` (0600, atomico) com o instante atual em segundos Unix (uma linha) a cada 5 s e uma vez ao conectar. **Nada e impresso na saida padrao** (cada linha da saida vira notificacao no Monitor). Se o `watch` perde a conexao, o arquivo deixa de ser atualizado.

## D. `--reconnect`
Na v1.1 o `watch` ja reconecta sozinho a cada 1 s para sempre (teste `tests/watch_log.rs::watch_reconecta_sem_imprimir_duplicata`); **isso nao muda sem a opcao**. `--reconnect` troca a espera fixa de 1 s por backoff exponencial 1, 2, 4, 8, 16, 30, 30... s (volta a 1 s depois de uma conexao bem-sucedida) e imprime uma linha em **stderr** (`chatd: reconectando em Ns`) a cada tentativa. Em ambos os casos, ao reconectar autentica de novo e retoma do ultimo `seq` entregue (em memoria; ou o de `--resume-file`), sem repetir nem perder eventos, ate `--exit-after` ou SIGINT/SIGTERM. Falha de autenticacao (`unauthenticated`) e **definitiva**: o `watch` sai com o codigo da v1.1, sem retry, com ou sem `--reconnect`.

## E. Invariantes
1. Nenhum byte de controle chega ao terminal (secao F da v1.1 continua valendo).
2. Nenhuma das opcoes novas muda o conteudo das linhas de evento impressas.
3. O token nunca aparece na saida, nos arquivos novos nem no stderr.

## Contrato testavel
- A1-A5, B, C, D acima, um teste por item; mais: opcao `--exit-after` invalida (`0`, `25h`, `abc`), `--resume-file` com lixo, com `N > head`, e exclusao com `--from-start`.
- Reconexao: reiniciar o daemon (novo processo, mesmo diretorio de estado) durante o `watch --reconnect` e provar que um evento registrado depois do reinicio e entregue exatamente uma vez.
- Retomada: registrar eventos, rodar `watch --resume-file F --exit-after 2s`, registrar mais eventos, rodar de novo: o segundo entrega so os novos, em ordem.
