# chatd v1.6: identidade explicita (fail-closed) e reconexao do cliente (RASCUNHO para decisao do Jabbur)

Origem: incidente de 2026-10-15 17:01. Depois de reiniciar o `codex-blocks`, `chatd whoami` nele devolveu `as=jabbur (config)`: sem `CHATD_AS`/`CHATD_TOKEN_FILE` no ambiente, o cliente cai em `default_profile: jabbur` (v1.5, regra A5) e usaria o token do dono. Todos os agentes rodam sob o mesmo usuario do sistema, entao "esqueci de exportar a variavel" vira "o agente fala como o dono". Nada foi enviado como `jabbur` (conferido em `chatd log`), mas a falha e silenciosa. No mesmo dia, dois agentes ficaram com `Operation not permitted` no socket (politica de sandbox do Codex mudou para rede desligada) e o cliente nao distingue isso de "daemon fora do ar". Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes pelo Haiku. **Nada aqui esta decidido.**

## Parte A: identidade explicita, sem perfil padrao

### Problema (exemplo)
O dono configura `default_profile: jabbur` para digitar `chatd status` no terminal dele. Um agente iniciado sem as variaveis de identidade herda o mesmo `client.json` e passa a agir como `jabbur`, sem erro e sem aviso.

### Regras
1. **`default_profile` e ignorado.** O arquivo continua valido (compatibilidade com a v1.5), o campo e lido e **nunca** fornece identidade. Cada execucao de cliente que encontra o campo escreve **uma linha em stderr**: `aviso: default_profile ignorado desde a v1.6; use --profile NOME`.
2. **Fontes de identidade, e so estas:** (a) `--as` / `CHATD_AS` **com** `--token-file` / `CHATD_TOKEN_FILE`; (b) `--profile` / `CHATD_PROFILE`. Por campo a precedencia da v1.5 (flag > ambiente > perfil) continua. **Nenhuma outra fonte.**
3. **Fail-closed:** sem (a) nem (b), saida **2** com `identidade nao definida: informe --profile NOME ou CHATD_AS e CHATD_TOKEN_FILE`. Nesse caso o cliente **nao abre arquivo de token algum e nao conecta ao socket**.
4. **`whoami`** passa a imprimir a **origem de cada campo** (`flag`, `env`, `profile:NOME`) e, se a identidade for `jabbur` com origem `profile:*` mas sem `--profile` explicito na linha (isto e, vinda de `CHATD_PROFILE` herdado), um aviso em stderr: `aviso: identidade jabbur vem de CHATD_PROFILE herdado do ambiente`.
5. **Uso do dono:** um alias no shell interativo dele, `alias chatd='chatd --profile jabbur'`. Alias nao e herdado por processos filhos; por isso e a forma recomendada (documentar; o `chatd` nao cria nem altera o alias). Exportar `CHATD_PROFILE=jabbur` no `.bashrc` e **desaconselhado**: agentes lancados desse shell herdariam.
6. **Limite conhecido, nao resolvido aqui:** os arquivos de token ficam legiveis entre processos do mesmo usuario. Contencao real exige um usuario ou namespace por agente (v2). Fica documentado em `SECURITY.md`.

### Contrato testavel (Parte A)
- **I1**: `client.json` com `default_profile` e nenhuma variavel: `chatd status` sai 2, mensagem `identidade nao definida...`, **o daemon falso nao recebe conexao** e nenhum arquivo de token e aberto (verificavel por `strace`/contador do arquivo falso).
- **I2**: o mesmo arquivo + `--profile jabbur`: funciona; stderr contem o aviso de `default_profile ignorado`.
- **I3**: `CHATD_AS`+`CHATD_TOKEN_FILE` sem arquivo de configuracao: funciona (regressao zero da v1.4).
- **I4**: `whoami --json` traz `source` por campo (`as`, `token_file`, `socket`, `state_dir`) e **nunca** o token.
- **I5**: `CHATD_PROFILE=jabbur` herdado, sem `--profile` na linha: `whoami` avisa em stderr; o cliente ainda funciona (aviso, nao erro).
- **I6**: perfil inexistente e arquivo gravavel por grupo/outros continuam saida 2 (v1.5, C4 e C5).

## Parte B: reconexao e erro de permissao distinto

### Problema (exemplo)
O daemon leva ~30 a 50 ms para voltar numa troca de binario; um `chatd send` nessa janela falha com `connect: Connection refused` e o agente conclui "chatd caiu". No incidente das 15:40 o erro real foi outro, `Operation not permitted` (sandbox), e o agente gastou tentativas como se fosse indisponibilidade.

### Regras
1. **Reconexao nos clientes de uma requisicao** (`send`, `inbox`, `ack`, `show`, `status`, `bind`): em `ECONNREFUSED` ou `ENOENT` no `connect`, ate **3 novas tentativas** com espera de 100, 300 e 900 ms (total <= 1,3 s); depois saida **4** (daemon indisponivel, como hoje). Nunca repete depois de a requisicao ter sido **enviada** (evita duplicar `send`; a idempotencia por `id` continua sendo a protecao, mas a repeticao automatica so cobre falha de conexao).
2. **Permissao negada e outro erro:** `EPERM` ou `EACCES` no `connect` **nao repete** e sai com codigo **5** e a mensagem `permissao negada ao socket: o ambiente do processo nao permite conectar (sandbox ou permissoes); nao e queda do daemon`.
3. **`watch`:** em queda da conexao, reconecta com backoff exponencial (200 ms ate 5 s) e retoma com `events.resume` do ultimo `seq` entregue **em memoria**; `--resume-file`, `--exit-after` e `--heartbeat` seguem como o item 7 do backlog (fora desta versao).

### Contrato testavel (Parte B)
- **R1**: daemon falso que aceita so na 3a tentativa: cliente conclui com saida 0 e **uma unica** mensagem registrada (sem duplicata).
- **R2**: socket inexistente por mais de 1,3 s: saida 4, 4 tentativas no total, nenhuma depois da primeira requisicao enviada.
- **R3**: socket cujo `connect` devolve `EPERM` (falso por `seccomp`/`LD_PRELOAD` de teste ou diretorio sem permissao): saida **5**, **uma** tentativa, mensagem sem o caminho do token.
- **R4**: `watch` com o daemon reiniciado no meio: nenhuma linha perdida nem repetida (seq contiguo), aviso em stderr da reconexao.
- **R5**: nenhuma regressao: suites v1.1 a v1.5 e a auditoria F passam.

## Perguntas abertas
1. Codigo 5 novo e aceitavel ou prefere reaproveitar o 4 com mensagem distinta?
2. A reconexao automatica do `send` e segura o bastante com `id` idempotente, ou so o `watch` deve reconectar?
3. O aviso de `default_profile` deve ser silenciavel (`CHATD_QUIET=1`)? Proposta: nao, para o agente nunca nao ver.

## Esclarecimentos (resolvem as ambiguidades dos testes independentes, 2026-10-15)
1. **Formato de `whoami --json` (I4):** continua o da v1.5: objeto com `as`, `profile`, `socket`, `state_dir`, `token_file` e **`sources`** (objeto campo -> origem). Valores de origem: `flag`, `env`, `profile:NOME` (veio de um perfil escolhido por `--profile`/`CHATD_PROFILE`), `ignored` (so para `default_profile`, que e lido e nao usado). `config` deixa de existir como origem de identidade. `profile` e `null` quando nenhum perfil foi escolhido explicitamente. O token nunca aparece.
2. **I5, "identidade jabbur":** significa `as == "jabbur"` com origem `profile:*` vinda de `CHATD_PROFILE` (e nao de `--profile` na linha). O aviso e emitido por `whoami` e por todo cliente que resolve identidade.
3. **R1b:** apos a requisicao ter sido enviada e a conexao cair sem resposta, o cliente **nao repete** e sai com codigo **4**, imprimindo o `id` da mensagem para reconciliar por `chatd show --id`.
4. **R2:** as 4 tentativas (1 + 3 repeticoes) sao contadas pelo falso; o tempo total e >= 1,2 s e <= 3 s.
5. **R4:** o aviso de reconexao do `watch` e uma linha em stderr que **comeca com `aviso: reconectando`**; a linha do `watch` e a da v1.2 (inalterada).
6. **Backoff do `watch` e a flag `--reconnect` da v1.2 (pergunta do codex-blocks, seq 253):** o padrao **sem** `--reconnect` passa a ser o da Parte B (200 ms ate 5 s, exponencial); **com** `--reconnect` fica **exatamente** o comportamento da v1.2 (1, 2, 4 ... 30 s), para manter a regressao zero. Se algum teste da v1.2 fixar o intervalo de 1 s **sem** a flag, o implementador diz qual e eu decido (mudanca de tempo interno, sem mudar contrato publico).
