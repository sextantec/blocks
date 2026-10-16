# Experimento: o que cada hook do Claude Code recebe (2026-10-16, claude 2.1.302, Haiku 5.5)

Instancia isolada (cwd e `--settings` no scratchpad, `--setting-sources project`, nada da sessao do Jabbur). `rec.sh EVENTO` grava `TMUX_PANE`, a presenca de `TMUX` e o stdin de cada hook. Reproduzir:

```
python3 -I - <<'PY'   # gera settings.json com o rec.sh em cada evento (async)
import json; ev=["UserPromptSubmit","PermissionRequest","PermissionDenied","Notification","Stop","StopFailure","SessionStart","SessionEnd","PreToolUse","PostToolUse","Elicitation"]
json.dump({"hooks":{e:[{"matcher":"","hooks":[{"type":"command","command":"/CAMINHO/rec.sh "+e,"async":True}]}] for e in ev}},open("settings.json","w"))
PY
claude -p 'Rode no shell exatamente: python3 -c "print(42)" e responda so o numero.' --model claude-haiku-5-5 --setting-sources project --settings settings.json
```
Resultado: dispararam `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `Stop`, `SessionEnd`. Todos receberam `TMUX_PANE` do painel de quem rodou o `claude`. Campos do stdin: comuns `session_id`, `transcript_path`, `cwd`, `hook_event_name`; `prompt_id` (o mesmo em todos os eventos de um turno) e `permission_mode` nos de turno; `PermissionRequest` traz `tool_name`, `tool_input`, `permission_suggestions`; `Stop` traz `stop_hook_active`, `last_assistant_message`, `background_tasks`, `session_crons`; `SessionEnd` traz `reason`; `SessionStart` traz `source`. Nao testado aqui: `Esc` (interrupcao), `Notification`, `Elicitation`, `StopFailure`, `PermissionDenied`.
