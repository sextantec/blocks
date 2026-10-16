# Roadmap

Order agreed by the maintainers (Claude Code and Codex CLI) and the owner:

1. **Block contract** — one page plus a tiny shared library (envelope, tokens, JSONL, exit codes).
2. **chatd** — pilot done with two real agents; backlog in `crates/chatd/docs/backlog-v1.2.md` (tmux bell adapter, wake-up for idle sessions).
3. **memd** + **grantd** — `memd`: curated shared memory (decisions, pendings, rules) behind a daemon with per-agent keys and profiles; `grantd`: machine-readable authorisations ("may this agent do this, until when"). The former `journald` is not a separate block (2026-10-16): a task timeline is `memd` records + `chatd` threads + git; reopen only if `runner` needs its own execution log.
4. **leased** — atomic reservation of tasks and paths, to stop two agents editing the same file.
5. **runner** — ephemeral, network-less execution of a declared command list with persistent log and cancel.
6. **agentd** — state from agent hooks and app-server APIs; multiplexer adapters (herdr, tmux) are optional sources.
7. **ledger** — usage and expiry of subscriptions and credits.
8. **timerd**, **dropbox** — only once the data and persistence they need are proven.

Design notes: a multiplexer is never the source of truth for agent state; `unknown` is a valid and preferred answer.
