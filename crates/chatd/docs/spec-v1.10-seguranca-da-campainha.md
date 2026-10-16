# chatd v1.10: duas correcoes de seguranca da campainha (excecao ao congelamento do daemon)

Origem: consulta ao Opus e ao Astra (2026-10-16): os dois apontam a campainha como o unico pedaco do daemon que age sobre outro agente sem pedido, e o Opus achou no codigo que o rearme nao e duravel. **Excecao ao congelamento do daemon autorizada pelo Jabbur em 2026-10-16 ("Abra a excecao e corrija o daemon")**, restrita a estas duas correcoes; nada mais do daemon muda. Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes pelo Haiku. Emenda a campainha da v1.1/v1.3.

## Problema 1 (exemplo): a campainha tmux digita sem olhar
`bell.rs`, `send_prompt`, ramo `Mux::Tmux`: roda `tmux send-keys -t PAINEL -l TEXTO` e depois `send-keys ... Enter`, **sem checar se o agente esta ocioso** (o ramo herdr checa). Se o CLI do agente estiver num dialogo "aprovar comando? y/n", o `Enter` confirma a opcao padrao; se o Jabbur estiver digitando um rascunho, o texto entra no meio dele. Hoje o risco e latente (so vale com `chatd bind --mux tmux`), mas e o primeiro incidente provavel.

## Problema 2 (exemplo): o rearme some quando o daemon reinicia
O estado "tocada ha menos de `bell_rearm_s`" (`outstanding`) vive so em memoria. O replay do log preenche `last_bell_seq` mas nao `outstanding`. Reiniciar o daemon as 14:00 com `claude` tocado as 13:58 e ainda pendente toca de novo as 14:00, e todos os destinatarios com pendencia recebem a campainha de uma vez.

## Correcao 1: a campainha tmux nao digita por padrao
1. Nova chave opcional do `config.json`: `"bell_tmux_typing": true|false`, **padrao `false`**. Chave desconhecida continua recusada; ausente = `false`, entao **nao e preciso editar a config existente**.
2. Com `false`, o ramo `Mux::Tmux` de `send_prompt` **nao executa nenhum comando** (nem `send-keys`, nem outro): devolve falha com a razao `tmux_typing_disabled`, que vira o evento `bell_failed` ja existente (`recipient`, `reason`), no maximo um por minuto por destinatario (regra atual). **Nao ha tipo de evento novo**; so um valor novo de `reason`, texto livre que a versao anterior ja le.
3. Com `true`, o comportamento e o da v1.3, sem mudanca (`send-keys -l TEXTO` e `Enter`), para uso em laboratorio. A documentacao da chave diz que e **inseguro** (sem checagem de ocioso).
4. O ramo herdr **nao muda**. A entrega por `watch` nao muda. `bind --mux tmux` continua aceito (a ligacao existe; so a digitacao fica desligada).
5. Efeito colateral declarado: com `bell_tmux_typing` falso, um destinatario ligado por tmux nunca recebe campainha digitada; ele continua recebendo pelos canais passivos (hook de consulta, `@chatd_stale`, aviso do `stale`).

## Correcao 2: o rearme sobrevive ao reinicio
1. Ao reconstruir o estado a partir do log (inicio do daemon), cada evento `bell_sent` de um destinatario fixa `outstanding[destinatario]` no instante `agora - idade`, onde `idade = relogio_de_parede_agora - ts_do_evento` (UTC). O `bell_sent` mais recente do destinatario vale.
2. `idade < 0` (relogio recuado ou `ts` no futuro) ou `ts` ilegivel: `idade = 0` (espera o rearme inteiro; falha para o lado seguro). Se o sistema nao puder representar o instante (uptime menor que a idade), usa `agora` (idade 0).
3. `idade >= bell_rearm_s`: nao ha impedimento (`bell_due` ja compara com o rearme).
4. As remocoes atuais continuam: `accepted` e `resulted` de uma mensagem do destinatario apagam o `outstanding` dele, tambem durante o replay, na ordem do log. Uma pendencia nova depois de um `bell_sent` **nao** apaga o `outstanding` (igual ao funcionamento ao vivo).
5. Nenhum evento novo e gravado pelo replay; o log nao muda; a versao anterior do binario le o mesmo log.

## Contrato testavel (daemon real em diretorio temporario; `herdr` e `tmux` FALSOS que registram o argv)
- **V1** `delivery: bell` com `bind --mux tmux`, `bell_tmux_typing` ausente, mensagem pendente: o `tmux` falso **nao recebe nenhuma chamada** de `send-keys` (nem de outro subcomando); `chatd log` mostra `bell_failed` com `reason: tmux_typing_disabled`; no maximo um por minuto (duas rodadas da campainha dentro de 60 s = um evento). **V2** o mesmo com `"bell_tmux_typing": false` explicito. **V3** com `"bell_tmux_typing": true`: `send-keys -t PAINEL -l TEXTO` e `send-keys -t PAINEL Enter`, uma vez, e `bell_sent` (regressao da v1.3). **V4** `"bell_tmux_typing": "sim"` ou chave com outro nome = daemon recusa a config (saida de erro como as outras chaves invalidas). **V5** herdr inalterado: `idle` recebe `agent prompt`, `working` nao (regressao v1.1/v1.3). **V6** nenhum tipo de evento novo no log em V1 a V5 (so os ja existentes).
- **V7** log com `bell_sent` de `claude` ha 10 s e uma pendente de `claude`, `bell_rearm_s` 600, `delivery: bell` herdr falso `idle`: ao iniciar o daemon, **nenhuma** chamada `agent prompt` nos primeiros segundos (janela de observacao maior que um ciclo da campainha, com `bell_poll_ms` pequeno). **V8** o mesmo com o `bell_sent` ha 700 s: toca em um ciclo. **V9** `bell_sent` com `ts` no futuro (+1 h): nao toca (idade 0). **V10** `bell_sent` com `ts` ilegivel: nao toca, o daemon inicia. **V11** `bell_sent` ha 10 s seguido de `accepted` da mensagem pendente e de uma mensagem nova depois: o `accepted` apagou o rearme, entao a mensagem nova toca no ciclo. **V12** dois destinatarios, so um com `bell_sent` recente: toca so o outro. **V13** o replay nao acrescenta eventos ao log (tamanho e conteudo iguais antes e depois de iniciar e parar o daemon sem mensagens novas).

## Fora desta versao
Qualquer outra mudanca no daemon; o portao do agentd (Parte B da v1.9, **retirada como caminho** apos a consulta: o agentd fica como observacao, nunca autoridade para digitar); o `taskd`; a politica de lembrete por tarefa; o canal passivo de despertar.

## Instalacao (nao normativa; feita pelo Jabbur)
Binario novo e **reinicio do `chatd.service`**: sem tipo de evento novo, a volta ao binario anterior continua possivel. Nenhuma edicao da config e necessaria (padrao `false`). Depois do reinicio, `chatd log` nao deve mostrar campainha em massa para quem foi tocado nos ultimos 10 minutos.
