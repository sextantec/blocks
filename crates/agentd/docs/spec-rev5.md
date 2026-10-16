# agentd v1 rev5: fonte `tmux` e regras de estado como dados | RASCUNHO rev5 (2026-10-16)

Origem: decisao A22 do Jabbur (2026-10-16, "foco unico em tmux", a fonte `herdr` e descartada), plano de leitura e laboratorio aprovados ("Aprovado", "B", "Estou de acordo com o A" para o Pi) e ordem de escrever esta spec ("Siga com o spec", 2026-10-16, conversa do Claude Code). Base de prova: medidas em tmux 3.7c isolado com Claude Code 2.1.301, Codex 0.162.1 e Pi 1.3.0 (relatorio `docs/medidas-tmux-2026-10-16.md`, telas reais em `rules/fixtures/`). Esta spec **completa e em parte substitui** a `spec.md` (rev4); os testes falam com o **binario** e com o **protocolo no socket**, nunca com a API interna.

## 0. Relacao com a rev4 (leia primeiro)
| Parte da rev4 | Na rev5 |
|---|---|
| Secoes 0, 1, 2 (filosofia, observador, nao faz) | **Mantidas.** A secao 0 vale com mais forca: estado observado nao e autorizacao de digitar. |
| Secao 3 (modelo), regras 0, 1, 2, 4, 6 da secao 4 | **Mantidas**, com os acrescimos da secao 3 desta spec (campos `rule`, `input`; marcas `background` e `unmatched`; `kind` ganha `pi`). |
| Regra 3 (painel reaproveitado) e regra 5 (confirmacao de `idle`) | **Reescritas** para tmux (secao 5 desta spec). |
| Secao 5 (protocolo), 8 (CLI), 9 (seguranca), 13, 14 (`since`, `blocked_reason`) | **Mantidas**; acrescimos nas secoes 3, 8 e 9 desta spec. |
| **Secao 6 (fonte `herdr`) e a secao 7 (configuracao)** | **Substituidas** pelas secoes 4 e 7 desta spec. O `herdr` nao e mais lido, chamado nem aceito na configuracao. |
| **Rev5.1 (2026-10-16 tarde): modo de eventos** | A secao 4A acrescenta um **gatilho por eventos** (cliente de controle do tmux, so leitura) que antecipa o ciclo e da quietude por painel; o ciclo de `list-panes` + `capture-pane` continua sendo a unica fonte de estado. Sem eventos, tudo vale como na rev5 (polling). Decisao do Jabbur: tmux padrao, sem patch (nota: `tmux-patch-candidato.md`). |
| **Rev5.3 (2026-10-16 tarde): descoberta de agentes** | A secao 16 deixa o `agentd` achar sozinho os paineis que rodam `claude`, `codex` ou `pi` (opt-in, `discover`), sem marca; o nome vem da marca `@agentd`, da configuracao ou do diretorio. `list-panes` passa a ter 10 campos. Decisao do Jabbur ("Siga com o A", 2026-10-16). |
| Secao 10 (testes G1 a G22) | Os testes que dependem do `herdr` falso (G1, G2, G7, G8, G11, G15, G16, G17, G19, G21, G23 da secao 10) **sao reescritos** como R1 a R20 (secao 11); os de protocolo, CLI, reinicio, atomicidade e assinante lento (G5, G6, G9, G10, G12, G14, G20, G22) continuam, trocando apenas a fonte falsa. `since`/`blocked_reason` (G23 a G29 da secao 14) continuam, com a emenda da secao 6 desta spec. |

## 1. Problema, exemplo, solucao
**Problema.** O estado de um agente so existe na tela dele. O herdr e o Luvus a raspam e, quando nenhuma regra casa, devolvem `idle` (`manifest.rs:550-553`, `detect.rs:1301-1305`). As medidas mostraram quatro formas de isso dar errado, todas reais:
1. **Falso `idle` no Codex:** em cada resposta, por cerca de 1,8 s o indicador `Working ... esc to interrupt` some da tela e a tela fica igual a de ocioso; so o **titulo do painel** (spinner braille) denuncia o trabalho.
2. **Dialogo sem a frase esperada:** o dialogo de aprovacao de plano do Claude **nao tem** `Esc to cancel`; uma regra so por essa frase o perde.
3. **Caixa "vazia" que nao esta:** o Claude mostra uma **sugestao fantasma** (`❯ ls | wc -l`) e o Codex um placeholder (`Ask Codex to do anything`) na caixa de entrada sem ninguem ter digitado; ambos em atributo *dim*. Rascunho humano e atributo normal.
4. **Frases que envelhecem:** as frases do visualizador de transcricao do Codex 0.162.1 ja nao casam com as do herdr 0.9.5.

**Solucao.** O `agentd` passa a ler o tmux (so `list-panes` e `capture-pane`), aplica **regras de estado que sao dados** (um arquivo JSON, cada regra com tela real de prova, verificavel offline por `agentd rules check`), publica `idle` **somente com prova positiva**, trata agente conhecido sem regra casando como `unknown` (nunca `idle`) e classifica a caixa de entrada (`empty | draft | unknown`) sem copiar texto algum.

**Exemplo (Codex, um turno).** t=0 s caixa vazia, titulo `work-codex`: `idle`. t=0,5 s o titulo ganha `⠼ ` e a tela `Working (0s • esc to interrupt)`: `working` imediato. t=13,7 s o texto flui, a tela perde `esc to interrupt` e parece ociosa, mas o titulo ainda tem o spinner: continua `working`. t=15,5 s o spinner some e a tela e ociosa: primeira leitura `idle` (publicado: ainda `working`, marca `idle_unconfirmed`); 2,5 s depois, sem saida nova no painel, `idle` confirmado. Se, no meio, aparece o titulo `[ . ] Action Required | ...`, o estado vira `blocked` com `blocked_reason: permission`, na hora.

## 2. Principios (invariantes desta spec)
1. **Na duvida, `unknown`; `idle` so por prova positiva** de uma regra `idle` que casa **e** nenhuma regra `blocked` ou `working` casa na mesma leitura.
2. **Precedencia fixa por seguranca: `blocked` > `working` > `idle`** (nao por numero de prioridade). Empate dentro do mesmo estado resolve pela **ordem no arquivo**.
3. **Nenhum texto da tela, do titulo ou do stderr sai do daemon**: so classes enumeradas, identificadores de regra (escritos pelo operador, nunca extraidos da tela) e codigos fixos.
4. **Somente leitura:** o daemon nunca digita, nunca muda o tmux, nunca cria painel. A entrega de mensagem (enviar teclas) e outro programa e fica fora da v1 (secao 12).
5. **A tela e a unica fonte de estado.** Hooks dos agentes nao entram: nao cobrem `Esc` (UserPromptSubmit sem Stop), o aviso de bloqueio chega 6 s depois e permissao e pergunta sao indistinguiveis. A classe do bloqueio (`permission`/`user_input`) sai da propria tela (secao 6).

## 3. Modelo (acrescimos a secao 3 da rev4)
- **`kind`:** `claude | codex | pi`. Cada `kind` precisa existir no arquivo de regras.
- **`Agente` ganha dois campos** (aditivos, nulos quando nao se aplicam):
  - `rule`: **identificador** da regra que decidiu o estado desta leitura (string do arquivo de regras) ou `null` (sem regra casando, agente ausente, leitura falha). Serve para diagnostico; **nao gera evento**.
  - `input`: classe da caixa de entrada na ultima leitura valida: `"empty" | "draft" | "unknown"`, **somente quando o estado publicado e `idle`**; em qualquer outro estado e `null`. **Nao gera evento** (digitar nao pode inundar o anel). Ver secao 6.
- **Marcas** (conjunto): alem de `error`, `stale`, `idle_unconfirmed`:
  - `unmatched`: leitura valida, agente identificado, **nenhuma regra casou** (estado `unknown` **sem** `error`);
  - `background`: uma regra de marca casou (no Claude: subagente em segundo plano com o agente principal livre). Nao muda o estado: o principal continua `idle`.
- **Evidencia/fonte:** `evidence: observed`, `source: "tmux"` em toda leitura valida.
- **`blocked_reason`** (secao 14 da rev4) deixa de ser sempre `unknown`: vem da regra `blocked` que casou (`permission | user_input | unknown`). Mudar so a classe com o estado `blocked` mantido **gera evento**.
- **Leitura valida (substitui a da rev4):** o `list-panes` saiu 0, dentro do limite e com todas as linhas no formato da secao 4; o painel do agente aparece **uma unica vez**; esta vivo, o processo em primeiro plano e do `kind` (secao 4) e o `capture-pane` saiu 0 dentro do limite. Qualquer outra e **leitura falha** ou **painel ausente**, conforme a secao 4.

## 4. Fonte `tmux` (a unica da v1; substitui a secao 6 da rev4)
- **Uma fonte por agente**, exatamente (`sources` com um elemento), `{"type":"tmux","pane":"%N"}` **ou** `{"type":"tmux","tag":"NOME"}`:
  - `pane`: o `pane_id` do tmux (`%` e digitos). Estavel enquanto o servidor tmux vive; muda depois de reinicio do tmux.
  - `tag`: casa com a opcao de painel `@agentd` (`tmux set-option -p -t PAINEL @agentd NOME`). Preferivel: sobrevive a reinicios. `NOME` em `[a-z0-9_-]{1,32}`. Zero painel com a tag = **painel ausente**; mais de um = leitura falha **so desse agente** (`pane_ambiguous`).
- **Processos (lista fechada, sem shell, `Command` com `argv`):** o daemon chama so
  1. `TMUX -S SOCKET list-panes -a -F FORMATO` (uma vez por ciclo);
  2. `TMUX -S SOCKET capture-pane -p -e -J -t %N` (uma vez por agente cujo painel esta presente, vivo e identificado, em serie, depois do `list-panes`).

  `-S SOCKET` so aparece se `tmux_socket` esta configurado; `TMUX` e `tmux_bin`. Qualquer outro subcomando (`send-keys`, `new-*`, `kill-*`, `set-*`, `display*`, `resize-*`, `attach`, `respawn-*`, `pipe-pane`, `source-file`, `run-shell`...) e **proibido e coberto por teste** com tmux falso que registra as chamadas. O daemon **nunca** captura um painel que nao esta configurado nem um painel cujo processo em primeiro plano nao e do `kind`.
- **Formato do `list-panes`** (campos separados por TAB; `pane_title` por ultimo, pois o texto pode ser qualquer coisa):
  `#{pane_id}<TAB>#{pane_pid}<TAB>#{pane_dead}<TAB>#{pane_dead_status}<TAB>#{window_activity}<TAB>#{pane_current_command}<TAB>#{session_id}<TAB>#{@agentd}<TAB>#{pane_title}`.
  Linha com menos de 9 campos, `pane_id` fora de `%[0-9]+`, `session_id` fora de `$[0-9]+`, `pane_pid`/`window_activity` nao numericos ou `pane_dead` fora de `0|1` => **leitura toda falha** (`source_bad_output`). `pane_id` repetido => `duplicate_pane`. Campos extras no fim sao absorvidos pelo titulo. O titulo e usado **somente** pelas regras (secao 6) e descartado; nunca guardado.
- **Estado do painel por agente, nesta ordem:**
  1. nao esta na lista, ou `pane_dead` e `1`, ou `pane_current_command` **nao esta** em `identify.commands` do `kind` => **painel ausente** (rev4, regra 1b: `unknown` com `error` ate `gone_after_s`, entao `gone`). Motivo: o agente saiu e sobrou o shell (`zsh`); a tela de um shell **nunca** e lida.
  2. caso contrario, captura-se a tela.
- **Captura e analise:** a saida do `capture-pane -p -e -J` e dividida em linhas. Em cada linha, as sequencias SGR (`ESC [ P;P;... m`) sao interpretadas **parametro a parametro**: `0` zera tudo, `2` liga *dim*, `22` desliga *dim*, `7` liga *video reverso*, `27` desliga; `38`, `48` e `58` consomem os parametros da cor (`5;N` ou `2;R;G;B`) e sao ignorados; todo outro parametro (inclusive `1`, `39`, `49`) e ignorado. O estado SGR **recomeca zerado em cada linha** (nao ha SGR persistente entre linhas). Qualquer outra sequencia de escape e descartada. O texto plano resultante tem **U+00A0 trocado por espaco** (o Claude escreve `❯` seguido de U+00A0) e os espacos finais de cada linha removidos. Saida de `capture-pane` acima de **256 KiB** => falha **so desse agente** (`source_too_large`). Se a saida nao tiver SGR (fixture em texto plano), nenhum caractere e dim nem reverso.
- **Processo:** no maximo **um** em voo; `source_timeout_ms` (padrao 3000) por processo: ao estourar, `SIGKILL` no grupo e espera da coleta antes do proximo; stdout do `list-panes` limitado a 1 MiB; stderr lido e **descartado** (ate 64 KiB), nunca guardado.
- **`observed_at`:** instante em que o processo `list-panes` do ciclo foi iniciado (conservador, como na rev4).
- **Codigos de falha (so estes aparecem em `status`/logs):** `source_spawn_failed`, `source_timeout`, `source_exit_nonzero` (inclui "no server running": servidor tmux fora e **leitura falha**, nunca `gone`), `source_too_large`, `source_bad_output`, `duplicate_pane`, `pane_ambiguous`, `capture_failed`. `status.sources.tmux = {ok, last_ok_at, last_error}` (`ok` = a **execucao** do `list-panes` deu certo).
- **Identificacao medida (comando em primeiro plano):** Claude `claude`, Codex `codex`, Pi `pi` (o Pi mostrou `node` por ~1 s na partida: nesse intervalo o painel conta como ausente e volta). Se um agente for lancado por um caminho que mude o nome (por exemplo o Claude como `2.1.301` quando chamado pelo binario versionado [estimativa, nao medida]), a lista `identify.commands` do arquivo de regras e o lugar de acrescentar o nome.

## 4A. Modo de eventos (gatilho; rev5.1)
**Problema.** Com polling a cada 2 s, um dialogo de permissao leva em media 1 s (ate 2 s) para ser visto, e a quietude do `idle` so tem resolucao de 1 s e e da **janela**, nao do painel.
**Exemplo.** Claude abre `Do you want to create ola.txt?` 4 s depois do prompt; com 2 s de polling o `blocked` aparece 0 a 2 s depois; com eventos, ~0,17 s depois (medido, abaixo).
**Solucao.** O tmux avisa quando um painel produz bytes. O `agentd` usa esse aviso **so como gatilho** e como relogio de quietude; **o estado continua vindo da tela capturada** (nao se reconstroi tela a partir dos bytes).

- **Processo adicional (lista fechada, acrescimo a secao 4):** um por sessao tmux que contem painel configurado (ate 8), descoberta pelo campo `session_id` do `list-panes`: `TMUX [-S SOCKET] -C attach -f ignore-size -t '$N'`, stdin em pipe **mantido aberto e nunca escrito** (o `agentd` **nao envia nenhum comando** por esse canal), stdout lido linha a linha (linha acima de 1 MiB = cliente de eventos falho). So importa a linha `%output %N ...`: o `agentd` le o `pane_id`, **descarta o conteudo imediatamente** (nunca guardado, logado nem analisado) e registra `ultima_saida[%N]` (relogio monotonico). Todas as outras linhas `%...` sao ignoradas. **Nunca** usar `-r`: medido no tmux 3.7c, um cliente `-r` anexado faz o `send-keys` de **outro** processo (o `wakerd`) falhar com `client is read-only`, e `-r` **nao** impede `set-option` nem `kill-pane` (o codigo so bloqueia teclas, `switch-client`, `attach`, `detach`); a garantia de "so leitura" e o `agentd` nunca escrever no canal. `-f ignore-size`: o tamanho da janela nao mudou (medido, 120x30 antes e depois).
- **Gatilho:** `%output` de painel configurado => marca o painel como *sujo* e agenda um ciclo para `event_debounce_ms` (padrao 150) depois do **primeiro** byte novo; novos bytes nao adiam (no maximo um ciclo por `event_debounce_ms`). Um ciclo e o de sempre (`list-panes` uma vez; `capture-pane` por agente), mas o `capture-pane` so roda para painel **sujo**, em **janela de confirmacao de `idle`** ou no **ciclo de seguranca** (`safety_poll_ms`, padrao 5000, todos os agentes); os demais reaproveitam a ultima tela (sem saida, a tela nao mudou) com o titulo novo do `list-panes`.
- **Confirmacao de `idle` (5') com eventos:** a primeira leitura `idle` agenda uma nova leitura para `idle_confirm_ms` depois; a condicao (ii) passa a ser: **nenhum `%output` do painel** entre o inicio da primeira e o fim da ultima leitura, alem de mesmo par e mesma `rule`. Saida no meio zera a janela.
- **Degradacao:** cliente de eventos morto (`%exit`, EOF, falha ao subir, linha longa demais), sessao que sumiu ou `events: false` => modo **polling** da rev5 (`poll_ms`, quietude por `window_activity`), `status.sources.tmux.events = "degraded"` (ou `"off"`), religacao com recuo 1, 2, 4... ate 30 s. Nada no estado publicado depende de o cliente de eventos estar vivo: so a latencia e a precisao da quietude pioram.
- **Efeitos colaterais conhecidos (instalacao):** o cliente de controle aparece em `list-clients` e conta em `#{session_attached}`; hooks `client-attached` do proprio usuario podem rodar (**nao medido**). Limite do tmux: `CONTROL_MAXIMUM_AGE` e 300 s de atraso antes de derrubar o cliente (a leitura do `agentd` nao espera nada alem de uma linha, entao nao ha risco pratico).
- **Medido (tmux 3.7c, laboratorio isolado, 1 turno por agente, Claude 2.1.301/Haiku 5.5, Codex 0.162.1/gpt-6-luna, Pi 1.3.0; cliente de teste em Python, nao o `agentd`):**
  - Latencia do primeiro byte novo ate a captura pronta (debounce 150 ms + laco de 50 ms do cliente de teste): p50 161 a 170 ms, maximo 197 ms; o `capture-pane` em si 0,4 a 1,0 ms (tela 200x50).
  - Volume em `%output`: ocioso, Codex e Pi **0 byte em 45 s**; Claude 3 eventos em 45 s (o texto de exemplo `Try "..."` da caixa troca a cada 8 s nos primeiros ~25 s: pode zerar a janela de confirmacao e atrasar o `idle` por mais uma janela); trabalhando: Claude 1,6 KB/s (9 eventos/s), Codex 3 KB/s (65 eventos/s), Pi 9 KB/s (13 eventos/s, pico 11 KB/s no Codex e 9 KB/s no Pi).
  - CPU do servidor tmux: 0,1 a 0,2 % de um nucleo durante as execucoes de ~25 s.
  - Os tres agentes usam *synchronized output* (`ESC[?2026h`) em cada redesenho; nenhum emitiu progresso OSC 9;4 (`#{pane_pb_state}` = `hidden` sempre), BEL nem OSC 133: **nao ha canal declarado gratuito**.
  - Variavel de formato desconhecida em `list-panes -F` devolve campo vazio (sem erro): base do fallback caso um dia se adote o patch candidato.
  - Nao medido: varios turnos longos, saida muito grande, varios agentes ao mesmo tempo, custo do `agentd` em Rust.

## 5. Regras de estado (emendas as regras 3 e 5 da rev4; as demais valem como estao)
3'. **Painel reaproveitado:** o par (`pane_id`, `pane_pid`) da ultima leitura valida e lembrado. Se mudar, a janela de confirmacao e zerada, sai um evento e o estado fica `unknown` com `error` ate uma leitura valida do novo par (e `idle` volta pela regra 5'). Se `pane_pid` e o mesmo mas o agente reiniciou dentro do mesmo shell, o painel passou por "ausente" (shell em primeiro plano) ou a tela mudou (a regra 5' zera a janela por `window_activity`).
5'. **Confirmacao de `idle` observado (anti-oscilacao):** **toda** transicao para `idle` publicado, de qualquer estado anterior (inclusive `unknown`, `error`, `stale`, `gone`, partida), exige **tres condicoes juntas** em leituras validas: (i) `idle_confirm_polls` leituras consecutivas `idle` (padrao 2); (ii) o **mesmo par** (`pane_id`, `pane_pid`), o **mesmo `window_activity`** e o **mesmo `rule`** (identificador da regra `idle` que casou) em todas elas; (iii) pelo menos `idle_confirm_ms` entre a primeira e a ultima (padrao 2500). **Qualquer outra leitura** (falha, ausente, nao-`idle`, `unknown`, par, `window_activity` ou `rule` diferente) **zera** a janela. Enquanto nao confirmado, o estado publicado e o anterior (ou `unknown` se o anterior era `unknown`/`gone`) com a marca `idle_unconfirmed`. Entrar em `working` ou `blocked` e imediato. **Com o modo de eventos ativo (secao 4A), a condicao (ii) usa a quietude por painel (nenhum `%output` na janela) no lugar do `window_activity`, que fica so como fallback.** `window_activity` (segundos Unix da ultima saida da janela) faz o papel do `state_change_seq` do herdr: **qualquer saida no painel durante a janela impede a confirmacao**; num tmux com varios paineis na mesma janela a saida de outro painel tambem impede (so atrasa, nunca antecipa). Limite honesto: confirma **estabilidade amostrada com resolucao de 1 s**, nao quietude comprovada; por isso `idle_confirm_ms` recomendado e >= 2000.

## 6. Regras de estado como dados (arquivo de regras)
Arquivo JSON (`rules_file` da configuracao; exemplo normativo inicial: `crates/agentd/rules/default.json`, Anexo A). Campo desconhecido, **chave duplicada**, regex invalida, `kind` da configuracao sem entrada, regra sem fixture ou arquivo gravavel por grupo/outros (`mode & 0o022`) => o daemon **nao sobe** (saida 1) e `agentd rules check` falha.

```json
{
  "schema": 1,
  "agents": {
    "claude": {
      "tested_versions": ["2.1.301"],
      "identify": {"commands": ["claude"]},
      "rules": [
        {"id": "claude.blocked.permission", "state": "blocked", "reason": "permission",
         "all": [{"in": "tail:12", "re": "^\\s*Do you want to .+\\?\\s*$"}, {"in": "tail:12", "re": "Esc to cancel"}],
         "fixtures": ["claude/permission-bash", "claude/permission-write"]}
      ],
      "marks": [{"id": "claude.mark.background", "mark": "background", "all": [...], "fixtures": [...]}],
      "input": {"between": "^─{20,}$", "strip_prefix": "^(?:❯|›) ?"}
    }
  },
  "golden": [
    {"fixture": "claude/permission-bash", "kind": "claude",
     "expect": {"state": "blocked", "reason": "permission", "rule": "claude.blocked.permission", "input": null, "marks": []}}
  ]
}
```
- **Regra de estado:** `id` unico `[a-z0-9._-]{1,64}`; `state` em `blocked|working|idle`; `reason` obrigatoria em `blocked` (`permission|user_input|unknown`) e proibida nos outros; `all` (lista nao vazia de condicoes que **todas** precisam valer); `none` (opcional: condicoes que **nenhuma** pode valer); `fixtures` (lista nao vazia de nomes de tela que a regra deve **casar**). A regra casa quando todas as de `all` valem e nenhuma de `none` vale.
- **Condicao:** `{"in": REGIAO, "re": REGEX, "min": N}`; `REGIAO` = `screen` (todas as linhas), `tail:N` (as ultimas N linhas **nao vazias**, `N` de 1 a 64) ou `title` (o titulo do painel); `re` e uma expressao regular **sem retrocesso** (crate `regex`, Unicode) testada **linha a linha** (nunca atravessa linhas); `min` (padrao 1) = quantas linhas da regiao precisam casar.
- **Avaliacao por leitura:** (1) avaliam-se todas as regras de estado do `kind`; (2) se alguma `blocked` casa, o estado e `blocked` e a `reason` e `rule` vem da **primeira** `blocked` que casou na ordem do arquivo; senao, se alguma `working` casa, `working`; senao, se alguma `idle` casa, `idle`; senao **nenhuma casou**: `unknown` com a marca `unmatched`, `rule: null`. (3) avaliam-se as **regras de marca** (`marks`, mesma sintaxe de condicao, `mark` em `background`) e as que casam entram nas marcas da leitura.
- **Classificacao da caixa de entrada (`input`)** — calculada so quando o estado da leitura e `idle`:
  - `between`: regex de linha-regua. Pega-se as **duas ultimas** linhas da tela que casam; a caixa sao as linhas **estritamente entre** elas. Se houver menos de duas reguas, `input = "unknown"`.
  - `line` (alternativa ao `between`, para o Codex): `{"line": {"in": "tail:6", "re": "^›"}}`: a caixa e a **ultima** linha da regiao que casa. Sem linha, `"unknown"`.
  - `strip_prefix` (opcional; o Pi nao tem prefixo): regex aplicada ao inicio de cada linha da caixa; o trecho casado (prefixo `❯ ` ou `› `) sai da analise.
  - **`draft`** se, depois do prefixo, existe **ao menos um caractere nao branco que nao e dim e nao e video reverso**; **`empty`** se a caixa tem so brancos, texto dim (sugestao fantasma do Claude, placeholder do Codex) e/ou o cursor em video reverso (Pi); `unknown` se o localizador falha. Linhas de continuacao de rascunho com varias linhas contam no `between`; no `line` so a primeira linha conta (limite declarado). Texto plano sem SGR (fixture sem atributos) nunca e dim: um placeholder em fixture `.txt` simples sai `draft`; por isso as fixtures de `input` sao gravadas com `-e` (`.ansi`).
  - Resultado e **so a classe**: nenhum caractere da caixa e guardado ou publicado.
- **Consequencia de seguranca (repetida de proposito):** `input: "empty"` e uma observacao de **ate um ciclo atras**. Quem for digitar (outro programa) tem que reler a tela imediatamente antes e depois do envio; o `agentd` nao fecha essa corrida (secao 0 da rev4).
- **`golden.expect`:** `state` e `rule` sempre; `reason` em `blocked`; `marks` (lista; `["unmatched"]` quando `unknown`); `input` **so quando a fixture e `.screen` gravada com `-e`** (fixture em texto plano nunca tem atributo dim: um placeholder em texto plano sairia `draft`, entao a `golden` dela omite `input`). Chave ausente = nao verificada.
- **Teste offline das regras:** `agentd rules check --rules ARQ --fixtures DIR [--json]` (secao 8) executa todas as entradas `golden` e, para cada regra, as suas `fixtures`, e verifica tambem que **toda regra tem ao menos uma fixture que a faz casar e ao menos uma `golden` de outro estado que ela nao altera**. E o procedimento quando um agente atualiza: gravar a tela nova (Anexo B), acrescentar a fixture e rodar o check.

### Emenda a secao 14 da rev4 (G26)
`blocked_reason` agora vem da regra: em `blocked` e `"permission" | "user_input" | "unknown"` conforme a regra; em qualquer outro estado e `null`. Os testes que esperavam sempre `"unknown"` passam a esperar a classe da regra (`R1`).

## 7. Configuracao (substitui a secao 7 da rev4)
```json
{
  "tmux_bin": "tmux",
  "tmux_socket": null,
  "rules_file": "/home/USUARIO/.config/agentd/rules.json",
  "poll_ms": 2000,
  "events": true,
  "event_debounce_ms": 150,
  "safety_poll_ms": 5000,
  "source_timeout_ms": 3000,
  "stale_after_ms": 30000,
  "gone_after_s": 120,
  "idle_confirm_polls": 2,
  "idle_confirm_ms": 2500,
  "ring": 4096,
  "agents": {
    "claude":       {"kind": "claude", "sources": [{"type": "tmux", "tag": "claude"}]},
    "codex-blocks": {"kind": "codex",  "sources": [{"type": "tmux", "tag": "codex-blocks"}]},
    "pi":           {"kind": "pi",     "sources": [{"type": "tmux", "pane": "%7"}]}
  }
}
```
Campo desconhecido e **chave duplicada** recusados. Novos: `events` booleano (padrao `true`; `poll_ms` e o intervalo quando `events` e `false` ou degradado), `event_debounce_ms` 50 a 1000, `safety_poll_ms` 1000 a 60000 e >= `poll_ms`. Limites da rev4 (`poll_ms` 200 a 10000, `source_timeout_ms` 200 a 30000, `stale_after_ms` > `poll_ms` + `source_timeout_ms`, `gone_after_s` 5 a 86400, `idle_confirm_polls` 1 a 5, `idle_confirm_ms` 0 a 30000, `ring` 64 a 65536, ate 32 agentes) valem; recusa tambem `id` repetido, `pane` ou `tag` repetidos entre agentes, `kind` fora de `claude|codex|pi` ou sem regras, `type` diferente de `tmux` (**`herdr` e recusado**), `sources` com tamanho diferente de 1, `pane` fora de `%[0-9]+`, `tag` fora de `[a-z0-9_-]{1,32}`, `tmux_bin` ou `rules_file` vazios, `rules_file` ausente. `tmux_socket` e `null` (padrao do tmux) ou caminho absoluto.

## 8. CLI (acrescimos a secao 8 da rev4)
- `agentd rules check --rules ARQ --fixtures DIR [--json]`: offline, nao fala com o daemon nem com o tmux. Sai 0 se todas as `golden` e `fixtures` passam, 1 se alguma falha (a saida diz **qual fixture, qual regra e esperado vs obtido**, sem imprimir o conteudo da tela), 2 uso incorreto.
- `agentd get/list --json` trazem `rule` e `input`; sem `--json` a linha por agente nao muda (`ID ESTADO[,MARCAS] EVIDENCIA FONTE idade=Ns`).
- `agentd health`: sai 0 so se o daemon responde e `sources.tmux.ok` e verdadeiro.

## 9. Seguranca e privacidade (acrescimos)
- Alem da secao 9 da rev4: o daemon **nao escreve** no tmux (lista fechada, secao 4); **nunca captura** painel nao configurado ou sem agente; `rules_file` e dado do operador, validado e nao gravavel por grupo/outros; **o texto da tela e do titulo e usado so em memoria para casar regras e descartado**.
- **Nada da tela vira saida:** `rule` e identificador do arquivo, `input` e uma classe, `blocked_reason` e uma classe. Sentinelas na tela, no titulo, no stderr do tmux e no cwd nunca aparecem em `agent.*`, `status`, eventos, CLI nem log (`R5`).
- **Entrada hostil.** Conteudo que o proprio agente escreve na tela (por exemplo, um modelo induzido a imprimir "Esc to cancel") pode forcar `blocked` ou `working` (lado seguro). Para forcar `idle` seria preciso sumir com o sinal genuino de trabalho (o rodape `esc to interrupt`, o spinner do titulo ou a regua `Working`), que o agente nao controla pelo texto que imprime; **isto nao esta provado**, e o `idle` continua sendo "permitido com aviso" (secao 0 da rev4). Um processo do mesmo uid que escreva no painel ou altere `@agentd` e `pane_title` pode influenciar o estado: mesmo uid nao e isolamento.

## 10. Configuracao do tmux e do ambiente (nao normativa; para quem instala)
- `set -g extended-keys on` (o Pi avisa "Modified Enter keys may not work" sem isso).
- `set -g remain-on-exit on` ou `failed`: preserva o painel morto com `pane_dead_status` (o daemon trata painel morto como ausente; a opcao so ajuda a ver a saida).
- Marcar cada painel de agente: `tmux set-option -p -t PAINEL @agentd NOME` (ou lancar com `new-window` e marcar em seguida).
- Latencia: um ciclo e 1 `list-panes` + 1 `capture-pane` por agente sujo (algumas dezenas de KiB); com eventos a deteccao leva ~0,2 s; sem eventos, `poll_ms` 2000 e confortavel.
- Nao use `-r` em nenhum cliente de controle que fique anexado: o `send-keys` de outros processos passa a falhar (secao 4A).

## 11. Contrato testavel (testes independentes; o `tmux` e FALSO: script controlado que serve `list-panes` e `capture-pane` a partir de um diretorio de cenario, com telas das `rules/fixtures/`, e registra cada chamada)
- **R1** golden: para cada `golden` do arquivo de regras, o tmux falso serve a tela (e o titulo) e, apos o numero de ciclos necessario, `agent.get` traz `state`, `blocked_reason`, `rule`, `input` e `marks` esperados; `working`/`blocked` na primeira leitura; `idle` so apos confirmacao.
- **R2** `unknown`: tela que nenhuma regra casa (texto qualquer, tela vazia, tela de outro programa) => `unknown`, marca `unmatched`, sem `error`, `rule: null`, **nunca `idle`**; falhas: saida nao zero, saida invalida (`source_bad_output`), `capture-pane` falhando so de um agente (o outro segue), tela > 256 KiB, stdout do `list-panes` > 1 MiB, `pane_id` duplicado, `tag` ambigua (`pane_ambiguous`), servidor tmux fora (`source_exit_nonzero`, nunca `gone`).
- **R3** lista fechada: o tmux falso registra por 10 ciclos **apenas** `list-panes -a -F ...` (uma vez por ciclo) e `capture-pane -p -e -J -t %N` (uma vez por agente presente e identificado); jamais `send-keys` nem outro subcomando (a unica excecao e o processo persistente `-C attach -f ignore-size -t $N` da secao 4A, cujo stdin **nunca recebe byte**); jamais captura painel nao configurado, morto ou com shell em primeiro plano; nunca dois processos em voo.
- **R4** sem shell: `tag`, `pane`, `tmux_bin`, `tmux_socket` com `;`, `$()`, espacos e aspas nao executam nada.
- **R5** privacidade: sentinela unica na tela, no titulo, no stderr do tmux falso, na saida invalida e no cwd nunca aparece em `agent.*`, `status`, eventos, CLI nem log; `rule` mostra so ids do arquivo.
- **R6** confirmacao de `idle` (5'): leitura `idle` isolada entre `working` nao muda o publicado (`idle_unconfirmed`); duas consecutivas **com mesmo par, mesma quietude (sem `%output` do painel; `window_activity` so no fallback), mesma `rule` e `idle_confirm_ms` decorrido** confirmam; `window_activity` que muda em qualquer leitura, par que muda, `rule` que muda, erro, ausencia, `unknown` ou nao-`idle` no meio **zeram**; `working` e `blocked` imediatos; `idle_confirm_polls:1` com `idle_confirm_ms:0` e imediato. **Falso idle do Codex:** sequencia de telas parecidas com ociosas mas com spinner no titulo => `working` o tempo todo; fim do spinner => `idle` so apos a janela.
- **R7** caixa de entrada: tela de cada agente com caixa vazia, rascunho, **sugestao fantasma do Claude e placeholder do Codex (`.ansi`, dim)**, cursor invertido do Pi, rascunho de varias linhas (Pi) => `empty | draft`; localizador que falha (menu aberto) => `unknown`; estado diferente de `idle` => `input: null`; mudar so `input` **nao** gera evento.
- **R8** marca `background`: Claude com subagente em segundo plano e principal livre => `idle` com `background`; sem a lista de agentes a marca sai; a marca nao muda o estado; mudar so a marca gera evento.
- **R9** precedencia: telas que casam regras de mais de um estado (dialogo + `esc to interrupt`; titulo `Action Required` com spinner) => `blocked` > `working` > `idle`; duas `blocked` que casam => a `reason` da primeira do arquivo.
- **R10** painel: `pane_pid` muda => evento, `unknown,error`, janela zerada; painel some, morre (`pane_dead=1`) ou volta ao shell (`zsh`) => ausente: `unknown,error` ate `gone_after_s` (inclusivo) e entao `gone`; lista vazia apos erros nao produz `gone` imediato; `gone` volta com evento.
- **R11** arquivo de regras: campo desconhecido, chave duplicada, regex invalida, `kind` sem entrada, regra sem fixture, `reason` ausente em `blocked` ou presente em outro estado, `mode 0666` => nao sobe (saida 1); `agentd rules check` saida 0/1/2, mensagem sem conteudo de tela, e **falha se uma regra nao tem fixture que a case**.
- **R12** configuracao: `type: herdr` recusado; `kind` fora da lista; `pane`/`tag` invalidos ou repetidos; `rules_file` ausente; limites da rev4.
- **R13** `stale` e `gone` e limites exatos (G3, G4, G18 da rev4) refeitos sobre o tmux falso: leitura travada => `unknown,error`, depois `stale` com `last_known`; no limite exato de `stale_after_ms` nao, de `gone_after_s` sim.
- **R14** atomicidade, protocolo, assinante lento, reinicio e CLI (G5, G6, G9, G10, G12, G14, G20, G22 da rev4) com a fonte falsa trocada; `since`/`blocked_reason` (G23 a G29 da secao 14) com a emenda acima.
- **R15** regressao dos sinais medidos (uma por linha, cada uma com a tela real): Claude diálogo de **plano sem `Esc to cancel`** => `blocked`/`permission`; menu `/model` => `blocked`/`user_input`; pergunta ao usuario => `blocked`/`user_input`; confianca de pasta => `blocked`/`permission`; Claude em 60 colunas trabalhando => `working`; Codex dialogo `[ . ]` e `[ ! ]` => `blocked`; Codex `Update available` => `blocked`; Codex visualizador de transcricao => `blocked`; Pi `Working` em 50 colunas => `working`; Pi seletor `/model` => `blocked`.
- **R16** deriva de versao: com as telas gravadas de **outra versao** (`tested_versions` nao inclui) o resultado e `unknown`/`unmatched` quando nenhuma regra casa, **nunca `idle`**.
- **R17** janela do Enter no Claude (limite 1 do Anexo A): tela de rascunho/eco logo apos o envio seguida, uma leitura depois, da tela `working` => o estado publicado nunca e `idle`; `idle` vindo de `working` com a mesma tela de rascunho estavel por `idle_confirm_ms` confirma (e `input: draft`).
- **R18** eventos antecipam o ciclo: com cliente de eventos falso que emite `%output %7` para o painel do agente, o ciclo roda em `event_debounce_ms` (+- tolerancia) e nao em `poll_ms`; N linhas `%output` em rajada geram **um** ciclo; `%output` de painel nao configurado nao gera ciclo; o conteudo da linha nunca aparece em `status`, log nem evento (injetar marcador unico e varrer).
- **R19** quietude e degradacao: com eventos, saida do painel durante a janela de `idle` zera a confirmacao; saida de **outro** painel da mesma janela **nao** zera; cliente de eventos que morre (`%exit`, EOF, linha acima de 1 MiB, nao sobe) => `events: "degraded"`, o estado passa a usar `window_activity`, religa com recuo (1, 2, 4... ate 30 s) e nada vira `idle` por causa da morte do cliente.
- **R20** cliente de eventos so observa: o argv do processo e exatamente `-C attach -f ignore-size -t $N` (nunca `-r`, nunca `-d`/`-x`/`-y`), o stdin do processo falso recebe **zero byte** durante o teste inteiro, ha no maximo 8 processos desse tipo e um por sessao distinta.
- **R21** (rev5.3) descoberta: com `discover: true`, painel vivo cujo comando em primeiro plano e de um `kind` (`identify.commands`) aparece em `agent.list` com `origin: "discovered"`, id `<kind>-<diretorio>` (secao 16) e estado pelas regras; com `discover: false` (padrao) **nenhum** painel fora de `agents` aparece e nenhum e capturado. Shell, `vim`, comando desconhecido e painel morto nunca viram agente.
- **R22** (rev5.3) nome estavel: depois de descoberto, o id **nao muda** enquanto o painel vive (mesmo `pane_id` e `pane_pid`), ainda que o diretorio mude no cenario; dois paineis no mesmo diretorio e `kind` ficam `-A` e `-A-2` pela ordem do `pane_id`; o segundo nao rouba o id do primeiro quando o primeiro some e reaparece.
- **R23** (rev5.3) precedencia e origem: fonte configurada (`pane`, `tag` ou `cwd`) vence e da `origin: "configured"` (painel configurado nunca e descoberto de novo); painel com `@agentd NOME` sem entrada em `agents` vira agente `NOME` com `origin: "mark"`; `@agentd off` exclui o painel da descoberta; id descoberto igual a id configurado leva sufixo (`-2`), nunca toma o do configurado.
- **R24** (rev5.3) ciclo de vida: painel descoberto que some vai a `gone` como os demais e e **esquecido** depois de `discover_forget_s` em `gone` (`agent.get` => `unknown_agent`, sem evento extra); o teto de 32 agentes (configurados + descobertos) vale: excedentes nao entram e `status.sources.tmux.discover.ignored` os conta; um painel descoberto que volta ao shell some da lista de ativos como em R10.
- **R25** (rev5.3) captura minima: com descoberta, so se captura painel configurado, marcado ou descoberto e vivo (R3 estendido); clientes de eventos (R20) tambem nascem para sessoes com painel descoberto, no maximo 8.
- **R26** (rev5.3) caminho hostil: diretorio com TAB, aspas, `$()`, espacos, unicode, nome de 300 caracteres e raiz `/` gera id valido (`[a-z0-9_-]{1,32}`), nada executa, a leitura do `list-panes` nao quebra, e o caminho nunca aparece fora do id derivado (R5 estendido: sentinela no caminho nao vaza para log, status, evento nem CLI, exceto o trecho que compoe o id).
- **R27** (rev5.3) contrato: `origin` aparece em `agent.get`, `agent.list`, eventos, `agentd list/get --json` e na linha da CLI; `status.sources.tmux.discover = {"on", "found", "ignored"}`; `discover_forget_s` fora de 60 a 86400 e `cwd` nao absoluto ou com caractere fora de `[A-Za-z0-9._/-]` sao recusados (saida 1).

## 12. Fora desta spec
Entrega de mensagem (enviar teclas, verificar ocioso e caixa vazia antes e depois): programa proprio (`wakerd`), com seu proprio `capture-pane`; **despertar e integracao do Pi: fase F7** (aqui o Pi entra so como regra de estado, decisao do Jabbur em 2026-10-16); fonte `declared` e hooks dos agentes (Anexo da rev4; sem metodo de escrita na v1); `monitor-silence`; barra de status do tmux lendo o `agentd`; integracao com o `chatd`.
**Nao medido (viram `unknown` ate haver tela gravada):** Claude em `acceptEdits`/`bypassPermissions` (rodape diferente), durante `/compact` longo, erro ou limite de cota; Codex em aprovacao de edicao de arquivo e em outros modos de sandbox; Pi com extensoes de aprovacao; terminais abaixo de ~40 colunas (rodape truncado); Claude com tela alternativa e historico (`capture-pane` ve so a janela visivel); varios paineis por janela.

## Anexo A (normativo para a v1 de dados): regras iniciais
Regioes `tail:N` contam linhas nao vazias. Todas as regex em sintaxe do crate `regex`; fixtures citadas existem em `crates/agentd/rules/fixtures/` (`NOME.screen` = saida do `capture-pane -p -e -J`, ou texto plano; `NOME.title` = titulo do painel, uma linha, opcional).

**Claude (`identify.commands`: `claude`)**
| id | estado | `all` (regiao: regex) | `none` |
|---|---|---|---|
| `claude.blocked.permission` | blocked/permission | `tail:12`: `^\s*Do you want to .+\?\s*$` e `tail:12`: `Esc to cancel` | |
| `claude.blocked.plan` | blocked/permission | `tail:12`: `Would you like to proceed\?` e `tail:12`: `^\s*❯ 1\. ` | |
| `claude.blocked.trust` | blocked/permission | `tail:12`: `Quick safety check` e `tail:12`: `Enter to confirm` | |
| `claude.blocked.menu` | blocked/user_input | `tail:3`: `Esc to cancel` | |
| `claude.working.footer` | working | `tail:8`: `esc to interrupt` | |
| `claude.idle.box` | idle | `tail:8`: `^❯` e `tail:8`: `^\s*(⏸|⏵⏵) ` (rodape de modo) | `tail:8`: `esc to interrupt`; `tail:12`: `Esc to cancel` |
| marca `claude.mark.background` | `background` | `screen`: `(Waiting for \d+ background agent|^\s*◯ )` | |
`input`: `between` `^─{20,}$`, `strip_prefix` `^(?:❯|›) ?`.

**Codex (`identify.commands`: `codex`)**
| id | estado | `all` | `none` |
|---|---|---|---|
| `codex.blocked.title` | blocked/permission | `title`: `Action Required` | |
| `codex.blocked.approval` | blocked/permission | `tail:12`: `Would you like to .+\?` e `tail:12`: `Press enter to confirm or esc to cancel` | |
| `codex.blocked.menu` | blocked/user_input | `tail:3`: `(enter select · esc back|Browsing transcript|enter continue · esc skip)` | |
| `codex.working.title` | working | `title`: `^[\x{2800}-\x{28FF}]` | |
| `codex.working.screen` | working | `tail:8`: `esc to interrupt` | |
| `codex.idle.box` | idle | `tail:6`: `^›` e `tail:3`: `^\s+\S.* · [/~]` (linha de modelo e diretorio) | |
`input`: `line` `tail:6` `^›`, `strip_prefix` `^› ?`.

**Pi (`identify.commands`: `pi`)**
| id | estado | `all` | `none` |
|---|---|---|---|
| `pi.blocked.menu` | blocked/user_input | `tail:6`: `Escape/Ctrl\+C to cancel` | |
| `pi.working.ruler` | working | `tail:6`: `^── [\x{280B}\x{2819}\x{2839}\x{2838}\x{283C}\x{2834}\x{2826}\x{2827}\x{2807}\x{280F}] Working ─+$` | |
| `pi.idle.box` | idle | `tail:6`: `^─{20,}$` com `min: 2` e `tail:2`: `\$\d+\.\d{3}` | |
`input`: `between` `^─{20,}$`, sem `strip_prefix`.

**Limites conhecidos dessas regras (medidos, nao escondidos):**
1. **Claude, janela de ~0,5 s depois do Enter:** antes de o rodape ganhar `esc to interrupt`, a tela e **identica a de um rascunho humano** (caixa com texto, rodape de modo): sai `idle` com `input: draft`. Quem protege e a confirmacao da regra 5' (duas leituras, `window_activity` igual, `idle_confirm_ms`): o estado publicado nao vira `idle` por causa dessa janela (teste R17).
2. **Claude com rascunho:** o rodape perde a dica `? for shortcuts`; por isso a regra de `idle` usa so o simbolo de modo (`⏸`/`⏵⏵`). Modos de rodape nao medidos (`accept edits`, `bypass`) so casam se usarem esses simbolos; senao, `unknown`.
3. **Codex com rascunho:** o rodape perde `? for shortcuts` e a regra usa a linha de modelo e diretorio (` · /caminho`). Na partida o titulo do Codex tem spinner enquanto carrega: conta como `working`.
4. **Menus de comando (`/model` no Claude e no Codex, autocompletar) dentro da caixa** aparecem como `idle` com `input: draft` quando o menu e so a lista de sugestoes; o seletor modal (com `Esc to cancel`/`enter select`) e `blocked`.

(O Anexo A e o ponto de partida: o proprio `rules check` e a lista de fixtures da secao seguinte decidem se cada regra se sustenta. Qualquer discrepancia entre estas regex e as telas reais vence a **tela real**, e a regex e corrigida.)

## Anexo B: fixtures e como gravar
Pasta `crates/agentd/rules/fixtures/{claude,codex,pi}/`. Cada fixture e um par `NOME.screen` + `NOME.title`. Nomes: `idle`, `idle-manual`, `working-*`, `permission-*`, `plan`, `trust`, `menu-model`, `ask-user`, `slash-menu`, `narrow-working`, `background`, `steering-queue`, `interrupted`, `transcript`, `update-banner`, `approval-dot`, `approval-bang`; para a caixa (gravadas com `-e`): `box-empty`, `box-draft`, `box-draft-multiline`, `box-placeholder` (Codex), `box-ghost` (Claude: a linha da caixa foi gravada com `-e` numa sessao real e colada na tela do mesmo turno, porque a sugestao nao reaparece de forma reproduzivel). As fixtures de estado em texto plano vem de `capture-pane -p -J` e as `box-*` de `-e`; todas passam por sanitizacao de caminhos e do nome da maquina. Gravar: num tmux **isolado** (`tmux -S SOCKET -f /dev/null`), `tmux capture-pane -p -e -J -t ALVO > NOME.screen` e `tmux display -p -t ALVO '#{pane_title}' > NOME.title`; **sanitizar** caminhos do usuario (`/home/...`, diretorios temporarios) trocando por `/work/proj` e conferir que nao ha chave, token, e-mail nem texto de conversa real; registrar a versao do agente em `tested_versions`.

## Historico
| Fonte | Achado | Onde ficou |
|---|---|---|
| Decisao A22 (Jabbur, 2026-10-16) | tmux unico multiplexador; fonte `herdr` descartada | secoes 4, 7 |
| Leitura do herdr e do Luvus | sem regra casando = `idle`; empate de prioridade; carencia; texto plano da grade | secoes 2, 6 |
| Medicao do modo de controle (2026-10-16 tarde) | `-r` quebra o `send-keys` de outros processos; `-r` nao protege `kill-pane`; latencia ~0,17 s com debounce 150 ms; ocioso silencioso em Codex e Pi; `Try "..."` do Claude troca a cada 8 s; nenhum canal OSC 9;4 | secao 4A |
| Laboratorio tmux (rodadas 1 e B) | hooks nao cobrem Esc; titulo cobre o falso idle do Codex; dialogo de plano sem `Esc to cancel`; sugestao fantasma e placeholder dim; subagente em segundo plano; `window_activity` constante em ocioso | secoes 1, 5, 6, Anexo A |

## 15. Esclarecimentos dos testes independentes (rev5.2, 2026-10-16 tarde)
O escritor dos testes (Haiku 5.5, sem ver o codigo) levantou 10 ambiguidades; esta e a resposta, aditiva e sem mudar o que ja estava escrito. Em empate, vale este texto.
1. **`--json` de `get`/`list`:** `data` repete o resultado do protocolo sem o `v`: `get` => `{"agent": Agente, "epoch", "head_seq"}`; `list` => `{"agents": [Agente...], "epoch", "head_seq"}`. Cada linha de `watch --json` continua trazendo o `Evento` em `data`.
2. **`status.sources.tmux.events`:** `"on"` com o cliente de eventos vivo em todas as sessoes com painel configurado, `"degraded"` se algum caiu (ou nao subiu), `"off"` com `events: false`. Sem painel configurado: `"off"`.
3. **Saida acima de 1 MiB do `list-panes`:** `source_too_large` (o mesmo codigo da captura acima de 256 KiB); a leitura inteira falha.
4. **`tmux_bin` ausente da configuracao:** vale o padrao `"tmux"` (procurado no `PATH`); nao e recusa. `tmux_bin` presente e vazio ou nao string: recusa (saida 1).
5. **Fixtures:** `serve` confere so que a lista `fixtures` de cada regra nao e vazia e **nao abre** os arquivos de fixture. `agentd rules check` abre cada fixture: arquivo inexistente ou ilegivel conta como falha (saida 1, com o nome da fixture).
6. **Separador do `-F`:** o byte TAB literal (0x09) no argumento, nove campos.
7. **`-S SOCKET`:** vem **antes** do subcomando (`TMUX -S SOCKET list-panes ...`, `TMUX -S SOCKET capture-pane ...`, `TMUX -S SOCKET -C attach ...`).
8. **Quietude com eventos:** com o cliente de eventos vivo na sessao do painel, a quietude e so o tempo desde o ultimo `%output` **daquele painel** (ou desde a partida do cliente); `window_activity` nao entra. Sem cliente (degradado, `events: false`), vale a regra 5' com `window_activity` da rev5.
9. **`gone_after_s`:** o limite e inclusivo no relogio monotonico do daemon e medido em ciclos; os testes aceitam o atraso de ate um `poll_ms` e nao medem o limite exato.
10. **Prontidao:** continua valendo a rev4 (secao 13, item 6): uma linha `agentd: pronto` em stderr quando o socket aceita conexao.

## 16. Descoberta de agentes (rev5.3, 2026-10-16 tarde; decisao do Jabbur: "Siga com o A")
**Problema.** Na rev5.1 cada painel precisa ser marcado (`@agentd`) ou citado por `pane`. O herdr achava o agente sozinho; aqui o operador teria de marcar todo painel novo.
**Exemplo.** O Jabbur abre `codex` num painel em `~/Projects/blocks`. Com `discover: true` o `agentd` ve o comando `codex`, le o diretorio, e publica `codex-blocks` (`origin: "discovered"`) sem marca. O Claude em `~/horus-reality-check` seria `claude-horus-reality-check`; para o nome combinado `claude`, uma linha em `agents` (`"cwd"`) ou a marca `@agentd claude` fixa o nome.
**Solucao.** O `agentd` continua so lendo: o que muda e **quais** paineis ele considera agente e **como** os nomeia. Nada novo e escrito no tmux.

- **Configuracao:** `discover` booleano (**padrao `false`**: opt-in; sem ele tudo vale como na rev5.1) e `discover_forget_s` inteiro de 60 a 86400 (padrao 3600). `agents` passa a poder ser vazio quando `discover` e `true`. Fonte nova em `agents`: `{"type":"tmux","cwd":"/caminho/absoluto"}` (so `[A-Za-z0-9._/-]`, sem `..`; casa com o diretorio **exato** do painel) alem de `pane` e `tag`.
- **`list-panes` ganha o decimo campo** `pane_current_path`, **antes** de `pane_title` (que continua por ultimo). O formato e `#{s|[^a-zA-Z0-9._/-]|_|:pane_current_path}`: o proprio tmux troca por `_` qualquer caractere fora de `[A-Za-z0-9._/-]` (medido no 3.7c: TAB, aspas, `$()` e unicode viram `_`; sem isso um TAB no diretorio quebraria os campos). Linha com menos de 10 campos, ou `pane_current_path` vazio, e leitura falha (`source_bad_output`) como as demais. Isso substitui a lista de nove campos da secao 4.
- **Quem e candidato:** painel vivo, nao ligado a nenhum agente de `agents`, cujo `pane_current_command` esta em `identify.commands` de exatamente um `kind` do arquivo de regras (Pi aparece ~1 s depois, quando deixa de ser `node`), **e** cuja marca `@agentd` nao e `off`.
- **Nome (id) e origem**, nesta ordem de precedencia:
  1. fonte configurada (`pane`, `tag` ou `cwd`): id e o da entrada; `origin: "configured"`;
  2. marca `@agentd NOME` valida sem entrada em `agents`: id `NOME`; `origin: "mark"`;
  3. descoberto: id `<kind>-<base>`, `base` = ultimo componente de `pane_current_path`, em minusculas, caracteres fora de `[a-z0-9_-]` viram `-`, repeticoes de `-` colapsam, apara `-`/`_`/`.` nas pontas; vazio (raiz `/`) => so `<kind>`; truncado a 32 caracteres (o prefixo `<kind>-` fica); `origin: "discovered"`.
  Colisao de id (entre descobertos, ou com um id configurado) leva sufixo `-2`, `-3`... aplicado pela ordem numerica do `pane_id`; um id configurado nunca e tomado.
- **Estabilidade:** o id e calculado **uma vez**, na primeira leitura valida do painel, e guardado junto de `(pane_id, pane_pid)`. Se o agente mudar de diretorio, o id **nao muda**. Troca de `pane_pid` no mesmo `pane_id` e painel novo (regra 3 da rev4): id recalculado.
- **Ciclo de vida:** o agente nasce `unknown`, `evidence: none`, marca `error`, e a primeira leitura valida gera o evento (rev4 13.4). Painel que some vira `gone` apos `gone_after_s`, como os configurados, e e **esquecido** (sai de `agent.list`; `agent.get` => `unknown_agent`) depois de `discover_forget_s` em `gone`, sem evento adicional. Limite de 32 agentes no total; os excedentes nao entram e sao contados em `status.sources.tmux.discover.ignored`.
- **Campo novo `origin`** em `Agente` e nos eventos: `"configured" | "mark" | "discovered"` (aditivo; nao gera evento; nao muda `v`). **Por que existe:** o nome derivado do diretorio nao e autoridade. Um programa do mesmo uid pode se chamar `codex` num diretorio `blocks` e receber o id `codex-blocks`. **Regra para consumidores (wakerd, `send-keys`):** entregar prompt so para agente com `origin` `configured` ou `mark`; `discovered` serve para mostrar e observar, nunca como destino de digitacao sem confirmacao do operador.
- **`status.sources.tmux.discover`** = `{"on": bool, "found": N, "ignored": N}`. Sem painel descoberto, `found` e 0.
- **Seguranca e privacidade:** com `discover: true` o `agentd` captura a tela de **qualquer** painel do servidor tmux que rode `claude`, `codex` ou `pi`, sem marca previa; o conteudo continua sem sair do daemon (principio 3). Para excluir um painel, `tmux set-option -p -t PAINEL @agentd off` (`off` e nome reservado). O id derivado revela o nome do diretorio de trabalho no `agent.list`/`status`/CLI: e a unica derivacao de dado do ambiente que sai do daemon, e so no formato do id.
- **Eventos (4A) e R3:** clientes de controle tambem nascem para sessoes com painel descoberto (limite de 8 inalterado); a lista fechada de comandos (secao 4) nao muda.
- **Fora desta secao:** marcar painel pelo proprio `agentd` (ele continua so lendo), descobrir outros programas alem dos `kind` do arquivo de regras, e escolher nome por titulo da janela.
