# blocklog v0: motor de log duravel de commits (contrato normativo) | RASCUNHO v0 (2026-10-16)

**Estado: RASCUNHO para revisao. Nao ha codigo.** Crate de biblioteca compartilhado pelo `memd` (`docs/memd-spec-v0.md`) e pelo `grantd` (`docs/grantd-spec-v0.md`). Resolve em um so lugar a durabilidade e a atomicidade apontadas na revisao do Astra (job 997) e do codex-blocks (msg 621). Fontes lidas no codigo: `crates/chatd/src/store.rs:203-262,565-591` (abertura, recuperacao, `append_many`), Pi Durable `packages/durable/src/storage/jsonl/storage.ts:288-300`, `harness/submissions.ts:155-159`.

## 0. O que e e o que nao e
Um arquivo `log.jsonl` de **commits** de acrescimo, um dono por diretorio, com recuperacao de cauda, idempotencia por pedido e relogio nao regressivo. **Nao** interpreta o conteudo dos eventos (isso e do `memd`/`grantd`), nao tem indices, instantaneos, rotacao nem arquivamento (v0.2), nem rede. A biblioteca nunca fala com socket.

## 1. Garantias e premissas (leia primeiro)
- **G1. Sucesso implica durabilidade** sob as premissas do armazenamento: `write_all` + `sync_data` retornaram `Ok` num sistema de arquivos local POSIX. Um disco ou sistema de arquivos que mente sobre `fsync` esta **fora** da garantia; checksums nao a restauram.
- **G2. Ausencia de resposta e resultado indeterminado.** Queda entre o `sync_data` e a resposta: o commit existe. Queda antes: nao existe. O chamador resolve repetindo com o **mesmo `request_id`** (G4).
- **G3. Atomicidade logica por linha:** o commit e **uma linha**; na recuperacao ou ela e aplicada inteira ou e descartada inteira. Isso nao e atomicidade fisica de escrita; e a regra de recuperacao (secao 4) que a produz.
- **G4. Idempotencia:** o mesmo `(agent, request_id)` com a mesma carga devolve o resultado original sem novo commit (secao 6).
- **G5. Sem lacunas:** `seq` numera **commits** (linhas), comeca em 1 e cresce de 1 em 1.
- **G6. Fail-closed:** falha de escrita ou de `sync` torna o log `broken` ate o processo reiniciar (secao 5).

## 2. Layout e permissoes
Diretorio de estado `D` (modo 0700) com `log.jsonl` (0600) e `LOCK` (0600). Na abertura: diretorio com qualquer bit de grupo/outros, ou arquivo com `mode & 0o077 != 0`, ou arquivo que nao e regular, ou symlink (abertura com `O_NOFOLLOW`) = recusa (`insecure_permissions`). Criacao: arquivos novos com 0600 e `fsync` do diretorio depois de criar `log.jsonl`. O dono do arquivo deve ser o uid do processo.

## 3. Formato do commit
Uma linha UTF-8 terminada em `\n`, objeto JSON, chaves exatas (desconhecida = corrupcao):
`{"v":1,"seq":N,"ts":"2026-10-16T22:00:00.123Z","tx":[E1,E2,...],"req":{"agent":"A","id":"R","h":"HEX64"},"res":RES}`
- `v` = 1; `seq` inteiro; `ts` RFC 3339 UTC com milissegundos; `tx` = lista **nao vazia** de ate `tx_max` (16) objetos (os eventos, opacos para o `blocklog`); `req` opcional (todas as tres chaves obrigatorias quando presente: `agent` `[a-z0-9_-]{1,32}`, `id` `[A-Za-z0-9._-]{1,64}`, `h` = SHA-256 em hexadecimal minusculo da carga canonica enviada pelo chamador); `res` opcional (JSON qualquer, o resultado original a devolver em repeticao; obrigatorio quando `req` presente).
- **Limite:** a linha serializada (sem `\n`) tem no maximo `line_max` bytes (padrao 262144; faixa 4096 a 4194304). Estourar = `too_large` **antes** de qualquer escrita.
- O `blocklog` nao reordena nem reescreve eventos. Serializacao: `serde_json` sem espacos; a ordem das chaves do envelope e a da lista acima.

## 4. Abertura e recuperacao
1. Tomar `flock(LOCK_EX|LOCK_NB)` em `LOCK`; falha = `already_running`.
2. Ler `log.jsonl` inteiro. Os bytes **depois do ultimo `\n`** sao a **cauda**: **descartados** e o arquivo truncado para o ultimo `\n`, com `sync_all` do arquivo **antes** de aceitar escritas. Isso vale mesmo que a cauda seja um JSON valido sem `\n` (nao terminou de ser gravada). (Mais estrito que o `chatd`, que descarta tambem uma ultima linha **com** `\n` e ilegivel.)
3. Cada linha terminada em `\n` deve ser um commit valido (secao 3) com `seq` = anterior + 1 e `ts` >= o anterior. **Qualquer linha completa invalida, em qualquer posicao, inclusive a ultima, recusa a partida** (`corrupt`, com o numero da linha); o `blocklog` nunca apaga uma linha completa.
4. Reconstruir o indice de idempotencia: para cada commit com `req`, `(agent,id)` -> `(seq, h, res, ts)`. **Duas ocorrencias do mesmo `(agent,id)`** (com `h` igual ou diferente): a segunda e valida **somente se** `ts` da segunda >= `ts` da primeira + `request_retention_days` (a primeira ja tinha vencido quando a segunda foi aceita; a segunda passa a ser a vigente); caso contrario e **corrupcao** (`Corrupt`, numero da linha da segunda), porque `commit` nunca grava duas ocorrencias vivas. Ao fim do replay, so entram no indice as entradas com `ts` + `request_retention_days` > o instante da abertura.
5. Entregar os commits ao chamador (iterador em ordem) para que ele reconstrua a sua projecao. Log vazio = estado inicial valido.

## 5. Escrita
`commit(tx, req?, res?)` (chamado **depois** que o chamador validou tudo e **antes** de ele aplicar na memoria):
1. Se `broken` -> `broken`. 2. Se ha `req`: indice com o mesmo `(agent,id)`: `h` igual -> devolve o `res` original com `replayed:true`, **sem escrever**; `h` diferente -> `request_conflict`; nao ha -> segue. 3. Validar `tx` (nao vazio, <= `tx_max`) e o tamanho da linha. 4. `ts = max(agora, ts do ultimo commit)` (relogio nao regressivo; o relogio e injetavel). 5. Montar a linha com `seq = ultimo + 1` e fazer **um** `write_all` seguido de `sync_data`. 6. Sucesso: registrar no indice, avancar `seq` e devolver `{seq, replayed:false}`; so entao o chamador aplica. 7. **Erro em `write_all` ou `sync_data` (inclui `ENOSPC`, escrita curta, `EIO`):** o log fica `broken` (nenhum commit novo, leitura continua); o chamador **nao** aplica nada; a linha pode ou nao existir apos reiniciar (G2). `log_max_bytes` (padrao 268435456; faixa 1 MiB a 64 GiB): ultrapassar = `log_full`, sem escrever, **sem** `broken`.
O `blocklog` nunca re-tenta uma escrita.

## 6. Idempotencia (detalhe)
Igualdade = mesmo `agent`, mesmo `id`, mesmo `h`. O chamador define a carga canonica (o `memd`/`grantd` hasheiam o metodo e os parametros em JSON canonico: chaves ordenadas, sem espacos). Depois de `request_retention_days` o mesmo `(agent,id)` conta como pedido novo (documentado; nao ha erro). A repeticao devolve o `res` **gravado**, nunca recalculado; a **autorizacao** de quem repete e conferida pelo chamador antes de devolver (o `blocklog` so entrega o `res`).

## 7. Abstracoes para teste (obrigatorias no desenho)
Toda E/S passa por tres interfaces injetaveis (secao 8): `LogFile`, `Storage` e `Clock`. A versao real usa a std e `flock`; os testes usam falhas injetadas (erro em `write_all`, escrita curta, erro em `sync_data`, `ENOSPC`, relogio regressivo). Nao ha codigo de teste no caminho de producao alem dessas interfaces.

## 8. API publica (Rust; assinaturas normativas, os testes compilam contra elas)
Crate `blocklog` (biblioteca; edicao e toolchain do workspace), sem `unsafe` fora do `flock` (`libc`). Tipos de JSON: `serde_json::Value`; instante: `chrono::DateTime<chrono::Utc>`.
```rust
pub trait LogFile: Send {                       // uma instancia = o log.jsonl aberto
    fn read_all(&mut self) -> std::io::Result<Vec<u8>>;
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;   // acrescenta no fim
    fn sync_data(&mut self) -> std::io::Result<()>;
    fn sync_all(&mut self) -> std::io::Result<()>;
    fn set_len(&mut self, len: u64) -> std::io::Result<()>;
    fn len(&mut self) -> std::io::Result<u64>;
}
pub trait Storage: Send {                       // o diretorio de estado
    fn lock(&mut self) -> Result<(), OpenError>;            // flock exclusivo nao bloqueante
    fn open_log(&mut self) -> Result<Box<dyn LogFile>, OpenError>; // cria 0600, confere permissoes, fsync do diretorio
}
pub trait Clock: Send { fn now(&self) -> chrono::DateTime<chrono::Utc>; }

pub struct FsStorage; impl FsStorage { pub fn new(dir: &std::path::Path) -> FsStorage }   // implementacao real (std + libc)
pub struct SystemClock;                          // implementacao real do Clock

#[derive(Clone, Debug)]
pub struct Options { pub line_max: usize, pub tx_max: usize, pub request_retention_days: u32, pub log_max_bytes: u64 }
impl Default for Options { /* 262144, 16, 30, 268435456 */ }
impl Options { pub fn validate(&self) -> Result<(), OpenError> }  // faixas da secao 3 e 5; fora = OpenError::BadOptions

#[derive(Clone, Debug, PartialEq)]
pub struct Req { pub agent: String, pub id: String, pub h: String }   // h = sha256_hex da carga canonica
#[derive(Clone, Debug, PartialEq)]
pub struct Commit { pub seq: u64, pub ts: String, pub tx: Vec<serde_json::Value>, pub req: Option<Req>, pub res: Option<serde_json::Value> }
#[derive(Clone, Debug, PartialEq)]
pub struct Committed { pub seq: u64, pub replayed: bool, pub res: Option<serde_json::Value> }

#[derive(Debug)] pub enum OpenError { AlreadyRunning, InsecurePermissions(String), Corrupt { line: usize, reason: String }, BadOptions(String), Io(std::io::Error) }
#[derive(Debug)] pub enum CommitError { Broken, TooLarge, TxTooLarge, EmptyTx, InvalidInput(String), LogFull, RequestConflict, MissingRes, Io(std::io::Error) }

pub struct Log { /* privado */ }
impl Log {
    pub fn open(storage: Box<dyn Storage>, clock: Box<dyn Clock>, opts: Options) -> Result<(Log, Vec<Commit>), OpenError>;
    pub fn open_dir(dir: &std::path::Path, opts: Options) -> Result<(Log, Vec<Commit>), OpenError>;   // FsStorage + SystemClock
    pub fn commit(&mut self, tx: Vec<serde_json::Value>, req: Option<Req>, res: Option<serde_json::Value>) -> Result<Committed, CommitError>;
    pub fn head_seq(&self) -> u64;               // 0 se vazio
    pub fn is_broken(&self) -> bool;
}
pub fn sha256_hex(data: &[u8]) -> String;       // 64 hex minusculos
```
- Na repeticao idempotente (`req` ja no indice com o mesmo `h`) `commit` devolve `Committed{seq: original, replayed: true, res: gravado}` e **ignora** o `res` recebido; sem escrita. `req` presente e `res` ausente = `CommitError::MissingRes`. `tx` com mais eventos que `Options::tx_max` = `TxTooLarge`; vazio = `EmptyTx`. **`InvalidInput(motivo)`** (nada e gravado, sem `broken`): algum elemento de `tx` que nao e objeto JSON; `Req.agent` fora de `[a-z0-9_-]{1,32}`; `Req.id` fora de `[A-Za-z0-9._-]{1,64}`; `Req.h` que nao e 64 hexadecimais minusculos. A validacao roda **antes** de qualquer escrita, na ordem: `Broken`, `EmptyTx`, `TxTooLarge`, `InvalidInput`, `MissingRes`, idempotencia, `TooLarge`, `LogFull`.
- Faixas de `Options`: `line_max` 4096..=4194304; `tx_max` 1..=64; `request_retention_days` 1..=3650; `log_max_bytes` 1048576..=68719476736. `open` chama `validate` antes de tudo.
- Cada operacao de `LogFile`/`Storage` que falhar vira `Io(...)` (em `open`) ou `Io(...)` + `Broken` conforme a secao 5 (em `commit`). `FsStorage::lock` e `open_log` mapeiam `EWOULDBLOCK` -> `AlreadyRunning` e o modo/dono/symlink invalido -> `InsecurePermissions`.

## 9. Contrato testavel (testes independentes; sem rede; falhas por `LogFile`/`Clock` injetados)
- **Recuperacao:** B1 cauda sem `\n` com JSON parcial descartada e arquivo truncado; B2 cauda sem `\n` com JSON **valido** tambem descartada; B3 linha completa invalida na ultima posicao recusa a partida; B4 linha invalida no meio recusa; B5 `seq` com lacuna, repetido ou retrocedido recusa; B6 `ts` regressivo recusa; B7 chave desconhecida ou `tx` vazio recusa; B8 depois da recuperacao o arquivo termina em `\n` e novo commit recebe `seq` correto.
- **Escrita:** B9 sucesso grava uma linha e um `sync_data`; B10 `write_all` falho = `broken`, nada aplicado, leitura continua, commit seguinte `broken`; B11 `sync_data` falho idem; B12 escrita curta idem; B13 `ENOSPC` idem; B14 linha acima de `line_max` = `too_large` sem escrever e sem `broken`; B15 `tx` > `tx_max` ou vazio; B16 `log_max_bytes` = `log_full` sem `broken`; B17 `ts` nao regride com relogio regressivo.
- **Entrada invalida e duplicata (adicionados na rev1.1):** B29 elemento de `tx` que nao e objeto = `InvalidInput`, nada gravado, sem `broken`; B30 `Req` com `agent`, `id` ou `h` fora do formato (casos: maiuscula no agent, id com espaco, `h` com 63 ou 65 caracteres ou maiuscula) = `InvalidInput`; B31 log com duas ocorrencias do mesmo `(agent,id)` a menos de `request_retention_days` uma da outra recusa a partida; B32 duas ocorrencias separadas por mais de `request_retention_days` sao validas e a ultima e a vigente.
- **Idempotencia:** B18 mesma `(agent,id,h)` devolve `res` original com `replayed:true` e nao grava; B19 mesmo id e `h` diferente = `request_conflict`; B20 a idempotencia sobrevive a reabertura; B21 fora de `request_retention_days` conta como novo; B22 `req` sem `res` e recusado.
- **Dono e permissoes:** B23 segundo `open` = `already_running`; B24 modo errado/symlink = `insecure_permissions`; B25 arquivos criados 0600 e diretorio com `fsync` apos criar; B26 uid errado recusado (so se rodar como root; senao registrar como nao testado).
- **Estado reconstruivel:** B27 reabrir e reler os commits devolve a mesma sequencia que foi gravada; B28 SHA-256 contra vetores NIST (implementacao propria, como a do `chatd`, `crates/chatd/src/hash.rs`).

## 10. Dependencias
`serde`, `serde_json`, `libc`, `chrono` ou `time` (RFC 3339). SHA-256 propria (a do `chatd` e o ponto de partida) ou `sha2` aprovado pelo Jabbur. Sem outras.

## 11. Fora desta spec (v0.2 em diante)
Instantaneos, rotacao e arquivamento de segmentos, indices, `watch`, checksum por linha, compactacao, replicacao, rede, cifra.
