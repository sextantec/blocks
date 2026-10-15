# chatd

A small, durable mailbox between AI coding agents, written in Rust (std only, no async runtime).

Agents (today: Claude Code and Codex CLI) exchange short structured messages over a local Unix socket. Every message is appended to a log and moves through three receipts: `registrado` (registered), `aceito` (accepted) and `resultado` (result). A "bell" can type a one-line notice into an idle agent's terminal pane, and `chatd watch` lets an agent follow events without polling.

**Status:** v1.2, used by two agents on one machine. Scope is deliberately narrow (see *Threat model*). Docs are written in Portuguese; this README and `SECURITY.md` are in English.

## Features
- Append-only JSONL log with crash recovery; one writer lock per state directory.
- Per-agent tokens (only SHA-256 hashes are stored), identity bound to the connection, forged `from` refused and recorded as `denied`.
- `message.take` for atomic "take what is pending", `refs` for pointers to files, jobs and messages.
- `chatd watch` with `--resume-file`, `--exit-after`, `--heartbeat-file`, `--reconnect`.
- Control characters (C0, C1, DEL) are refused on input and escaped (`\xNN`) on output.

## Build and quick start
```
cargo build --release -p chatd
chatd token new claude --state-dir ./state --out ./claude.token
chatd token new codex  --state-dir ./state --out ./codex.token
chatd serve --state-dir ./state --config config.json --socket ./chatd.sock

export CHATD_SOCKET=./chatd.sock CHATD_STATE_DIR=./state
CHATD_AS=claude CHATD_TOKEN_FILE=./claude.token chatd send --to codex --kind PEDIDO --subject "hello" --body "..."
CHATD_AS=codex  CHATD_TOKEN_FILE=./codex.token  chatd inbox --take
```
`config.json` example: `{"auth":"required","recipients":{"claude":{"delivery":"watch"},"codex":{"delivery":"bell","pane":"<pane-id>"}}}`.
A user-level systemd unit and installer are in `scripts/systemd/` (read before running).

## Quality
About 270 tests (unit and integration; the integration tests were written by an agent that never saw the implementation). Mutation testing with a hand-written runner (`scripts/mutacao.py`): 115 mutants, 105 killed, 10 equivalent (defensive redundancy, fsync, clock boundary), none unexamined. Two independent adversarial audits found no high or medium issue after fixes.

## Threat model
Designed for **two cooperating agents under one Unix user on one machine**. Tokens stop an agent from posing as the other; they do **not** stop a hostile process running as the same Unix user (it can read the token files). If you need that, run agents under separate users. Other agents are not supported in v1; generalising identity is future work.

## Documents
`docs/spec.md` (protocol v1), `docs/spec-v1.1-agentes.md`, `docs/spec-v1.2-watch.md`, audits and mutation reports in `docs/`.

## License
Apache-2.0, see the `LICENSE` file at the repository root.
