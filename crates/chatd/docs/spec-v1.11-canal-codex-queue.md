# chatd v1.11: canal `codex-queue` (acordar um agente Codex sem digitar)

**Estado: APROVADA PARA IMPLEMENTAR.** Escrita pelo Sextante em 2026-10-16 a pedido do Jabbur ("Siga com o spec do codex-queue"). **Excecao ao congelamento do daemon AUTORIZADA pelo Jabbur em 2026-10-16 ("Excecao aberta", conversa do Claude Code), restrita a esta spec; nada mais do daemon muda.** Registro de autorizacao tambem em `docs/pendencias-jabbur.md` da bancada (P-87). Evidencias: `docs/codex-queue-analise-2026-10-16.md` (fonte do Codex 0.163.0 e experimentos medidos) e `docs/pesquisa-web-entrada-externa-2026-10-16.md`.

## Problema (exemplo)
O `codex-blocks` termina um turno e fica ocioso. O Sextante manda um PEDIDO pelo `chatd`. Hoje a unica forma de acordar o Codex e a campainha: digitar `[chat] 1 pendente(s). Rode: chatd inbox --take` no painel. No herdr isso confere `idle` antes; no tmux a v1.10 **desligou** a digitacao porque ela nao ve dialogo de aprovacao nem rascunho. Sem canal, o Codex so le o PEDIDO quando alguem digita no painel.

## Solucao
O Codex tem `codex queue --thread ID --message TEXTO`: enfileira uma mensagem no daemon do app-server, **sem teclado**. Medido (2026-10-16, sessao Luna de teste): ocioso = turno em 0,2 s; ocupado = espera o fim do turno e roda em turno proprio; rascunho no compositor da TUI fica intacto; thread inexistente = erro (saida 1). O chatd passa a chamar esse comando como mais um tipo de entrega.

```
chatd (bell) --argv--> codex queue --thread UUID --message "[chat] N pendente(s)..." --> daemon do Codex --> turno do agente
```

## Contrato

### 1. Configuracao (`config.json`, estrita como as demais)
- Destinatario novo: `{"delivery": "codex-queue", "thread_file": "/caminho/absoluto"}`. `thread_file` e **obrigatorio** e absoluto; `pane` nao e aceito com esse valor (e `thread_file` nao e aceito com os outros).
- Chave global `codex_bin` (padrao `"codex"`, validada como `tmux_bin`: nao vazia, sem NUL).
- Chave global `codex_versions`: lista de versoes aceitas (ex.: `["0.163.0"]`). **Obrigatoria e nao vazia** se algum destinatario usar `codex-queue` (o recurso e **nao documentado** oficialmente e pode mudar sem aviso); sem ela, o daemon recusa a config.
- Chave desconhecida continua recusada. Config existente sem essas chaves nao muda de comportamento.

### 2. De onde vem o id da thread
O Codex injeta `CODEX_THREAD_ID` nos comandos que o agente executa (fonte: `core/src/exec_env.rs:30,42`, `protocol/src/shell_environment.rs:167`). O proprio agente (ou o script que o inicia) grava o id no `thread_file`:
`printf '%s\n' "$CODEX_THREAD_ID" > ~/.local/state/chatd/threads/codex-blocks`
Nao ha protocolo novo, nem registro de ligacoes, nem reinicio do daemon: o arquivo e lido **a cada toque**. Uma sessao nova do Codex so precisa regravar o arquivo.

Validacao do arquivo (qualquer falha = `bell_failed` com a razao e **nenhuma execucao**):
- existe, e arquivo regular (nao symlink), do mesmo usuario do daemon, sem escrita para grupo/outros, tamanho <= 64 bytes -> senao `thread_file_invalid` (ausente: `thread_file_missing`);
- conteudo, sem a quebra de linha final, casa exatamente `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (impede texto que pareca opcao, como `--help`) -> senao `thread_file_invalid`.
So o UUID e aceito; nome de thread nao (nomes mudam).

### 3. O toque
1. Mesma elegibilidade da campainha de hoje (pendencia > 0, `bell_due`, no maximo 1 evento de falha por minuto por destinatario, rearme `bell_rearm_s`, rearme duravel da v1.10).
2. **Checagem de versao:** executar `codex_bin --version` (sem shell, limite 5 s, saida limitada), extrair o numero e exigir que esteja em `codex_versions`; resultado em cache por 10 min (por destinatario ou global, tanto faz); fora da lista = `bell_failed codex_version_unsupported` e **nao** chama `queue`.
3. Executar, por argv, sem shell, limite 5 s, saida limitada a 64 KiB e `stdin` nulo:
   `codex_bin queue --thread UUID --message TEXTO`
   com TEXTO = o texto fixo atual (`prompt_text(n)`: so a contagem; nunca remetente, assunto ou corpo).
4. Sucesso = saida 0 **e** a saida comeca por `Queued message ` (formato medido: `Queued message ID for thread UUID.`) -> `bell_sent`. Qualquer outra coisa:
   - nao conseguiu executar -> `spawn_failed`; passou de 5 s -> `timeout`; saida diferente de 0 ou saida 0 sem `Queued message` -> `queue_failed`.
   - **Timeout depois de o comando ter aceitado e resultado incerto:** tratado como falha; pode haver mensagem duplicada, e e inofensiva (o agente roda `inbox --take`, que na segunda vez esta vazio).
5. **Nunca** cai em `send-keys` nem em `herdr agent prompt`, em nenhuma falha. O ramo herdr e o tmux nao mudam.
6. Nao ha tipo de evento novo: usa `bell_sent` e `bell_failed` (campo `reason` texto livre, como na v1.10).

### 4. Garantias e limites declarados
- **Custo:** cada toque bem-sucedido inicia um turno do agente (tokens do modelo). Pior caso com a pendencia aberta: um turno a cada `bell_rearm_s` (600 s padrao = 6/h). Ajustar `bell_rearm_s` se pesar.
- **Atraso:** observado ~0,2 s ocioso; ate o fim do turno atual quando ocupado.
- **Aprovacao aberta:** nao reproduzida em experimento; pelo codigo, aprovacao pendente e turno ativo e a mensagem espera (`core/src/session/turn_input.rs:519`). Confirmar na migracao com o Jabbur presente.
- **Mensagens repetidas:** pode enfileirar iguais enquanto o turno dura mais que o rearme; a coalescencia nao foi provada e nao se conta com ela.
- **Rascunho:** o turno nao espera o usuario terminar de digitar (medido); o texto digitado nao e alterado.
- **Seguranca:** o unico dado variavel que chega ao Codex e a contagem; o id da thread passa por validacao de formato antes do `exec`; o arquivo e confiavel como a propria config (mesmo usuario, sem escrita de terceiros).

## Contrato testavel (daemon real em dir temporario; `codex` FALSO que grava o argv; `herdr` e `tmux` FALSOS que gravam chamadas)
- **Q1** caminho feliz: argv exatamente `queue --thread UUID --message "[chat] 1 pendente(s). Rode: chatd inbox --take"`, uma vez; `bell_sent`. **Q2** `thread_file` ausente: `bell_failed thread_file_missing`, `codex` nao e executado. **Q3** conteudo invalido (vazio, `--help`, 65+ bytes, duas linhas, UUID maiusculo, espaco): `thread_file_invalid`, sem exec. **Q4** symlink ou arquivo gravavel por grupo/outros: `thread_file_invalid`, sem exec. **Q5** versao fora de `codex_versions`: `codex_version_unsupported`, `queue` nao chamado. **Q6** falso sai com 1: `queue_failed`. **Q7** falso dorme mais de 5 s: `timeout` (daemon nao trava). **Q8** falso sai 0 sem imprimir `Queued message`: `queue_failed`.
- **Q9** rearme: segundo ciclo dentro de `bell_rearm_s` nao chama de novo. **Q10** `accepted` da pendencia + mensagem nova: toca de novo no ciclo. **Q11** configuracao: `codex-queue` sem `thread_file`, com `pane`, `thread_file` relativo, ou sem `codex_versions`: o daemon recusa; chave desconhecida continua recusada; `thread_file` com outro `delivery`: recusado.
- **Q12** o `herdr` e o `tmux` falsos **nao recebem nenhuma chamada** para um destinatario `codex-queue`, nem em falha. **Q13** nenhum tipo de evento novo no log. **Q14** o texto enviado e so a contagem (assunto e remetente com metacaracteres e aspas nao aparecem). **Q15** `codex_bin` com espacos e metacaracteres (`;`, `$()`) e executado como um unico argv, sem interpretar. **Q16** reiniciar o daemon logo depois de um toque nao toca de novo dentro do rearme (v1.10). **Q17** o conteudo do `thread_file` e relido a cada toque (mudar o arquivo entre dois toques muda o `--thread`).
- Experimento real, fora do gate: com a sessao Luna de teste e `codex_versions: ["0.163.0"]`, ocioso e ocupado, como em `codex-queue-analise-2026-10-16.md`.

## Fora desta versao
Ler o estado da thread (`idle`/`active`/`waitingOnApproval`) pelo protocolo do app-server antes de enfileirar: exigiria cliente WebSocket no daemon (dependencia nova) e fica para o `agentd` (observacao, fonte `codex app-server`). Canal equivalente para o Claude (ver P-86). Coalescencia de mensagens iguais. `chatd bind` para Codex. Qualquer outra mudanca no daemon.

## Instalacao (nao normativa; feita pelo Jabbur, apos a implementacao)
Binario novo e reinicio do `chatd.service`; no `config.json` do destinatario: `"codex-blocks": {"delivery": "codex-queue", "thread_file": "/home/mjabbur/.local/state/chatd/threads/codex-blocks"}`, mais `"codex_bin": "/home/mjabbur/.local/bin/codex"` e `"codex_versions": ["0.163.0"]`. Cada sessao nova do `codex-blocks` regrava o `thread_file` (hook de inicio ou primeira acao). Sem tipo de evento novo: a volta ao binario anterior continua possivel (a config nova seria recusada pelo binario antigo; manter copia da config anterior).
