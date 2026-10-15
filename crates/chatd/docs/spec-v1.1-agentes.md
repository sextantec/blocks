# chatd v1.1: otimizações para consumo só por agentes (adendo à spec v1)

Origem: Jabbur (2026-10-15): "O chatd é algo que será utilizado e consumido somente por agentes. Fique à vontade de otimizar a linguagem entre vocês se necessário e se for ajudar." A v1 (`docs/spec.md`) fica como base e passa primeiro pelo ciclo completo (implementação, testes independentes, mutação). A v1.1 é uma **segunda rodada pequena**. Única quebra de compatibilidade: a autenticação (seção 0) passa a ser obrigatória por padrão; a configuração aceita `"auth": "off"` para os testes e clientes da v1.

## Princípio
Cada mensagem custa tokens de **quem lê** (e cada ida e volta custa um turno). Otimizar para: menos chamadas por mensagem, menos texto por mensagem, nenhum campo que o agente precise adivinhar. Humano continua podendo **auditar** (o log é JSONL legível e o `chatd log` imprime em claro), mas não é o leitor de referência.

## 0. Autenticação por token (pedido do Jabbur: "algum nível de autenticação... evitaria mensagens cruzadas")
**Problema, exemplo:** dois agentes no mesmo usuário usam o mesmo socket. Se o Codex roda `chatd send --from claude-main` por engano (ou um painel herda a variável de ambiente errada), a mensagem sai como se fosse do Claude e o recibo vai para a caixa errada. Hoje o `from` e o `by` são só texto.
**Solução (simples):** cada agente tem um **token próprio**; a conexão se autentica uma vez e **a identidade passa a ser a da conexão**, não um campo de texto.
1. **Emitir:** `chatd token new NOME --state-dir D [--out ARQ]` gera 32 bytes aleatórios (hex, de `/dev/urandom`), grava só o **SHA-256** em `D/auth.json` (0600) e entrega o token **uma única vez** (em `--out` com 0600, ou em `stdout`). `chatd token revoke NOME` remove. O servidor relê `auth.json` a cada tentativa de autenticação (revogação vale sem reinício).
2. **Usar:** o cliente lê `CHATD_NAME` e `CHATD_TOKEN_FILE` (caminho; o conteúdo nunca vai em argumento de linha de comando nem é impresso). Primeira requisição da conexão: `auth {v, name, token}` -> `{v, name}`. Qualquer outro método antes disso: erro `unauthenticated`. Token errado: `unauthenticated` (comparação em tempo constante); 5 falhas fecham a conexão.
3. **Identidade imposta:** `message.send` usa `from` = identidade (se vier outro valor: `forbidden`); `list`, `take` e `ack` só operam na caixa da própria identidade (`to`/`by` ausentes = identidade; diferentes = `forbidden`); `events.resume` só entrega eventos em que a identidade é `from` ou `to`; `status` devolve só contagens. Nome sem token não conecta (mas pode ser destinatário).
4. **Invasão: modelo de ameaça e camadas (pedido do Jabbur: "e invasão").**
   | Quem tenta | Camada que barra | Barra de verdade? |
   | --- | --- | --- |
   | Outro usuário Unix da máquina | diretório 0700, socket 0600, **checagem de `SO_PEERCRED`: uid do cliente = uid do servidor, senão a conexão é fechada** e token | sim |
   | Rede (outra máquina, tailnet) | o `chatd` **só escuta em socket Unix, nunca em TCP** (regra permanente); acesso remoto só entra pela conta do usuário (ssh) | sim |
   | Agente com identidade trocada por engano | token por agente + identidade imposta pela conexão (itens 1 a 3) | sim |
   | Processo do **mesmo usuário** sem o token | não lê a caixa nem envia como outro; **mas** pode ler o arquivo de token de outro agente ou `/proc/<pid>/environ` | **não totalmente** |
   | Agente induzido por texto malicioso (prompt injection) a abusar do chatd | corpo **nunca executado**; campainha de texto **fixo**; mensagem é informação, não ordem; limites de tamanho e de conexões | limita o dano a "informação falsa", que o agente já trata como suspeita |
   **Controle detectivo (barato, no espírito do verificador do Kelpie):** cada conexão e cada `auth` gravam um evento `auth_ok`/`auth_failed` com `peer_uid`, `peer_pid` e nome pretendido (nunca o token); 5 falhas em 60 s do mesmo pid trancam novas tentativas dele por 5 minutos; `chatd log` destaca falhas e envios de `from` forjado (`forbidden`). Quem invadir deixa rastro.
   **Única barreira real contra o mesmo usuário:** um usuário Unix por agente (como já existe a conta `escriba` para os executores), com `auth.json` mapeando `uid -> nome` e `SO_PEERCRED` provando a identidade sem token. Hoje Claude e Codex rodam ambos como o mesmo usuario do dono, então **não propomos agora**; fica como decisão futura do Jabbur se o risco justificar a complexidade de contas separadas.
5. **Entrega do token ao Codex:** o Codex da Web não alcança a bancada nem o scratchpad; o arquivo de token dele precisa estar num caminho que ele leia, **fora do que ele comita** (decisão do Jabbur no piloto: onde fica esse arquivo).
6. **Testes:** conexão de uid diferente do servidor é recusada (simulável só em unidade, com uid injetado); `auth_failed` gravado com `peer_pid` e sem token; 5 falhas trancam o pid; sem `auth` = `unauthenticated`; token de A não autentica como B; `from` forjado = `forbidden`; A não lista nem aceita a caixa de B; token revogado deixa de valer; `auth.json` e o arquivo de saída em 0600; o token nunca aparece em `stdout`/`stderr` do cliente nem em log.

## 0.1 Só Claude e Codex (decisão do Jabbur: "usaremos somente Claude e Codex, portanto o código deve prever somente eles")
O risco de um usuário Unix por agente **não se justifica** (decisão do Jabbur, 2026-10-15); fica como está na seção 0, item 4. O código pode assumir **dois tipos de agente**, sem arquitetura de plugins:
| Agente | Como recebe | Por quê |
| --- | --- | --- |
| **Claude** (sessão da bancada, painel `w1:p1` do Jabbur) | **`chatd watch --as NOME`**, um processo que o próprio Claude arma no `Monitor`; imprime uma linha por mensagem nova e nunca toca o painel | `w1:p1` é o painel do humano: a campainha ali se mistura com o que o Jabbur digita (problema 2 do P-44). O Monitor já entrega eventos à sessão sem polling de tela |
| **Codex** (painel `w2:p1`) | **campainha** (`herdr agent prompt`), com portão `idle`/`done` | o Codex não tem Monitor; só enxerga o que entra na caixa |
Consequências no código: `recipients.<nome>.delivery` = `"watch"` ou `"bell"` (só estes dois valores; `bell` exige `pane`); um único adaptador de campainha (herdr), **sem trait genérico**; o laço da campainha só roda para quem é `bell`; `watch` é o `events.resume` com formatação compacta (linha por evento, sai quando o socket fecha). Nada de multi-host, multi-conta ou adaptadores futuros: se um terceiro agente aparecer, muda-se o código.
**Configuração mínima esperada (o "nível de configuração" que o Jabbur previu):** um arquivo com 2 destinatários, o `pane` do Codex e o caminho do `herdr`; um token por agente emitido por `chatd token new`.

## Mudanças
1. **`inbox --take`** (e `message.take` no socket): lista as mensagens pendentes **com corpo** e marca todas como `aceito` na mesma operação atômica. Hoje são 3 passos (listar, ler, aceitar); passa a ser 1. `aceito` passa a significar "o agente recebeu o texto" (documentar). Sem `--take`, o `inbox` continua só leitura.
2. **Corpo inline por padrão em `inbox`** (linha compacta: `seq id kind from>to | subject` e, na linha seguinte, o corpo recuado). `--ids` devolve só os ids.
3. **`from` implícito:** `CHATD_AS` (nome do agente) vale para `send --from`, `inbox --as` e `ack --as`. Agente configura uma vez no ambiente do painel e nunca mais repete.
4. **`refs` estruturado e opcional** em `message.send`: lista de `{"t":"path|commit|msg|job|url","v":"..."}` (até 16). Quem envia aponta para o arquivo, o commit ou a mensagem em vez de colar o conteúdo no corpo. Fecha a porta para "relatório de 200 linhas no corpo": o corpo continua com teto de 16 KiB, mas a **recomendação** passa a ser corpo ≤ 15 linhas e o resto por `refs`.
5. **Campainha mínima:** `[chat] N pendente(s). chatd inbox --take` (o `--as` vem de `CHATD_AS`). Sem o nome do destinatário no texto, a mesma frase serve para todos e é mais curta.
6. **Respostas e erros curtos:** `send` imprime só `id seq` (uma linha, sem JSON, salvo `--json`); erros em `stderr` como `chatd: <code>` + 1 linha de causa. Nada de texto explicativo longo no caminho comum.
7. **Idioma:** campos, enums e códigos ficam em inglês ASCII (já são). **Corpo: qualquer idioma**; para tráfego entre agentes recomendo inglês técnico terso (estimativa: o texto em português gasta mais tokens que o equivalente em inglês; **não medido**). Tudo que o Jabbur precisa ler continua em português nos documentos dele, não no tráfego.
8. **`chatd log [--tail N] [--follow]`:** visão de auditoria do dono (linhas legíveis com hora local). Não é usada por agentes.

## O que NÃO otimizar
- Não comprimir nem codificar o corpo (base64, abreviações privadas): quebra a auditoria e o próprio agente erra mais. Terso sim, cifrado não.
- Não remover os três recibos nem a idempotência para ganhar linhas.
- Não aceitar corpo que seja instrução de máquina para ser executada: mensagem continua sendo **informação, não ordem** (regra do protocolo Claude-Codex).

## Critérios de aceite da v1.1
`--take` atômico sob concorrência (duas chamadas simultâneas nunca devolvem a mesma mensagem como pendente para ambos? Definir: a segunda recebe vazio); `CHATD_AS` com e sem flag explícita (flag vence); `refs` validadas (tipo, tamanho, teto); texto da campainha exato; testes independentes novos escritos só deste adendo.

---

# Contrato testável da v1.1 (normativo; as seções acima são a justificativa)
Onde este contrato e o texto acima divergirem, **vale este**. Tudo que a v1 (`docs/spec.md`) diz e não é alterado aqui continua valendo.

## A. Configuração (substitui a da v1, seção 7)
```json
{
  "auth": "required",
  "herdr_bin": "herdr",
  "bell_poll_ms": 5000,
  "bell_rearm_s": 600,
  "recipients": {
    "codex-web": {"delivery": "bell", "pane": "w2:p1"},
    "claude-main": {"delivery": "watch"}
  }
}
```
`auth`: `"required"` (padrão) ou `"off"` (comportamento da v1, sem `auth`, para testes e uso local sem token). `delivery`: só `"bell"` (exige `pane`) ou `"watch"` (proíbe `pane`). Campo `bell` da v1 deixa de existir (config antiga é recusada, saída 1). Campos desconhecidos continuam recusados.

## B. Autenticação (`auth: "required"`)
- **Emitir:** `chatd token new NOME --state-dir D [--out ARQ] [--rotate]`: gera 32 bytes aleatórios de `/dev/urandom`, token = 64 hex minúsculos. Grava em `D/auth.json` (0600, substituição atômica: temp + rename) apenas `{"v":1,"tokens":{"NOME":"<sha256 hex do token>"}}`. Entrega o token **uma vez**: com `--out ARQ`, grava o token em `ARQ` (0600, recusa se existir) e imprime só `ok NOME`; sem `--out`, imprime o token em `stdout`. `NOME` já com token: erro (saída 1) a menos que `--rotate`. `NOME` precisa casar `[a-z0-9_-]{1,32}`.
- `chatd token revoke NOME --state-dir D` remove; `chatd token list --state-dir D` imprime só nomes. O servidor **relê `auth.json` a cada `auth`** (revogação vale sem reiniciar).
- **Handshake:** primeira requisição da conexão = `auth {v, name, token}` → `{v, name}`. Qualquer outro método antes: erro `unauthenticated`. `name` ausente de `auth.json`, token errado ou formato inválido: o **mesmo** erro `unauthenticated` e a mesma mensagem (sem oráculo). Segundo `auth` na mesma conexão: `invalid_request`. Comparação do hash em tempo constante.
- **Trava:** 5 falhas de `auth` do mesmo `peer_pid` em 60 s fecham a conexão e recusam `auth` desse pid por 300 s (`unauthenticated`). Cada falha grava o evento `auth_failed` `{name?, peer_uid, peer_pid}` (nunca o token); sucesso **não** grava evento.
- **Par do socket:** `SO_PEERCRED`; uid do cliente diferente do uid do servidor = conexão fechada sem resposta.
- **Identidade imposta (a identidade é a da conexão):** `message.send`: `from` ausente = identidade; presente e diferente = `forbidden`. `message.list`, `message.take`: `to` ausente = identidade; diferente = `forbidden`. `message.ack`: `by` ausente = identidade; diferente = `forbidden` (e a regra da v1 de `by == to` da mensagem continua). `events.resume`: entrega só eventos em que a identidade é `from`, `to` ou `recipient`. `status`: devolve só a entrada da própria identidade.
- Com `auth: "off"` nada disto vale: `from`/`to`/`by` são obrigatórios como na v1 e o método `auth` não existe (`method_not_found`).

## C. Novos recursos
1. **`message.take {v, to?, limit?}`** (`limit` 1 a 200, padrão 50): em **uma seção crítica do servidor**, seleciona as mensagens `registrado` da identidade em ordem de `seq` (até `limit`), grava um evento `accepted` com `note:"take"` para cada uma e devolve `{v, messages:[Mensagem com corpo e state "aceito"], head_seq}`. Chamadas concorrentes nunca devolvem a mesma mensagem duas vezes. Sem pendentes: `messages: []` e nada gravado.
2. **`refs`** em `message.send`: lista opcional (até 16) de `{"t": "path|commit|msg|job|url", "v": TEXTO}`, `v` com 1 a 512 caracteres sem quebra de linha; fora disso `invalid_envelope` com `/refs/N/t` ou `/refs/N/v`. Guardada no evento `registered`, devolvida em `Mensagem.refs` (lista, vazia se ausente) e **incluída no hash** do envelope.
3. **Campainha (só `delivery: "bell"`)**: mesmas regras da v1 (seção 8); texto fixo e literal: `[chat] N pendente(s). Rode: chatd inbox --take`. O laço da campainha nunca roda para `watch`.
4. **`recipients.<nome>.delivery` = `watch`:** sem campainha; a entrega é o cliente `chatd watch`.

## D. Linha de comando v1.1 (substitui o que conflitar com a seção 4 da v1)
Variáveis de ambiente: `CHATD_AS` (nome), `CHATD_TOKEN_FILE` (caminho do token), `CHATD_SOCKET`, `CHATD_STATE_DIR`. **Flag explícita vence a variável.** O cliente faz o `auth` sozinho quando há token (`CHATD_TOKEN_FILE` ou `--token-file`); com `auth: required` e sem token: saída 2.
| Comando | Efeito |
| --- | --- |
| `chatd send [--from N] --to N --kind K --subject T (--body T \| --body-file A \| --body-file -) [--thread ID] [--id ID] [--ref t:v]...` | Imprime **uma linha** `ID SEQ` (e `dup` ao final se `duplicate`); `--json` imprime o JSON da v1 |
| `chatd inbox [--as N] [--after SEQ] [--all] [--take] [--ids] [--json]` | Padrão: pendentes (`registrado` e `aceito`), **com corpo**: por mensagem `SEQ ID KIND FROM>TO \| SUBJECT` e, se houver corpo, as linhas do corpo recuadas em 2 espaços; `--ids` só os ids; `--json` uma mensagem JSON por linha; `--take` usa `message.take` (marca `aceito`, só devolve o que acabou de tomar) |
| `chatd ack --id ID [--as N] --stage aceito\|resultado [--result R] [--note T]` | como na v1; imprime `ok` |
| `chatd watch [--as N] [--from-start]` | Fica ligado: imprime, a cada `registered` dirigido à identidade, **uma linha** `SEQ ID KIND FROM>TO \| SUBJECT` (`stdout` com flush por linha). Começa pelo `head_seq` atual (só novas), ou do começo com `--from-start`. Mantém a conexão viva enviando `status` a cada 15 s. Se a conexão cair, reconecta a cada 1 s com o último `seq` visto e **nunca imprime a mesma mensagem duas vezes**. Sai com 0 em SIGTERM/SIGINT |
| `chatd status [--json]` | como na v1 (só a entrada da identidade com auth) |
| `chatd log --state-dir D [--tail N]` | Lê `D/log.jsonl` direto e imprime uma linha por evento `SEQ TS TYPE ID FROM>TO \| SUBJECT` (campos ausentes omitidos); não usa o socket |
| `chatd token new\|revoke\|list` | como na seção B |
Erros do servidor continuam em `stderr` como `chatd: <code>: <message>`.

## E. Aceite da v1.1
Os testes independentes da v1 que dependem de `auth` rodam com `"auth":"off"`; novos testes (escritos só deste contrato) cobrem as seções B, C e D: handshake e `unauthenticated` (3 causas, mesma mensagem), `forbidden` em send/list/take/ack, `events.resume` filtrado, trava por pid, token revogado, permissões 0600 de `auth.json` e do `--out`, token nunca em `stdout`/`stderr`/log (exceto `token new` sem `--out`), `take` atômico sob concorrência (N clientes, 0 duplicatas), `refs` (limites, hash), texto da campainha, `watch` (linha por mensagem, reconexão sem duplicata, keepalive), `CHATD_AS` e precedência da flag, `log`. Mutantes: `scripts/mutacao.py` estendido para o código novo.

## F. Correções da auditoria adversarial (Fable, commit 68d40b2; normativo)
1. **Caracteres de controle (M1).** `message.send` recusa com `invalid_envelope` (`/subject`, `/body`, `/refs/N/v`) qualquer caractere de controle C0 (U+0000 a U+001F), C1 (U+0080 a U+009F) ou DEL (U+007F), **exceto** `\n` e `\t` **só no `body`** (o `body` aceita `\n` e `\t`; `\r` e os demais controles são recusados). `message.ack` aplica a mesma regra a `note` (aceita só `\t`; `\n` recusado). O cliente (`inbox`, `watch`, `log`, `send` ecoando) **escapa** qualquer controle remanescente como `\xNN` (ou `\u{NNNN}`) ao imprimir, de modo que nenhum byte de controle chegue ao terminal (cobre logs gravados antes da correção).
2. **Rastro de `from` forjado (B1).** `message.send`, `message.list`, `message.take` e `message.ack` recusados por `forbidden` gravam o evento `denied` `{method, peer_uid, peer_pid, claimed}` (`claimed` = o `from`/`to`/`by` pretendido, **nunca** assunto, corpo ou token). `chatd log` destaca esses eventos (`!!`). O tipo `denied` é aceito na releitura do log.
3. **Trava por pid (B2).** Autenticação bem-sucedida **limpa** o histórico de falhas e a trava do pid; entradas com janela expirada e sem trava são **podadas** a cada `auth`; o mapa tem teto de 4.096 entradas (acima disso, as mais antigas sem trava saem primeiro).
Testes novos (independentes) cobrem cada regra: controle em subject/body/note/ref.v (aceita `\n`,`\t` no body, recusa o resto), eco escapado no `inbox --take`, `watch` e `log`, evento `denied` e destaque no `log`, `auth` OK zerando falhas (4 falhas + sucesso + 1 falha **não** trancam), e a releitura do log com evento `denied`.
