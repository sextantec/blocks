# Autorizacoes do Jabbur registradas para o repositorio `blocks`

Regra 5 do `AGENTS.md`: autorizacao e do Jabbur e fica registrada em `pendencias-jabbur.md` (bancada) **ou aqui**. Este arquivo e a copia local das que afetam o `blocks`; cada linha cita a frase do Jabbur, a data e a conversa. Quem registra e o Sextante; o Jabbur pode corrigir ou revogar qualquer linha.

| # | Autorizacao | Frase do Jabbur | Quando e onde |
|---|---|---|---|
| A1 | Contrato do `chatd` v1.6 (identidade explicita, saida 5, reconexao) aprovado, com as propostas escritas na spec (codigo 5 novo, reconexao do `send` so em falha de conexao, aviso sem opcao de silenciar) | "1.6: Aprovado" | 2026-10-15 noite, conversa do Claude Code |
| A2 | Contrato do `chatd` v1.7 (`chatd stale`) aprovado, com sink `tmux` reservado e recusado ate o `agentd` | "V1.7: Aprovado" | 2026-10-15 noite, conversa do Claude Code |
| A3 | Prazo T de "parado" = 15 min (900 s), padrao da v1.7 | "P-75c: OK" | 2026-10-15 noite, conversa do Claude Code |
| A4 | **Executor da v1.7 e da v1.6: o `codex-blocks` implementa**, v1.6 primeiro, depois v1.7, um commit por versao, sem push, sem instalar binario nem reiniciar o daemon | "Executor v1.7: Siga com a implementação" | 2026-10-15 noite, conversa do Claude Code |
| A5 | `wakerd` arquivado; verificador = `chatd stale` | "P75a: Confirmado" | 2026-10-15 noite, conversa do Claude Code |
| A6 | Autorizacao permanente do `chatd` (send/ack/inbox/status/show/watch com a propria identidade) | "Autorização AGENTS: OK" | 2026-10-15 noite, conversa do Claude Code |

**Continua exigindo ok do Jabbur** (`AGENTS.md`, "Precisa de ok"): push, publicar, instalar binario, reiniciar servico, escrever em `~/.local`, rede, credencial, cota alem de Haiku 5.5 e `gpt-6-luna`, mudar contrato publico de um bloco. As autorizacoes acima **nao** cobrem instalar o `chatd` novo nem reiniciar o daemon: isso e do Jabbur.

| A7 | **Troca do codigo de saida "daemon indisponivel" de 1 para 4** nos clientes `send`, `inbox`, `ack`, `show`, `status`, `watch` (o `bind` ja saia 4), mais o codigo 5 para permissao negada. Mudanca de contrato publico da v1.6 | "1: OK" (resposta ao item 1: autorizar o codigo de saida 4) | 2026-10-15 noite, conversa do Claude Code |
| A8 | **Commits e push do `blocks`** | "7. Autorizado os commits e push" | 2026-10-15 noite, conversa do Claude Code |
