# Security policy

## Reporting
Please use GitHub's private vulnerability reporting for this repository (Security tab). Do not open a public issue for a vulnerability. Include version, a minimal reproduction and the effect you observed.

## Scope and threat model
chatd v1 is built for two cooperating agents under one Unix user. In scope: identity confusion between the two agents, message leakage between them, terminal-escape injection through message content, denial of service of the daemon by one agent, log corruption. Out of scope: a hostile process running as the same Unix user, a compromised host, and multi-user or networked deployments.

## Handling contributed code
Treat contributions, including those produced by AI agents, as untrusted input: read before running, run in a sandbox, and require independent tests (not written by the author of the change) and a passing mutation run for changes that touch authentication, identity or the log.
