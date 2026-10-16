# ledger v0: controle de uso e expiracao de assinaturas e creditos (contrato)

Origin: requested by the owner on 2026-10-15. Bloco do lego: um binario pequeno, estado em arquivos legiveis, identidade e envelope do `chatd`. **Nao** e proxy de modelo: so contabiliza, avisa e (opcionalmente) responde "posso gastar mais nesta rota?". Imposicao de limite so existe onde ha ponto unico de passagem (API/OpenRouter via Pi); nas CLIs de assinatura (Claude, Codex) o ledger registra e alerta, nao bloqueia.

## Modelo de dados (JSONL, 0600, anexo apenas)
- **route** (`ledger route set`): `name` (ex.: `anthropic-sub`, `openai-sub`, `openrouter-credit`, `deepseek-credit`, `google-cloud-credit`, `antigravity`, `muse`), `kind` (`subscription|credit|api`), `unit` (`usd|tokens|requests|percent|unknown`), `balance` (numero ou `null`), `expires_at` (UTC ou `null`), `reset_at` (para assinaturas com janela), `source` (`manual|reported|measured`), `updated_at`.
- **usage** (`ledger usage add`): `ts`, `route`, `agent` (`claude|codex|pi|...`), `task` (texto livre curto ou id), `amount`, `unit`, `source` (`manual|reported|measured`).
- Campo desconhecido e `null`; **nunca** estimado em silencio (regra do Codex, aceita).

## Comandos (`--json` em todos; codigos 0 ok, 2 uso, 10 limite/alerta)
- `ledger route set|list|get`
- `ledger usage add|list --route R --since T`
- `ledger report [--route R]`: saldo, consumo por janela, dias ate expirar, **uso previsto ate expirar** quando houver taxa conhecida.
- `ledger alerts [--within 7d]`: rotas que expiram em N dias com saldo nao usado (a razao de ser: nao deixar credito vencer).
- `ledger check --route R --amount X`: 0 pode, 10 nao pode (so rotas `credit|api` com saldo conhecido).

## Fontes de dado (de pior para melhor)
1. **manual** (o Jabbur ou o agente digita saldo e validade).
2. **reported**: o agente grava `usage add` ao fechar uma tarefa (rotulo da rota e valor que a CLI mostra).
3. **measured**: tokens/custo vindos da resposta da API (OpenRouter e DeepSeek via Pi).

## Integracoes (depois do v0)
`chatd`: alerta de expiracao vira mensagem para o Jabbur/Claude; `agentd`: relaciona consumo ao agente e a tarefa; `memd`: cada gasto aponta para a pendencia (tarefa); `grantd`: para a concessao que autorizou o gasto.

## Aceite do v0
Testes independentes (Haiku), mutacao, fmt/clippy, e um piloto: carregar as 7 rotas reais com saldo e validade informados pelo Jabbur e rodar `ledger alerts`.
