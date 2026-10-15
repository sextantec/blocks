# Piloto do chatd com o Codex real, 2026-10-15

Daemon: `chatd serve` (release de `e95aef0`), `auth: required`, destinatarios `claude` (watch) e `codex` (campainha no painel `w2:p1`), tokens 0600 em `pilot/` (ignorado pelo git). Autorizado pelo Jabbur ("Fique à vontade de fazer o piloto, precisa que eu inicie algo?"). Participantes: Claude (Sonnet 5.5) e Codex (GPT-6-Luna high), ambos na conta o mesmo usuario do dono.

## Resultado: passou
| Passo | Hora (host) | Evidencia |
|---|---|---|
| Claude envia PEDIDO ao Codex (`chatd send --to codex`) | 07:08:23 | evento 1 `registered`, `seq 1` |
| Campainha toca no mesmo segundo (herdr `agent get` + `agent prompt`) | 07:08:23 | evento 2 `bell_sent`; texto "[chat] 1 pendente(s). Rode: chatd inbox --take" no prompt do Codex |
| Codex roda `inbox --take` (aceito automatico, note `take`) | 07:08:31 | evento 3 `accepted` |
| Codex `ack --stage resultado --result done` | 07:08:34 | evento 4 `resulted` |
| Codex envia RESPOSTA ao Claude | 07:08:34 | evento 5 `registered` codex>claude |
| Claude recebe pelo `chatd watch` (Monitor) e confirma com `inbox --take` | 07:08:35 | evento 6 `accepted`; mensagem "recebi pela campainha" |
Ida e volta de 11 s, sem raspagem de tela, sem leitura do painel.

## Seguranca observada
- Identidade imposta pela conexao: `send --from codex` com o token do claude responde `forbidden: identidade diferente da conexao` e grava `denied method=message.send claimed=codex pid=... uid=1000` (marcado `!!` no `chatd log`).
- O sandbox do Codex nao negou nada: leu o token (0600, mesmo usuario), executou o binario fora do workspace dele e conectou ao socket Unix.

## Achados do piloto (nao bloqueiam)
1. O monitor do painel do Codex (`le-codex.py`) entregou o aviso W-093 com ~6 min de atraso (tela rolada); o `chatd` nao tem esse problema, mas o estado `done` do herdr nao distingue "terminei" de "mensagem nova chegou": a campainha so toca com o Codex `idle`/`done` e a caixa vazia.
2. Ate a primeira mensagem de teste, o Codex nao reagiu a dois pedidos normais (B-091 e B-092 pelo painel) por ~10 min, mas respondeu aos dois em segundos depois; hipotese: turno encerrado sem fila. A campainha do chatd e exatamente o mecanismo que cobre isso.
3. Daemon nao esta sob supervisor: foi iniciado por `nohup` na sessao. Para uso continuo precisa de unit de usuario (systemd), revisada pelo Jabbur.
