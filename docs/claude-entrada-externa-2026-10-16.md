# Entrada externa no Claude Code: o que foi observado (2026-10-16)

Regra 13: o fonte do CLI do Claude Code NAO esta disponivel. O repositorio publico `anthropics/claude-code` (clonado raso em `ref/claude-code`, commit `03b5892` de 2026-10-15) contem so plugins, `mods`, exemplos, `CHANGELOG.md` e `feed.xml`; nenhum codigo do CLI. Portanto tudo abaixo e **observacao de experimento** ou **intencao (CHANGELOG)**, nunca leitura de codigo.

## Observado (sessao Claude de teste `tmp-2c`, Projects/tmp, painel herdr `w9:p1`, modo auto, aberta pelo Jabbur)
| caso | resultado |
| --- | --- |
| ocioso | `SendMessage` entregou na caixa de entrada; a sessao iniciou turno sozinha (tela mostrou turno em andamento 8 s depois) e respondeu `recebido` por SendMessage |
| ocupado (rodando `sleep 20`) | a mensagem B foi aceita durante o turno; chegaram `A-ok` e depois `B-ok`, **na ordem de envio**, sem perda |
| rascunho no compositor (16:11, painel `w9:p1` em manual mode, rascunho `rascunho do Jabbur, nao apague,` sem Enter) | a mensagem chegou, a sessao iniciou turno sozinha, respondeu `rascunho-ok` em ~4 s e o **rascunho ficou intacto** no compositor; a mensagem aparece como linha `Message from @NOME: ...` na conversa |
| aprovacao aberta (manual mode; `sleep 3; echo` roda sem pedir, `python3 -c ...` abre dialogo de aprovacao) | com o dialogo aberto por mais de 60 s, o painel ficou `blocked` no herdr, a mensagem E apareceu na conversa mas **nao respondeu nem destravou o dialogo**; o turno ficou parado ate o Jabbur escolher "1. Yes"; depois chegaram `D-ok` e `E-ok` **na ordem de envio**, sem perda |

Descobrir pares: `ListAgents` (nome, estado idle/busy, interativa/bg). Ao enviar: `SendMessage` com `to` = nome. Aviso opcional de ociosidade (`notify_when_idle`). A resposta chega com `from=uds:/run/user/1000/cc-socks/PID.sock` e `from-mode`.

## Intencao (CHANGELOG, nao provada)
- Existe um socket por sessao em `/run/user/1000/cc-socks/PID.sock` (modo 0700 no diretorio; sockets 0600). CHANGELOG:3362: o socket de entrada fecha conexoes que nao mandam uma linha completa em 30 s e "scripts posting to it" devem conectar so quando o dado estiver pronto. **Formato da linha: desconhecido** (nao ha fonte); nao adivinhar contra sessoes vivas.
- `crossSessionInbound` (CHANGELOG:3186, 3467, 3766): aceita, **segura para aprovacao** (sessao com permissoes ignoradas) ou recusa; recusa e informada ao remetente. `--restricted` nao abre o socket (845).
- Mensagens de pares aparecem recolhidas como `Message from @NOME: primeira linha` (3229).

## Comparacao com o Codex
| | Codex (`codex queue`) | Claude (`SendMessage`) |
| --- | --- | --- |
| Chamador externo qualquer processo | sim (CLI + socket do daemon) | so outra sessao do Claude; processo externo exigiria o formato do socket (desconhecido) |
| Estado legivel de fora | sim (`thread/read`: idle/active/aprovacao) | `ListAgents` (idle/busy), so de dentro de outra sessao |
| Duravel | fila no daemon | caixa de entrada da sessao (sem garantia se ela sair) |
| Ocupado | espera o fim do turno | espera e entrega em ordem |

## Pendente
Rascunho no compositor e aprovacao aberta do lado Claude; formato da linha do socket (so com fonte ou experimento isolado); como o `chatd` falaria com o Claude por esse socket (provavelmente nao: o Claude ja recebe por `delivery: watch`).

## Observacao lateral (nao investigada)
O `herdr agent list` mostrou `w6:p1` (codex-blocks) com o mesmo `agent_session` da thread de teste `tmp` (`01a145f9-...`); possivel mapeamento defasado do herdr. Os testes de fila usaram o id da thread pelo protocolo e nao foram afetados.
