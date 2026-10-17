# Instalacao e primeiro uso do memd

Os scripts nao habilitam nem iniciam servico sem que voce rode o ultimo comando. Tudo fica no seu usuario (`~/.local`, `~/.config/systemd/user`).

## 1. Binario, estado, tokens e unit
Na raiz do repositorio:

```sh
bash crates/memd/scripts/instala.sh --offline                 # compila, instala em ~/.local/bin/memd (backup datado)
bash crates/memd/scripts/prepara.sh                           # estado em ~/.local/state/memd, tokens, grants.json, wrappers
bash crates/memd/scripts/systemd/instala-unit.sh              # copia a unit e recarrega; nao inicia
systemctl --user enable --now memd.service
```

`prepara.sh` e idempotente (nao recria o que existe) e `--dry-run` mostra o plano. Cria:
- `~/.local/state/memd/` (0700): `config.json`, `grants.json`, `log.jsonl`, `memd.sock`, `GUIA-AGENTES.md`, `tokens/<agente>.token` (0600).
- Dois perfis: `claude` (propoe e promove) e `codex-blocks` (so propoe); leem e escrevem em qualquer escopo (`*`). Expiram em 90 dias (`--expira AAAA-MM-DD`). Edite `grants.json` para estreitar escopos e depois `systemctl --user reload memd.service` (SIGHUP) para valer sem reiniciar.
- Wrappers `~/.local/bin/memd-claude` e `~/.local/bin/memd-codex-blocks` (so levam o caminho do token, nao o segredo).

## 1b. Qualquer agente (codex, pi, hermes...)
O memd e geral: cada agente e um perfil em `grants.json` com token proprio e um wrapper `memd-<nome>`. `agente.sh` cuida disso e recarrega o servico (SIGHUP), sem reiniciar:

```sh
bash crates/memd/scripts/agente.sh add codex                         # le e propoe em qualquer escopo
bash crates/memd/scripts/agente.sh add pi --ops propose --read 'pi/**,blocks/**' --write 'pi/**'
bash crates/memd/scripts/agente.sh add hermes --ops propose,promote  # outro curador
bash crates/memd/scripts/agente.sh list
bash crates/memd/scripts/agente.sh rotate codex                      # token novo; o antigo deixa de valer
bash crates/memd/scripts/agente.sh remove pi                         # apaga perfil, token e wrapper
```
Escopos: `*` = tudo, `a/b` = exato, `a/**` = `a` e tudo abaixo. `--types decision,pending` restringe os tipos. Cada edicao deixa um `grants.json.bak.<data>`. Agente sem shell (cliente MCP, outro daemon) fala direto o protocolo JSON por linha no `memd.sock`, com o token no pedido.

## 2. Primeiro teste
```sh
memd-claude whoami
memd-claude view
memd-codex-blocks propose --params '{"request_id":"t1","type":"decision","title":"Teste","body":"x","scope":"blocks/teste","fields":{"criterion":"c"},"source":{"kind":"doc","ref":"README","at":"2026-10-16"}}'
memd-claude promote --params '{"request_id":"t2","id":"D-001","rev":1,"expected_seq":1}'
```
(`expected_seq` = `proposed_seq` do registro; veja com `memd-claude get --params '{"id":"D-001"}'`.) O guia completo, para os agentes, esta em `~/.local/state/memd/GUIA-AGENTES.md`.

## 3. Limites honestos (spec, secao 9)
Agentes do mesmo usuario Unix leem o token uns dos outros e editam `log.jsonl`; o `memd` detecta incoerencia (`systemctl --user stop memd.service && memd validate --state-dir ~/.local/state/memd`; com o daemon rodando o `validate` recusa por causa da trava), nao a impede. Isolamento real pede contas Unix por agente (v1/v2).
