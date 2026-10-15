# blocks

Small, composable building blocks of infrastructure for AI coding agents. Each block does one thing, keeps its state in plain files, speaks JSON over stdio or a local Unix socket, and is meant to be wired to the others like Lego bricks.

The project is built and maintained by AI agents (Claude Code and Codex CLI) under human ownership; see `AUTHORS.md`. Docs are partly in Portuguese; READMEs and `SECURITY.md` are in English.

## Blocks
| Block | What it does | Status |
|---|---|---|
| [`chatd`](crates/chatd) | Durable mailbox between agents: append-only log, three receipts, per-agent tokens, bell and `watch` | v1.2, audited, ~270 tests, mutation-tested |
| `agentd` | Observes agent state (`idle`, `working`, `blocked`, `gone`, `unknown`) from the agents' own hooks and APIs; never scrapes the screen | design only, see `docs/roadmap.md` |
| `ledger` | Tracks subscriptions and credits: balance, expiry, usage per route; warns before credit expires | contract only, see `docs/ledger-spec-v0.md` |
| contract | One-page block contract: CLI, `--json` envelope, exit codes, identity | planned |

## Build
```
cargo build --release          # Rust 1.99.0 (rust-toolchain.toml)
cargo test
```

## Principles
- One binary per block, state in readable files (JSONL), versioned `--json`.
- When a source is not observable the answer is `unknown`, never a guess.
- Identity is bound to the connection; tokens are never printed or logged.
- Independent tests: the tests of a security-relevant change are not written by its author, and a mutation run must pass.

## License
Apache-2.0, see `LICENSE`.
