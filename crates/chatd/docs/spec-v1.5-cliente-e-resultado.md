# chatd v1.5: configuracao do cliente com perfis e resultado automatico por RESPOSTA (contrato testavel)

Origem: decisao do Jabbur (2026-10-15): "Penso que ele deveria ter um arquivo de configuracao" (item 1) e "De acordo" ao resultado automatico por RESPOSTA (item 2). O terceiro item proposto (alarme por falta de aceite) **fica fora**: e do `agentd` (decisao do Jabbur: "Esse assunto nao seria do agentd?"; ver "Fora de escopo"). Contrato escrito pelo Sextante; implementacao pelo `codex-blocks`; testes independentes por outro agente que nao le `src/`. Protocolo v1.1 a v1.4 intacto para quem nao usa as novidades.

## Parte A: configuracao do cliente com perfis

### Problema (exemplo)
O Jabbur abre um terminal e roda `chatd status`: erro de uso, porque o cliente exige socket, identidade e token por flag ou variavel de ambiente e o terminal dele nao as tem. Os agentes exportam quatro variaveis em cada comando. Todos os agentes sao o mesmo usuario Unix; so a identidade os distingue. Um arquivo com **uma** identidade fixa viraria o padrao de todos (o Jabbur rodaria `inbox --take` como `claude` e consumiria as mensagens do Claude). Por isso o arquivo traz **perfis**.

### Regras
1. **Arquivo** (JSON, sem dependencia nova): `$CHATD_CONFIG` se definida; senao `$XDG_CONFIG_HOME/chatd/client.json`; senao `$HOME/.config/chatd/client.json`. Arquivo ausente no caminho padrao = sem configuracao (comportamento da v1.4 exato). `CHATD_CONFIG` apontando para arquivo ausente = erro de uso (saida 2).
2. **Formato:** `{"socket": S?, "state_dir": D?, "default_profile": P?, "profiles": {NOME: {"as": N, "token_file": F}}?}`. `socket`, `state_dir` e `token_file` aceitam `~/` no inicio (expandido com `$HOME`; so isso, sem `$VAR`). Nomes de perfil e `as`: `[a-z0-9_-]{1,32}`. `default_profile` precisa existir em `profiles`. **Campo desconhecido, tipo errado ou JSON invalido = erro de uso (saida 2), sem conectar a nada.**
3. **Seguranca do arquivo:** deve ser arquivo regular, do uid do processo e **nao gravavel por grupo/outros** (`mode & 0o022 == 0`); senao erro de uso (saida 2) com mensagem que nao imprime o conteudo. **O token nunca fica no arquivo**: so o caminho do arquivo de token (cuja regra 0600 da v1.1 nao muda).
4. **Precedencia, por valor:** flag > variavel de ambiente > arquivo > erro de uso como na v1.4. Vale para socket, estado, identidade e token.
5. **Perfil:** `--profile NOME` ou `CHATD_PROFILE`; sem eles, `default_profile`; sem nenhum, a configuracao nao fornece identidade (identidade vem de flag/ambiente, como hoje). Perfil inexistente = erro de uso (saida 2). O perfil fornece `as` e `token_file`; `--as`/`CHATD_AS` e `--token-file`/`CHATD_TOKEN_FILE` continuam prevalecendo.
6. **Quem le o arquivo:** os clientes (`send`, `inbox`, `ack`, `watch`, `show`, `bind`, `status`) e `log` (so `state_dir`). **`chatd serve` e `chatd token` nao leem** o arquivo do cliente (o servidor tem a configuracao dele).
7. **Nenhuma escrita:** o cliente nunca cria, altera nem remove o arquivo.
8. **`chatd whoami [--profile P] [--json]`** (novo): imprime a resolucao final, sem conectar ao daemon e **sem nunca imprimir token**: `as`, `socket`, `state_dir`, `token_file` (caminho), `profile` e a **origem** de cada valor (`flag`, `env`, `config` ou `-`). Saida 0 se resolveu pelo menos socket; 2 caso contrario (com o que falta).

### Contrato testavel (Parte A)
- **C1**: sem arquivo e sem variaveis: erro de uso como na v1.4 (mensagem conhecida). Com as variaveis da v1.4: tudo igual (regressao zero).
- **C2**: arquivo valido com `default_profile`: `chatd status` funciona sem nenhuma variavel; `whoami` mostra origem `config`.
- **C3**: precedencia: `CHATD_AS=outro` ou `--as outro` vence o perfil; `CHATD_SOCKET` vence o arquivo; `whoami` mostra a origem certa em cada campo.
- **C4**: `--profile` e `CHATD_PROFILE` selecionam o perfil; perfil inexistente = saida 2.
- **C5**: arquivo gravavel por grupo/outros, de outro uid ou nao regular = saida 2; nao conecta; a mensagem nao contem o conteudo do arquivo.
- **C6**: campo desconhecido, JSON invalido e `default_profile` inexistente = saida 2.
- **C7**: o token nunca aparece em `whoami`, `--json`, stderr nem no log; `serve` e `token new` ignoram o arquivo do cliente.
- **C8**: `~/` e expandido; `$VAR` nao e; `CHATD_CONFIG` ausente = saida 2.

## Parte B: resultado automatico por RESPOSTA

### Problema (exemplo)
Hoje o recibo `resultado` depende de o agente lembrar de rodar `chatd ack --stage resultado`. O Codex da Web responde ao B-095 com uma RESPOSTA, mas a mensagem original fica `aceito` para sempre; a caixa do Claude acumulou 4 itens assim. O remetente nao distingue "ainda trabalhando" de "ja respondeu".

### Regras
1. Ao registrar uma mensagem `kind: RESPOSTA` com `thread` = id de uma mensagem **M**, o daemon, **na mesma secao critica e antes do `fsync`**, tambem grava `resulted` para M quando **todas** forem verdade: `reply.from == M.to`, `reply.to == M.from`, M esta `registrado` ou `aceito`. Resultado `done`, `note` = `auto:<id da resposta>`.
2. Ordem no log: `registered` da resposta (seq N) e `resulted` de M (seq N+1), consecutivos.
3. **Nunca desfaz nem sobrescreve:** M ja em `resultado` (inclusive `refused` ou `failed` gravados antes) = nada acontece (sem evento, sem erro). Quem recusa grava `ack --result refused` **antes** de responder.
4. **Idempotencia:** reenvio da mesma resposta (`duplicate:true`) nao grava novo `resulted`.
5. So `RESPOSTA`: `AVISO`, `PEDIDO`, `PERGUNTA` e `ACK` nao fecham nada. `thread` de terceiros (reply.to != M.from) nao fecha.
6. **Opt-out:** `chatd send ... --keep-open` (parametro `keep_open: true` em `message.send`, fora do hash de idempotencia): nao fecha. Sem a opcao, fecha.
7. A resposta de `message.send` ganha `closed` (id de M ou `null`); `send --json` o imprime; sem `--json` a saida nao muda.
8. Visibilidade: o evento `resulted` automatico chega a M.from e M.to como qualquer `resulted` (com `watch --receipts` da v1.4 o remetente ve `RECIBO SEQ ID resultado:done TO`). Releitura do log aceita `resulted` com essa `note`.

### Contrato testavel (Parte B)
- **R1**: A envia PEDIDO a B; B aceita (`take`); B envia RESPOSTA com `thread`; M passa a `resultado/done`, `note` `auto:<id>`; A recebe `RECIBO ... resultado:done B` (com `--receipts`).
- **R2**: M ainda `registrado` (B nunca fez `take`) e B responde: M vai direto a `resultado` (transicao ja permitida na v1).
- **R3**: M ja `resultado/refused`: a RESPOSTA nao altera (sem novo `resulted`).
- **R4**: reply de terceiro (C responde a M que era de A para B) ou reply.to != M.from: nao fecha.
- **R5**: `--keep-open` nao fecha; `AVISO`/`PERGUNTA`/`ACK` com `thread` nao fecham.
- **R6**: reenvio idempotente nao duplica; seq consecutivos; `closed` correto em `--json`.
- **R7**: reinicio do daemon: o estado de M continua `resultado`; releitura do log sem erro.
- **R8**: regressao: suites v1.1 a v1.4 e a auditoria F passam.

## Fora de escopo
- **Alarme por falta de aceite / escalada** (reenviar, avisar o Jabbur, marcar agente como parado): e do `agentd` (estado dos agentes), que consome os fatos que o `chatd` ja expoe (`registered_at`, `accepted_at`, recibos via `show` e `watch --receipts`). O re-toque da campainha (`bell_rearm_s`) continua como esta.
- Criar o arquivo do cliente ou o token `jabbur`: e acao do Jabbur (o `chatd` nao escreve fora do estado do daemon).
