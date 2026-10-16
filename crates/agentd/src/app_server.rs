use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
    path::Path,
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Status {
    pub state: String,
    pub reason: Option<String>,
    pub lost: bool,
    pub fallback: bool,
}

pub fn read_thread_id(path: &Path) -> Result<String, ()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| ())?;
    let meta = file.metadata().map_err(|_| ())?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
        return Err(());
    }
    let mut raw = Vec::with_capacity(64);
    file.take(65).read_to_end(&mut raw).map_err(|_| ())?;
    if raw.len() > 64 {
        return Err(());
    }
    let value = std::str::from_utf8(&raw)
        .map_err(|_| ())?
        .trim_end_matches(['\n', '\r']);
    let b = value.as_bytes();
    if b.len() != 36
        || b[8] != b'-'
        || b[13] != b'-'
        || b[18] != b'-'
        || b[23] != b'-'
        || !b.iter().enumerate().all(|(i, c)| {
            [8, 13, 18, 23].contains(&i) && *c == b'-'
                || c.is_ascii_digit()
                || (b'a'..=b'f').contains(c)
        })
    {
        return Err(());
    }
    Ok(value.to_owned())
}

pub fn observe(
    socket: &Path,
    thread_file: &Path,
    reread: Duration,
    min_delay: Duration,
    max_delay: Duration,
    mut update: impl FnMut(Status),
) -> ! {
    let mut delay = min_delay;
    loop {
        let result = read_thread_id(thread_file)
            .and_then(|thread| session(socket, thread_file, &thread, reread, &mut update));
        let _ = result;
        update(Status {
            state: "unknown".into(),
            reason: None,
            lost: true,
            fallback: true,
        });
        std::thread::sleep(delay);
        delay = (delay * 2).min(max_delay);
    }
}

fn session(
    socket: &Path,
    thread_file: &Path,
    thread: &str,
    reread: Duration,
    update: &mut impl FnMut(Status),
) -> Result<(), ()> {
    let mut stream = UnixStream::connect(socket).map_err(|_| ())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(|_| ())?;
    let is_loaded = initialize(&mut stream, thread)?;
    if !is_loaded {
        update(Status {
            state: "unknown".into(),
            reason: None,
            lost: true,
            fallback: true,
        });
        return wait_for_loaded_thread(&mut stream, thread_file, thread, reread, update);
    }
    update(read_initial_status(&mut stream, thread)?);
    listen_status(&mut stream, thread_file, thread, reread, update)
}

fn initialize(stream: &mut UnixStream, thread: &str) -> Result<bool, ()> {
    handshake(stream)?;
    rpc(
        stream,
        1,
        "initialize",
        json!({"clientInfo":{"name":"agentd","title":"agentd","version":"0.0.1"},"capabilities":{}}),
    )?;
    notify(stream, "initialized", json!({}))?;
    let loaded = rpc(stream, 2, "thread/loaded/list", json!({}))?;
    Ok(loaded
        .get("data")
        .and_then(Value::as_array)
        .is_some_and(|threads| threads.iter().any(|t| t.as_str() == Some(thread))))
}

fn read_initial_status(stream: &mut UnixStream, thread: &str) -> Result<Status, ()> {
    let read = rpc(
        stream,
        3,
        "thread/read",
        json!({"threadId":thread,"includeTurns":false}),
    )?;
    Ok(read
        .get("thread")
        .and_then(|t| t.get("status"))
        .map(map_status)
        .unwrap_or_else(|| unknown(false)))
}

fn wait_for_loaded_thread(
    stream: &mut UnixStream,
    thread_file: &Path,
    thread: &str,
    reread: Duration,
    update: &mut impl FnMut(Status),
) -> Result<(), ()> {
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() >= reread {
            let _ = read_thread_id(thread_file);
            return Err(());
        }
        update_from_notification(stream, thread, update)?;
    }
}

fn listen_status(
    stream: &mut UnixStream,
    thread_file: &Path,
    thread: &str,
    reread: Duration,
    update: &mut impl FnMut(Status),
) -> Result<(), ()> {
    let mut last_reread = std::time::Instant::now();
    loop {
        if last_reread.elapsed() >= reread {
            if read_thread_id(thread_file).ok().as_deref() != Some(thread) {
                return Err(());
            }
            last_reread = std::time::Instant::now();
        }
        update_from_notification(stream, thread, update)?;
    }
}

fn read_status_notification(stream: &mut UnixStream, thread: &str) -> Result<Option<Status>, ()> {
    let Some(msg) = read_frame(stream)? else {
        return Ok(None);
    };
    if msg.get("method").and_then(Value::as_str) != Some("thread/status/changed") {
        return Ok(None);
    }
    let params = &msg["params"];
    if params.get("threadId").and_then(Value::as_str) != Some(thread) {
        return Ok(None);
    }
    Ok(Some(
        params
            .get("status")
            .map(map_status)
            .unwrap_or_else(|| unknown(false)),
    ))
}

fn update_from_notification(
    stream: &mut UnixStream,
    thread: &str,
    update: &mut impl FnMut(Status),
) -> Result<(), ()> {
    if let Some(status) = read_status_notification(stream, thread)? {
        update(status);
    }
    Ok(())
}

fn map_status(v: &Value) -> Status {
    let Some(o) = v.as_object() else {
        return unknown(false);
    };
    let kind = o.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "idle" if o.len() == 1 => Status {
            state: "idle".into(),
            reason: None,
            lost: false,
            fallback: false,
        },
        "active"
            if o.keys()
                .all(|k| ["type", "activeFlags"].contains(&k.as_str())) =>
        {
            let Some(flags) = o.get("activeFlags").and_then(Value::as_array) else {
                return unknown(false);
            };
            if flags.iter().any(|f| f.as_str().is_none()) {
                return unknown(false);
            }
            if flags
                .iter()
                .any(|f| !matches!(f.as_str(), Some("waitingOnApproval" | "waitingOnUserInput")))
            {
                return unknown(false);
            }
            if flags.iter().any(|f| f == "waitingOnApproval") {
                Status {
                    state: "blocked".into(),
                    reason: Some("permission".into()),
                    lost: false,
                    fallback: false,
                }
            } else if flags.iter().any(|f| f == "waitingOnUserInput") {
                Status {
                    state: "blocked".into(),
                    reason: Some("user_input".into()),
                    lost: false,
                    fallback: false,
                }
            } else {
                Status {
                    state: "working".into(),
                    reason: None,
                    lost: false,
                    fallback: false,
                }
            }
        }
        "notLoaded" => unknown(true),
        "systemError" => unknown(false),
        _ => unknown(false),
    }
}
fn unknown(lost: bool) -> Status {
    Status {
        state: "unknown".into(),
        reason: None,
        lost,
        fallback: false,
    }
}

fn rpc(s: &mut UnixStream, id: u64, method: &str, params: Value) -> Result<Value, ()> {
    send_frame(
        s,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string(),
    )?;
    loop {
        let Some(v) = read_frame(s)? else { continue };
        if v.get("id").and_then(Value::as_u64) == Some(id) {
            if v.get("error").is_some() {
                return Err(());
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}
fn notify(s: &mut UnixStream, method: &str, params: Value) -> Result<(), ()> {
    send_frame(
        s,
        &json!({"jsonrpc":"2.0","method":method,"params":params}).to_string(),
    )
}

fn handshake(s: &mut UnixStream) -> Result<(), ()> {
    let mut nonce = [0u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut nonce))
        .map_err(|_| ())?;
    let key = base64(&nonce);
    write!(s, "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").map_err(|_| ())?;
    let mut response = Vec::new();
    let mut byte = [0];
    while response.len() < 8192 && !response.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).map_err(|_| ())?;
        response.push(byte[0]);
    }
    let text = String::from_utf8(response).map_err(|_| ())?;
    if !text.starts_with("HTTP/1.1 101") && !text.starts_with("HTTP/1.0 101") {
        return Err(());
    }
    let expected = base64(&sha1(
        format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
    ));
    if !text.lines().any(|l| {
        l.to_ascii_lowercase().starts_with("sec-websocket-accept:")
            && l.split_once(':').is_some_and(|(_, v)| v.trim() == expected)
    }) {
        return Err(());
    }
    Ok(())
}
fn send_frame(s: &mut UnixStream, payload: &str) -> Result<(), ()> {
    let b = payload.as_bytes();
    if b.len() > 1_048_576 {
        return Err(());
    }
    let mask = random_mask()?;
    let len_code = if b.len() < 126 {
        b.len() as u8
    } else if b.len() <= u16::MAX as usize {
        126
    } else {
        127
    };
    s.write_all(&[0x81, 0x80 | len_code]).map_err(|_| ())?;
    if len_code == 126 {
        s.write_all(&(b.len() as u16).to_be_bytes())
            .map_err(|_| ())?;
    } else if len_code == 127 {
        s.write_all(&(b.len() as u64).to_be_bytes())
            .map_err(|_| ())?;
    }
    s.write_all(&mask).map_err(|_| ())?;
    let data: Vec<u8> = b.iter().enumerate().map(|(i, x)| x ^ mask[i % 4]).collect();
    s.write_all(&data).map_err(|_| ())?;
    s.flush().map_err(|_| ())
}
fn read_frame(s: &mut UnixStream) -> Result<Option<Value>, ()> {
    loop {
        let mut h = [0; 2];
        match s.read(&mut h[..1]) {
            Ok(0) => return Err(()),
            Ok(1) => {}
            Ok(_) => return Err(()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(_) => return Err(()),
        };
        s.read_exact(&mut h[1..]).map_err(|_| ())?;
        let opcode = h[0] & 15;
        let masked = h[1] & 128 != 0;
        let mut len = (h[1] & 127) as usize;
        if len == 126 {
            let mut b = [0; 2];
            s.read_exact(&mut b).map_err(|_| ())?;
            len = u16::from_be_bytes(b) as usize;
        } else if len == 127 {
            let mut b = [0; 8];
            s.read_exact(&mut b).map_err(|_| ())?;
            len = usize::try_from(u64::from_be_bytes(b)).map_err(|_| ())?;
        }
        if len > 1_048_576 {
            return Err(());
        }
        let mut mask = [0; 4];
        if masked {
            s.read_exact(&mut mask).map_err(|_| ())?;
        }
        let mut data = vec![0; len];
        s.read_exact(&mut data).map_err(|_| ())?;
        if masked {
            for (i, b) in data.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        match opcode {
            1 => return serde_json::from_slice(&data).map(Some).map_err(|_| ()),
            8 => return Err(()),
            9 => {
                let body = String::from_utf8_lossy(&data);
                send_control(s, 10, body.as_bytes())?;
            }
            10 => {}
            _ => return Err(()),
        }
    }
}
fn send_control(s: &mut UnixStream, opcode: u8, data: &[u8]) -> Result<(), ()> {
    if data.len() > 125 {
        return Err(());
    }
    let mask = random_mask()?;
    s.write_all(&[0x80 | opcode, 0x80 | data.len() as u8])
        .map_err(|_| ())?;
    s.write_all(&mask).map_err(|_| ())?;
    let b: Vec<u8> = data
        .iter()
        .enumerate()
        .map(|(i, x)| x ^ mask[i % 4])
        .collect();
    s.write_all(&b).map_err(|_| ())
}
fn random_mask() -> Result<[u8; 4], ()> {
    let mut mask = [0; 4];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut mask))
        .map_err(|_| ())?;
    Ok(mask)
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut data = input.to_vec();
    let bits = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0)
    }
    data.extend_from_slice(&bits.to_be_bytes());
    let (mut h0, mut h1, mut h2, mut h3, mut h4) = (
        0x67452301u32,
        0xefcdab89,
        0x98badcfe,
        0x10325476,
        0xc3d2e1f0,
    );
    for chunk in data.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, b) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1)
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
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
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }
    let mut out = [0; 20];
    for (i, v) in [h0, h1, h2, h3, h4].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes())
    }
    out
}
fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | (((*c.get(1).unwrap_or(&0)) as u32) << 8)
            | (*c.get(2).unwrap_or(&0) as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if c.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
