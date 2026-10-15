# chatd v1.7: `chatd stale`, verificador de mensagens paradas (APROVADA pelo Jabbur em 2026-10-15)

Origem: consulta de arquitetura de 2026-10-15 (Opus e Sol 6.1, `docs/consulta-arquitetura-despertar-2026-10-15.md` da bancada): D + E, sem `wakerd`. Decisao do Jabbur: canal de aviso **tmux**, mas "tmux nao pode ser implantado antes de termos o `agentd`, pois precisamos do herdr para funcionar". Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes pelo Haiku. **Cliente puro: nao altera o daemon.**

## Problema (exemplo)
O `codex-blocks` perde a rede da sandbox as 15:40 e a campainha toca de 10 em 10 minutos sem que ninguem aceite; o Monitor do Claude expira e fica meia hora sem `watch`. Hoje a unica pista e olhar a tela. Ninguem e avisado de que **ha mensagem registrada sem aceite ha muito tempo**.

## Achado que muda o desenho
O `status` autenticado devolve **so a entrada da propria identidade** (`store.rs:742`, `status_for(Some(who))`). Um verificador com identidade propria **nao enxerga as caixas dos outros**, ao contrario do que as duas consultas supuseram. O `log.jsonl` (0600, mesmo usuario, mesma fronteira de confianca que ja vale para `chatd log`) tem `seq`, `ts`, `type`, `id`, `recipient` e, nos `registered`, `from`/`to`/`kind`. Por isso o verificador **le o log**, nao o `status`.

## Regras
1. **`chatd stale [--state-dir D] [--older-than SEC] [--bells N] [--notify SINK] [--stamp-file F] [--json]`**. Le `D/log.jsonl` (sem conectar ao daemon, sem token, como `chatd log`). `--older-than` padrao 900; `--bells` padrao 2.
2. **Pendente** de um destinatario: mensagem com `registered` e sem `accepted`/`resulted` depois. **Idade** = agora - `ts` do `registered`. **Campainhas sem aceite** = `bell_sent` do destinatario depois do `registered` mais antigo ainda pendente.
3. **Parado** = existe pendente com idade > `--older-than` **ou** `bells_sem_aceite >= --bells`. Um destinatario parado por episodio gera **um** aviso; o episodio termina quando nao ha mais pendente.
4. **Saida:** uma linha por destinatario parado, **texto fixo**: `[chat] parado: NOME N pendente(s), mais antiga M min, campainha K vez(es)`. **Nunca** remetente, assunto ou corpo. Codigos: 0 nada parado; **6** ha parado; 2 uso; 4 log ilegivel ou ausente.
5. **Deduplicacao:** `--stamp-file` (0600, criado pelo verificador) guarda por destinatario `{episode_first_seq, last_alert_ts}`; no mesmo episodio nao reavisa antes de `--older-than` novamente.
6. **Sink (`--notify`), enumerado e sem shell:** `stdout` (padrao); `file:CAMINHO` (anexa a linha, 0600). **`tmux` fica reservado e rejeitado com saida 2 ate a decisao do Jabbur** (depende de tmux ser o multiplexador em uso, o que so ocorre depois do `agentd`).
7. **Somente leitura sobre o daemon e sobre as mensagens:** nao envia, nao aceita, nao toca campainha, nao executa nada vindo do log. Campo desconhecido no log e ignorado; linha invalida e contada em `skipped` (stderr) e nao derruba a verificacao.

## Unidade de sistema (documentacao, instalada pelo Jabbur)
`chatd-stale.timer` a cada 5 min chamando `chatd stale --notify file:%h/.local/state/chatd/alerts.log`. Nenhum modelo e chamado; nenhum custo.

## Contrato testavel
- **S1**: sem pendencia: saida 0, nenhuma linha. **S2**: pendente de 14 min (`older-than 900`): 0; de 16 min: saida 6 e uma linha com NOME, N, M e K. **S3**: 2 `bell_sent` sem aceite, idade pequena: saida 6. **S4**: `accepted` depois do `registered`: nao conta. **S5**: dois destinatarios, um parado: so ele aparece. **S6**: o assunto e o corpo da mensagem nunca aparecem na saida nem no `stamp-file`. **S7**: segunda execucao no mesmo episodio nao repete a linha; episodio novo (nova mensagem apos tudo aceito) avisa. **S8**: linha de log corrompida: `skipped=1`, o resto e avaliado. **S9**: `--notify tmux` = saida 2. **S10**: log ausente = saida 4 e nada e criado. **S11**: nenhuma conexao ao socket (daemon falso nao registra acesso) e nenhum arquivo de token e aberto. **S12**: `--stamp-file` criado com 0600; nao segue link simbolico.

## Fora desta versao
`watch_active` (o estado do `watch` so existe em memoria; exige o daemon), idade por mensagem pelo `status`, sink `tmux`, acordar o Claude por `claude --resume` (so depois de duas semanas medindo e com teto decidido pelo Jabbur).

## Nota de seguranca da implementacao
Os eventos `registered` do `log.jsonl` carregam `subject` e `body` completos. O verificador le esses campos como parte da linha, mas **nunca** os copia para saida, `stamp-file`, mensagem de erro nem contador (S6). Parsear so `type`, `ts`, `seq`, `id`, `from`, `to` e `recipient`; descartar o resto sem armazenar.
