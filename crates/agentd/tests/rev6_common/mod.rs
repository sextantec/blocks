#![allow(dead_code)]
//! Harness rev6 (caixa preta) do agentd. Reusa o harness rev5 (tmux FALSO, daemon, socket, CLI) por
//! `#[path]`, sem copia, e acrescenta o que a rev6 precisa:
//! - configuracao com as fontes estruturadas (`codex_app_server`, `claude_hooks`) e as chaves de tempo;
//! - o servidor WebSocket FALSO do app-server (`fake_codex.rs`);
//! - `agentd report EVENTO` com `TMUX_PANE` e `AGENTD_SOCKET` controlados, stdin de arquivo e tempo medido;
//! - subida com recusa (F2b): o daemon sai antes de aceitar conexao.
//! Nunca executa tmux, Codex, Claude ou herdr reais; nada de rede; `HOME` e `XDG_CONFIG_HOME` sao
//! temporarios (nunca ~/.codex, ~/.config, ~/.claude). Nao le `src/`. Escrito a partir de
//! `docs/spec-rev6.md` (secoes 4, 5, 7, 8, 11) e do harness rev5.

#[path = "../rev5_common/mod.rs"]
pub mod rev5;
pub use rev5::*;

#[path = "fake_codex.rs"]
pub mod codex;
#[allow(unused_imports)]
pub use codex::*;

use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Thread do Codex usada nos testes (UUID minusculo, valido para o thread_file).
pub const UUID: &str = "0190f3a2-7c4d-7e21-9a3b-5d6e7f8a9b0c";
/// Outra thread, para a lista de carregadas e para notificacoes de thread nao configurada.
pub const UUID_OUTRA: &str = "0190f3a2-7c4d-7e21-9a3b-000000000002";
/// Tela de um agente sem nenhuma regra casando (leitura `unknown`, sem `error`).
pub const TELA_SEM_REGRA: &[u8] = b"texto sem nenhuma regra\n";
/// Um segundo para os prazos de latencia (spec rev6 secao 4: "em menos de 1 s do evento").
pub const SEG: Duration = Duration::from_secs(1);

/// Diretorio HOME temporario do cenario (com `xdg` dentro). Nunca e o HOME do usuario.
pub fn home(m: &Mundo) -> PathBuf {
    let h = m.raiz.path().join("home");
    fs::create_dir_all(h.join("xdg")).expect("criar HOME falso");
    h
}

/// Copia as regras padrao para o cenario (modo 0644) e devolve o caminho.
pub fn regras(m: &Mundo) -> PathBuf {
    let r = m.raiz.path().join("regras.json");
    copia_regras(&r, &regras_padrao());
    r
}

/// Configuracao base da rev6: tmux falso, eventos desligados, poll 200 ms, confirmacao de `idle` imediata
/// (rev5 5'), `codex_socket` sempre no temporario do teste. `agentes` e o objeto `agents`.
pub fn cfg6(m: &Mundo, agentes: Value, codex_socket: &Path) -> Value {
    json!({
        "tmux_bin": s(&m.tmux),
        "tmux_socket": null,
        "rules_file": s(&regras(m)),
        "codex_socket": s(codex_socket),
        "poll_ms": 200,
        "events": false,
        "event_debounce_ms": 150,
        "safety_poll_ms": 5000,
        "source_timeout_ms": 400,
        "stale_after_ms": 3000,
        "gone_after_s": 5,
        "idle_confirm_polls": 2,
        "idle_confirm_ms": 0,
        "ring": 4096,
        "agents": agentes
    })
}

/// Agente `kind: claude` com a tela (tag `tag`) e o relato de hooks.
pub fn ag_claude(tag: &str) -> Value {
    json!({"kind": "claude", "sources": [{"type": "tmux", "tag": tag}, {"type": "claude_hooks"}]})
}

/// Agente `kind: codex` com a tela (tag `tag`) e a fonte `codex_app_server` apontando para `thread`.
pub fn ag_codex(tag: &str, thread: &Path) -> Value {
    json!({"kind": "codex", "sources": [{"type": "tmux", "tag": tag}, {"type": "codex_app_server", "thread_file": s(thread)}]})
}

/// Agente so com a tela (sem fonte estruturada): comportamento da rev5.
pub fn ag_so_tela(kind: &str, tag: &str) -> Value {
    json!({"kind": kind, "sources": [{"type": "tmux", "tag": tag}]})
}

/// Grava o arquivo de thread (conteudo bruto) com o modo dado. Devolve o caminho em `m.raiz/nome`.
pub fn thread_file(m: &Mundo, nome: &str, conteudo: &[u8], modo: u32) -> PathBuf {
    let p = m.raiz.path().join(nome);
    fs::write(&p, conteudo).expect("gravar thread_file");
    fs::set_permissions(&p, fs::Permissions::from_mode(modo)).expect("chmod thread_file");
    p
}

/// Sobe o daemon com a configuracao `c` (estado e HOME temporarios). Falha se nao aceitar conexao.
pub fn sobe6(m: &Mundo, c: &Value) -> Servidor {
    let estado = estado_dir(m);
    let cfgp = escreve_cfg(m.raiz.path(), c);
    let h = home(m);
    let xdg = h.join("xdg");
    sobe_com(
        None,
        &estado,
        &cfgp,
        &[("HOME", s(&h)), ("XDG_CONFIG_HOME", s(&xdg))],
    )
}

/// Espera o agente publicar `estado` dentro de `prazo` (contado a partir daqui). Falha se nao publicar.
pub fn publica_em(sk: &Path, id: &str, estado: &str, prazo: Duration) -> Value {
    let a = espera_estado(sk, id, estado, prazo);
    assert!(
        a["state"] == estado,
        "`{id}` nao publicou `{estado}` em {prazo:?}: {a}"
    );
    a
}

/// Espera a primeira leitura valida do agente (campo `observed_at` nao nulo).
pub fn espera_observado(sk: &Path, id: &str, prazo: Duration) -> Value {
    let a = espera_agente(sk, id, prazo, |a| !a["observed_at"].is_null());
    assert!(
        !a["observed_at"].is_null(),
        "`{id}` sem leitura valida em {prazo:?}: {a}"
    );
    a
}

/// Estado publicado agora.
pub fn estado_de(sk: &Path, id: &str) -> String {
    agente(sk, id)["state"].as_str().unwrap_or("?").to_string()
}

/// `true` se o agente tem a marca `marca` agora.
pub fn tem_marca(a: &Value, marca: &str) -> bool {
    marcas(a).contains(marca)
}

/// Amostra o estado durante `d` (a cada 20 ms) e diz se alguma amostra foi `proibido`.
pub fn ve_estado(sk: &Path, id: &str, proibido: &str, d: Duration) -> bool {
    let fim = Instant::now() + d;
    while Instant::now() < fim {
        if estado_de(sk, id) == proibido {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Dorme ate o instante `t` (nao dorme se ja passou).
pub fn dorme_ate(t: Instant) {
    let agora = Instant::now();
    if t > agora {
        thread::sleep(t - agora);
    }
}

/// `status.sources[nome]` (ex.: `app_server`, `hooks`, `tmux`).
pub fn fonte(sk: &Path, nome: &str) -> Value {
    status(sk)["sources"][nome].clone()
}

/// (aceitos, rejeitados) de `status.sources.hooks.reports`.
pub fn relatos(sk: &Path) -> (u64, u64) {
    let h = fonte(sk, "hooks");
    (
        h["reports"]["accepted"].as_u64().unwrap_or(0),
        h["reports"]["rejected"].as_u64().unwrap_or(0),
    )
}

/// Resultado de um `agentd report`: saida, codigo e tempo ate sair.
pub struct Relato {
    pub saida: Saida,
    pub duracao: Duration,
}

/// `agentd report EVENTO` com `TMUX_PANE` = `pane` (None = variavel ausente), `AGENTD_SOCKET` = `socket`
/// e `stdin` como payload (simula o JSON do hook). Espera ate 10 s.
pub fn report(m: &Mundo, socket: &Path, pane: Option<&str>, evento: &str, stdin: &[u8]) -> Relato {
    report_com(m, socket, pane, evento, stdin, None)
}

/// Como `report`, com `AGENTD_REPORT_TIMEOUT_MS` (quando `timeout_ms` e Some).
pub fn report_com(
    m: &Mundo,
    socket: &Path,
    pane: Option<&str>,
    evento: &str,
    stdin: &[u8],
    timeout_ms: Option<&str>,
) -> Relato {
    let h = home(m);
    let mut entrada = tempfile::tempfile().expect("stdin temporario");
    entrada.write_all(stdin).expect("gravar stdin");
    entrada.seek(SeekFrom::Start(0)).expect("rebobinar stdin");
    let mut saida_out = tempfile::tempfile().expect("stdout temporario");
    let mut saida_err = tempfile::tempfile().expect("stderr temporario");
    let mut cmd = Command::new(BIN);
    cmd.args(["report", evento])
        .env_remove("TMUX_PANE")
        .env_remove("AGENTD_REPORT_TIMEOUT_MS")
        .env("AGENTD_SOCKET", socket)
        .env("HOME", &h)
        .env("XDG_CONFIG_HOME", h.join("xdg"))
        .stdin(Stdio::from(entrada))
        .stdout(Stdio::from(saida_out.try_clone().expect("clonar stdout")))
        .stderr(Stdio::from(saida_err.try_clone().expect("clonar stderr")));
    if let Some(p) = pane {
        cmd.env("TMUX_PANE", p);
    }
    if let Some(t) = timeout_ms {
        cmd.env("AGENTD_REPORT_TIMEOUT_MS", t);
    }
    let inicio = Instant::now();
    let mut filho = cmd.spawn().expect("executar agentd report");
    let fim = inicio + Duration::from_secs(10);
    let code = loop {
        if let Ok(Some(st)) = filho.try_wait() {
            break st.code();
        }
        if Instant::now() >= fim {
            let _ = filho.kill();
            let _ = filho.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let duracao = inicio.elapsed();
    Relato {
        saida: Saida {
            code,
            stdout: le_arquivo(&mut saida_out),
            stderr: le_arquivo(&mut saida_err),
        },
        duracao,
    }
}

fn le_arquivo(f: &mut File) -> String {
    let _ = f.seek(SeekFrom::Start(0));
    let mut s = String::new();
    let _ = f.read_to_string(&mut s);
    s
}

/// Tudo o que o daemon expos: stderr, `agent.list`, `status` e todos os eventos retidos (como texto).
/// Usado para varrer sentinelas (P1, H5): nenhum deles pode conter o valor secreto.
pub fn dump_daemon(srv: &Servidor, sk: &Path) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        srv.stderr(),
        Value::Array(lista(sk)),
        status(sk),
        Value::Array(eventos_todos(sk))
    )
}

/// Saidas da CLI sobre o socket: `get ID` de cada agente, `list` e `status`, como texto.
pub fn dump_cli(m: &Mundo, sk: &Path, ids: &[&str]) -> String {
    let h = home(m);
    let mut partes = Vec::new();
    for id in ids {
        partes.push(todo_texto(&cli(&h, sk, &["get", id])));
    }
    partes.push(todo_texto(&cli(&h, sk, &["list"])));
    partes.push(todo_texto(&cli(&h, sk, &["status"])));
    partes.join("\n")
}

/// Resultado de tentar subir o daemon com uma configuracao (F2b).
pub enum Boot {
    /// Aceitou conexao (o daemon foi encerrado logo depois do teste).
    Subiu,
    /// Saiu antes de aceitar conexao: codigo de saida e stderr.
    Recusou(Option<i32>, String),
}

/// Sobe o daemon com `c` e diz se ele aceitou (`Subiu`) ou saiu (`Recusou`). Usa estado proprio `nome`.
/// Falha se nem subiu nem saiu em 4 s.
pub fn tenta_subir(m: &Mundo, nome: &str, c: &Value) -> Boot {
    let estado = m.raiz.path().join(format!("estado-{nome}"));
    fs::create_dir_all(&estado).expect("criar estado");
    fs::set_permissions(&estado, fs::Permissions::from_mode(0o700)).expect("chmod estado");
    let cfgp = m.raiz.path().join(format!("config-{nome}.json"));
    fs::write(&cfgp, c.to_string()).expect("gravar config");
    let h = home(m);
    let err_path = m.raiz.path().join(format!("stderr-{nome}.txt"));
    let errf = File::create(&err_path).expect("criar stderr");
    let mut filho = Command::new(BIN)
        .args(["serve", "--config", s(&cfgp), "--state-dir", s(&estado)])
        .env_remove("AGENTD_SOCKET")
        .env_remove("AGENTD_STATE_DIR")
        .env("HOME", &h)
        .env("XDG_CONFIG_HOME", h.join("xdg"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(errf))
        .spawn()
        .expect("executar agentd serve");
    let sock = estado.join("agentd.sock");
    let fim = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(Some(st)) = filho.try_wait() {
            let texto = fs::read_to_string(&err_path).unwrap_or_default();
            return Boot::Recusou(st.code(), texto);
        }
        if UnixStream::connect(&sock).is_ok() {
            let _ = filho.kill();
            let _ = filho.wait();
            return Boot::Subiu;
        }
        if Instant::now() >= fim {
            let _ = filho.kill();
            let _ = filho.wait();
            panic!("`{nome}`: o daemon nem subiu nem saiu em 4 s");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Cria um link simbolico `link` -> `alvo` (para o caso symlink de S6).
pub fn link(alvo: &Path, link: &Path) {
    symlink(alvo, link).expect("criar symlink");
}
