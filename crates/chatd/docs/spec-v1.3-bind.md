# chatd v1.3: `bind` e `delivery: auto` (contrato testavel)

Origem: decisao do Jabbur (2026-10-15): "o chatd nao pode depender de janelas fixas do herdr, temos que ter algum modo de identificar quem esta aonde" e "ele entra depois do F1". Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes por outro agente. Muda cliente e daemon; protocolo v1.1/v1.2 intacto para quem nao usa `bind`.

## Problema (exemplo)
Hoje `config.json` fixa `"pane": "w3:p1"`. Se o painel muda (aba nova, outro multiplexador) a campainha digita no painel errado, ou, com um valor errado, no painel de outra pessoa. A campainha so pode digitar em painel **provadamente do destinatario**.

## Regras
1. **`delivery: "auto"`** (sem `pane`): destinatario valido; a campainha usa a ligacao ativa (`bind`) do nome. Sem ligacao ativa nao ha campainha (`bell_failed` com `reason=unbound`, no maximo um por minuto, como os demais motivos). `bell` com `pane` fixo continua valendo (compativel com v1.1); se houver ligacao ativa para o nome, **ela prevalece** sobre o `pane` do arquivo.
2. **Metodo `agent.bind`** `{v:1, mux:"herdr"|"tmux", pane:"<id>"}` (so conexao autenticada; o nome e a identidade da conexao, nunca um campo). Resposta `{v:1, name, mux, pane, ttl_s}`.
3. **Verificacao, nao confianca.** A ligacao so e aceita se o processo que chamou (`SO_PEERCRED`, pid) **pertence ao painel declarado**:
   - **tmux:** `pane_pid` do painel (`tmux display-message -p -t <pane> '#{pane_pid}'`) deve ser ancestral do pid do chamador (cadeia de `PPid` em `/proc/<pid>/status`).
   - **herdr:** o `agent get <pane>` deve existir e o `cwd` e o tipo de agente devem coincidir com `/proc/<pid>/cwd` do chamador e com o nome do destinatario. O implementador deve **sondar** se `herdr` expoe pid do painel; se expuser, usar ancestralidade como no tmux. Se nao expuser, documentar a verificacao como mais fraca.
   - Falhou a verificacao: erro `forbidden` e evento `denied` (metodo, peer, nome); nada e ligado.
4. **Validade.** TTL de 120 s, renovado por novo `agent.bind` ou por `watch` ativo da mesma identidade (renovacao automatica a cada 30 s). Processo ausente (`kill -0` falha) invalida a ligacao na hora.
5. **Registro.** Evento `bound` (name, mux, pane, peer_pid) e `unbound` (motivo: `expired|process_gone|replaced`) no log; nenhum token, subject ou body. Visivel so em `chatd log`, nunca em `watch` de agente (como `denied`).
6. **Uma ligacao por nome**: nova ligacao do mesmo nome substitui a anterior (`unbound replaced`).
7. **Campainha** so digita em painel ligado e verificado **nesta** vida da ligacao; nunca em id vindo so de arquivo de configuracao quando `delivery: auto`.

## CLI
`chatd bind [--as N] [--mux herdr|tmux] [--pane ID] [--keepalive]`
- `--pane` padrao: variavel de ambiente do multiplexador (`TMUX_PANE` no tmux; no herdr, a que o implementador provar que existe).
- `--keepalive`: fica vivo renovando a cada 30 s; sai 0 em SIGINT/SIGTERM e desfaz a ligacao (`unbound`).
- Sem `--mux`, detecta pelo ambiente; ambiguo = erro de uso (codigo 2).
- Codigos de saida: 0 ligado; 2 uso; 3 verificacao recusada (`forbidden`); 4 daemon indisponivel.

## Contrato testavel
- **A**: `agent.bind` sem autenticacao e recusado; nome da ligacao = identidade da conexao.
- **B**: ligacao com painel que **nao** contem o processo e recusada (`forbidden`, `denied` registrado, nada ligado) — o caso do agente que tenta apontar a campainha para o painel de outro.
- **C**: ligacao valida habilita a campainha; sem ligacao com `delivery: auto` ha `bell_failed unbound` (no maximo 1/min) e nada e digitado.
- **D**: TTL: sem renovacao a ligacao expira (`unbound expired`) e a campainha para; `watch` ativo renova; processo morto invalida.
- **E**: `bell` com `pane` fixo (v1.1) continua funcionando sem `bind`; com `bind` ativo a ligacao prevalece.
- **F**: log: `bound`/`unbound` sem segredos e fora do `watch`; uma ligacao por nome (`replaced`).
- **G**: nenhuma regressao: a suite v1.1, v1.2 e a auditoria F passam.
Testes de integracao com multiplexador **falso** (script que imita `tmux display-message` e `herdr agent get`), sem tocar sessoes reais; pelo menos um teste contra `tmux -L <socket temporario>` real se o `tmux` existir no ambiente.

## Fora de escopo
Descoberta automatica do painel sem o agente chamar `bind`; troca de multiplexador em tempo real; `agentd` (o registro de quem esta onde passa para ele depois, e o `bind` vira cliente dele).
