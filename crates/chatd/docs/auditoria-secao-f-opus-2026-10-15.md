# Re-auditoria Opus da secao F do chatd (commit 8a5f59a), 2026-10-15

Auditor: Opus independente, em copia do repositorio (build `--offline`, 228 testes passam), provas por scripts Python contra o socket e o binario (`probes/p1.py` entrada e `denied`, `p2.py` exibicao, `p3.py` trava por pid); sem rede, sem agente real. O repositorio nao foi tocado. Relatorio transcrito pelo Claude (o auditor nao pode gravar arquivo).

**Veredito: 0 ALTO, 0 MEDIO, 4 BAIXO. As tres correcoes fecham os achados M1, B1 e B2 da auditoria Fable. Piloto com o Codex real: pode.**

## Achados (BAIXO, nenhum bloqueia o piloto)
1. **`inbox --json` imprime DEL e C1 crus** (`src/client.rs:407`, serde so escapa < 0x20). So com mensagem gravada antes da correcao (a entrada nova e recusada); saida para maquina. Opcional: escapar DEL/C1 tambem no `--json` ou documentar.
2. **Cosmetico:** o `\t` legitimo do corpo aparece como `\x09` no `inbox` (`client.rs:233`, `:275`); e o texto literal `\x1b` digitado pelo usuario fica igual a um ESC escapado. UTF-8 multibyte e `\n` intactos; `--json` inalterado. Opcional: nao escapar `\t` no corpo exibido.
3. **`denied` nao grava a identidade autenticada** da conexao, so `peer_pid`, `peer_uid` e `claimed` (`server.rs:370-381`, `store.rs:536`); segue a spec F.2, mas o rastro nao diz qual agente (codex ou claude) tentou. Opcional (v1.2): campo `name` com a identidade da conexao.
4. **Informativo (nao e regressao):** (a) por F.3, "4 falhas e 1 sucesso" zera o contador, entao quem tem um token valido tenta 4 palpites por sucesso; (b) a trava e por pid, e um processo novo recomeca com 5 tentativas; (c) pid reusado herda a trava. Nada explotavel: token de 256 bits (`auth.rs:35`).

## Provas do que fecha
- **Controles na entrada** recusados (`invalid_envelope`): subject (ESC, BEL, DEL, NUL, TAB, U+0085, U+009B), body (`\r`, ESC, NUL, C1, DEL), `ref.v` (ESC, TAB), `note` (LF, ESC, C1). Aceitos: body com `\n`, `\t`, acentos, emoji, U+2028; subject com U+00A0/U+00FF; note com TAB. Body volta byte a byte no `take`.
- **Exibicao:** log antigo com `\u001b[2K`, `\u009b`, `\u007f`, `\u0007`, `\r`, `\u0085`; `inbox`, `inbox --take`, `log`, `watch --from-start` nao emitem controle cru (saem `\xNN`).
- **`denied`:** gravado para send, take, list e ack (by alheio e mensagem alheia); nome invalido omite `claimed`; nunca grava assunto, corpo ou token; o claude nao ve `denied` no `events.resume`; sobrevive a reinicio do daemon.
- **Poda e teto:** pid trancado com 5 falhas; depois 4.300 pids distintos (acima de 4.096): continua trancado mesmo com token certo; daemon vivo, 13,7 MB. Entradas trancadas so saem pelo teto se houver mais de 4.096 trancadas.
- **Autenticacao:** `ct_eq` em tempo constante sobre SHA-256; `verify` rele `auth.json` a cada tentativa (revogacao imediata); `auth.json` 0600, atomico; 2o `auth` na conexao recusado; `from` forjado recebe `forbidden`; `watch` sem token com `auth: required` sai com status 2.
- **Disco:** um agente autenticado gera ~1.000 `denied`/s (~520 MB/h); nao e classe nova (o `message.send` de 16 KiB enche mais rapido e nao ha limite de taxa por identidade). Sem travamento (locks nao se aninham).

## Nao verificado
SO_PEERCRED com outro uid (exige segundo usuario); corrida `pid_locked`/`pid_failure` com conexoes simultaneas do mesmo pid (anterior a correcao); eco de texto do cliente em erros; caracteres Unicode de formato (bidi U+202E, Cf), fora da F.1; eviction com mais de 4.096 pids trancados (so por leitura).
