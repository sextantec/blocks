//! Servidor WebSocket FALSO do app-server do Codex (spec-rev6 secoes 4 e 11). So biblioteca padrao.
//!
//! - Aceita o upgrade so em `GET /`; outros caminhos recebem 404 e ficam registrados (a lista fechada
//!   da secao 4 proibe qualquer caminho que nao seja `/`).
//! - Responde `initialize`, `thread/loaded/list` e `thread/read`; qualquer outro pedido (com `id`)
//!   recebe erro e fica registrado. Notificacoes (sem `id`) so sao registradas.
//! - `muda`/`notifica` mandam `thread/status/changed` para todas as conexoes ativas.
//! - Forma das mensagens do servidor: respostas `{"id","result"}` e notificacoes `{"method","params"}`,
//!   sem o campo `jsonrpc` (como o app-server real). A spec nao fixa isso; ver REV6-TESTES.md.
//! Nao le `src/`. O socket e um caminho temporario de cada teste.

use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Pedido recebido pelo servidor falso (metodo, params, id se houver, instante).
#[derive(Clone, Debug)]
pub struct Pedido {
    pub metodo: String,
    pub params: Value,
    pub id: Option<Value>,
    pub t: Instant,
}

type Escritor = Arc<Mutex<UnixStream>>;

struct Estado {
    thread: String,
    status: Value,
    carregadas: Vec<String>,
    recusar: bool,
    conexoes: Vec<(u64, Escritor)>,
    prox: u64,
    caminhos: Vec<String>,
    pedidos: Vec<Pedido>,
    tentativas: Vec<Instant>,
}

/// Handle do servidor falso. Clonavel: os clones compartilham o mesmo estado.
#[derive(Clone)]
pub struct FakeCodex {
    pub socket: PathBuf,
    est: Arc<Mutex<Estado>>,
}

impl FakeCodex {
    /// Cria o servidor em `dir/app-server-control.sock`. `id_thread` e a unica thread carregada por
    /// padrao; `status` e o `thread.status` que `thread/read` devolve.
    pub fn inicia(dir: &Path, id_thread: &str, status: Value) -> FakeCodex {
        let socket = dir.join("app-server-control.sock");
        let _ = std::fs::remove_file(&socket);
        let ouvinte = UnixListener::bind(&socket).expect("bind do servidor falso");
        let est = Arc::new(Mutex::new(Estado {
            thread: id_thread.to_string(),
            status,
            carregadas: vec![id_thread.to_string()],
            recusar: false,
            conexoes: Vec::new(),
            prox: 0,
            caminhos: Vec::new(),
            pedidos: Vec::new(),
            tentativas: Vec::new(),
        }));
        let e2 = Arc::clone(&est);
        thread::spawn(move || {
            for c in ouvinte.incoming() {
                let Ok(stream) = c else { continue };
                let recusa = {
                    let mut e = e2.lock().unwrap();
                    e.tentativas.push(Instant::now());
                    e.recusar
                };
                if recusa {
                    // App-server "morto": a conexao e aceita pelo kernel e fechada sem handshake.
                    drop(stream);
                    continue;
                }
                let e3 = Arc::clone(&e2);
                thread::spawn(move || atende(stream, e3));
            }
        });
        FakeCodex { socket, est }
    }

    /// Muda o `thread.status` e avisa todas as conexoes com `thread/status/changed`.
    pub fn muda(&self, status: Value) {
        let id = {
            let mut e = self.est.lock().unwrap();
            e.status = status.clone();
            e.thread.clone()
        };
        self.notifica(&id, status);
    }

    /// Muda o status sem notificar: a proxima `thread/read` (por exemplo, depois de reconectar) ja o traz.
    pub fn muda_sem_avisar(&self, status: Value) {
        self.est.lock().unwrap().status = status;
    }

    /// Notificacao `thread/status/changed` para um `threadId` qualquer.
    pub fn notifica(&self, id_thread: &str, status: Value) {
        let msg = json!({"method": "thread/status/changed", "params": {"threadId": id_thread, "status": status}});
        self.envia_a_todas(&msg.to_string());
    }

    /// Envia um texto bruto como quadro de texto a todas as conexoes (para casos hostis).
    pub fn envia_bruto(&self, texto: &str) {
        self.envia_a_todas(texto);
    }

    fn envia_a_todas(&self, texto: &str) {
        let ws: Vec<Escritor> = self
            .est
            .lock()
            .unwrap()
            .conexoes
            .iter()
            .map(|(_, w)| Arc::clone(w))
            .collect();
        for w in ws {
            let _ = w.lock().unwrap().write_all(&quadro(texto));
        }
    }

    /// Derruba as conexoes ativas e recusa novas (o app-server morreu): o daemon ve EOF.
    pub fn derruba(&self) {
        let ws = {
            let mut e = self.est.lock().unwrap();
            e.recusar = true;
            std::mem::take(&mut e.conexoes)
        };
        for (_, w) in ws {
            let _ = w.lock().unwrap().shutdown(Shutdown::Both);
        }
    }

    /// Volta a aceitar conexoes.
    pub fn religa(&self) {
        self.est.lock().unwrap().recusar = false;
    }

    /// Define a lista de threads carregadas (`thread/loaded/list`).
    pub fn define_carregadas(&self, ids: &[&str]) {
        self.est.lock().unwrap().carregadas = ids.iter().map(|s| s.to_string()).collect();
    }

    pub fn pedidos(&self) -> Vec<Pedido> {
        self.est.lock().unwrap().pedidos.clone()
    }

    pub fn metodos(&self) -> Vec<String> {
        self.pedidos().into_iter().map(|p| p.metodo).collect()
    }

    pub fn conta(&self, metodo: &str) -> usize {
        self.pedidos().iter().filter(|p| p.metodo == metodo).count()
    }

    /// Todo caminho HTTP recebido (inclusive os recusados com 404).
    pub fn caminhos(&self) -> Vec<String> {
        self.est.lock().unwrap().caminhos.clone()
    }

    /// Instante de cada conexao aceita pelo socket (inclusive as recusadas).
    pub fn tentativas(&self) -> Vec<Instant> {
        self.est.lock().unwrap().tentativas.clone()
    }

    /// Conexoes com handshake concluido e ainda abertas.
    pub fn conexoes(&self) -> usize {
        self.est.lock().unwrap().conexoes.len()
    }
}

fn atende(stream: UnixStream, est: Arc<Mutex<Estado>>) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut leitor = BufReader::new(clone);
    let escritor: Escritor = Arc::new(Mutex::new(stream));
    let mut primeira = String::new();
    if leitor.read_line(&mut primeira).unwrap_or(0) == 0 {
        return;
    }
    let caminho = primeira.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut chave: Option<String> = None;
    loop {
        let mut l = String::new();
        match leitor.read_line(&mut l) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let t = l.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            if k.trim().eq_ignore_ascii_case("sec-websocket-key") {
                chave = Some(v.trim().to_string());
            }
        }
    }
    est.lock().unwrap().caminhos.push(caminho.clone());
    let chave = match (caminho == "/", chave) {
        (true, Some(k)) => k,
        (false, _) => {
            let _ = escritor.lock().unwrap().write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
        (true, None) => {
            let _ = escritor.lock().unwrap().write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
    };
    let resposta = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        aceite_websocket(&chave)
    );
    if escritor
        .lock()
        .unwrap()
        .write_all(resposta.as_bytes())
        .is_err()
    {
        return;
    }
    let id = {
        let mut e = est.lock().unwrap();
        e.prox += 1;
        let id = e.prox;
        e.conexoes.push((id, Arc::clone(&escritor)));
        id
    };
    loop {
        match le_quadro(&mut leitor) {
            Ok((1, p)) => {
                if let Ok(msg) = serde_json::from_slice::<Value>(&p) {
                    trata(&est, &escritor, msg);
                }
            }
            Ok((8, _)) => {
                let _ = escritor.lock().unwrap().write_all(&[0x88, 0x00]);
                break;
            }
            Ok((9, p)) => {
                let mut f = vec![0x8A, p.len().min(125) as u8];
                f.extend_from_slice(&p[..p.len().min(125)]);
                let _ = escritor.lock().unwrap().write_all(&f);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    est.lock().unwrap().conexoes.retain(|(i, _)| *i != id);
}

fn trata(est: &Arc<Mutex<Estado>>, w: &Escritor, msg: Value) {
    let Some(metodo) = msg
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let id = msg.get("id").filter(|v| !v.is_null()).cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let resposta: Option<Result<Value, &'static str>> = {
        let mut e = est.lock().unwrap();
        e.pedidos.push(Pedido {
            metodo: metodo.clone(),
            params: params.clone(),
            id: id.clone(),
            t: Instant::now(),
        });
        match metodo.as_str() {
            "initialize" => Some(Ok(json!({"userAgent": "fake-codex-app-server"}))),
            "initialized" => None,
            "thread/loaded/list" => Some(Ok(
                json!({"data": e.carregadas.clone(), "nextCursor": null}),
            )),
            "thread/read" => {
                let pedida = params.get("threadId").and_then(Value::as_str);
                if pedida == Some(e.thread.as_str()) && e.carregadas.contains(&e.thread) {
                    Some(Ok(json!({"thread": {"id": e.thread, "status": e.status}})))
                } else {
                    Some(Err("thread not found"))
                }
            }
            _ => Some(Err("fake: metodo fora do contrato")),
        }
    };
    if let (Some(id), Some(r)) = (id, resposta) {
        let msg = match r {
            Ok(res) => json!({"id": id, "result": res}),
            Err(m) => json!({"id": id, "error": {"code": -32601, "message": m}}),
        };
        let _ = w.lock().unwrap().write_all(&quadro(&msg.to_string()));
    }
}

/// Quadro de texto do servidor (nao mascarado, FIN=1).
fn quadro(texto: &str) -> Vec<u8> {
    let p = texto.as_bytes();
    let mut v = vec![0x81];
    if p.len() < 126 {
        v.push(p.len() as u8);
    } else if p.len() <= 0xFFFF {
        v.push(126);
        v.extend_from_slice(&(p.len() as u16).to_be_bytes());
    } else {
        v.push(127);
        v.extend_from_slice(&(p.len() as u64).to_be_bytes());
    }
    v.extend_from_slice(p);
    v
}

/// Le um quadro do cliente (mascarado, como exige a RFC 6455). Devolve (opcode, payload).
pub fn le_quadro(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h)?;
    let op = h[0] & 0x0f;
    let mascarado = h[1] & 0x80 != 0;
    let mut n = u64::from(h[1] & 0x7f);
    if n == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        n = u64::from(u16::from_be_bytes(b));
    } else if n == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        n = u64::from_be_bytes(b);
    }
    if n > 8 * 1024 * 1024 {
        return Err(io::Error::other("quadro grande demais"));
    }
    let mut chave = [0u8; 4];
    if mascarado {
        r.read_exact(&mut chave)?;
    }
    let mut p = vec![0u8; n as usize];
    r.read_exact(&mut p)?;
    if mascarado {
        for (i, b) in p.iter_mut().enumerate() {
            *b ^= chave[i % 4];
        }
    }
    Ok((op, p))
}

/// Valor de `Sec-WebSocket-Accept` para uma chave de cliente (RFC 6455, secao 4.2.2).
pub fn aceite_websocket(chave: &str) -> String {
    base64(&sha1(format!("{chave}{GUID}").as_bytes()))
}

fn sha1(dados: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = dados.to_vec();
    let bits = (dados.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for bloco in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                bloco[4 * i],
                bloco[4 * i + 1],
                bloco[4 * i + 2],
                bloco[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn base64(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in d.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}
