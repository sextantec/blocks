use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Arc, Mutex},
    thread,
};

use blocklog::{Log, Options, SystemClock};
use memd::{Config, Grants, Service, token_sha256_hex};
use serde_json::{Map, Value, json};

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err((code, message)) => {
            eprintln!("{message}");
            code
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), (i32, String)> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(usage)?;
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "serve" => serve(&rest),
        "init" => init(rest.first().map(PathBuf::from).ok_or_else(usage)?),
        "token-hash" => token_hash(),
        "validate" => validate(&rest),
        "propose" | "promote" | "reject" | "retire" | "get" | "query" | "view" | "whoami"
        | "status" => client(&command, &rest),
        _ => Err(usage()),
    }
}

fn serve(args: &[String]) -> Result<(), (i32, String)> {
    let state = required_path(args, "--state-dir")?;
    let config_path = required_path(args, "--config")?;
    let grants_path = required_path(args, "--grants")?;
    let socket = option(args, "--socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| state.join("memd.sock"));
    let config_text = read_text(&config_path)?;
    let cfg = Config::from_json(&config_text).map_err(startup)?;
    let line_max = config_number(&config_text, "line_max", 262_144)?;
    let max_connections = config_number(&config_text, "max_connections", 32)?;
    let grants = Grants::from_json(&read_text(&grants_path)?).map_err(startup)?;
    let opts = Options {
        line_max,
        ..Options::default()
    };
    let (log, replay) =
        Log::open_dir(&state, opts).map_err(|e| startup(format!("blocklog: {e:?}")))?;
    let mut service =
        Service::new(cfg.clone(), grants, log, replay, Box::new(SystemClock)).map_err(startup)?;
    service.tick();
    let shared = Arc::new(Mutex::new(service));
    let mut signals =
        signal_hook::iterator::Signals::new([signal_hook::consts::SIGHUP]).map_err(io_error)?;
    {
        let service = Arc::clone(&shared);
        let path = grants_path.clone();
        thread::spawn(move || {
            for _ in signals.forever() {
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(grants) = Grants::from_json(&text) else {
                    continue;
                };
                if let Ok(mut svc) = service.lock() {
                    svc.reload_grants(grants);
                } else {
                    return;
                }
            }
        });
    }
    if socket.exists() {
        return Err(startup("socket ja existe".into()));
    }
    let listener = UnixListener::bind(&socket).map_err(io_error)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    {
        let service = Arc::clone(&shared);
        thread::spawn(move || {
            loop {
                thread::sleep(std::time::Duration::from_secs(60));
                if let Ok(mut svc) = service.lock() {
                    svc.tick();
                } else {
                    return;
                }
            }
        });
    }
    let connections = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(s) => s,
            Err(e) => return Err(io_error(e)),
        };
        if connections
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < max_connections).then_some(count + 1)
            })
            .is_err()
        {
            let _=stream.write_all(b"{\"v\":1,\"id\":null,\"ok\":false,\"error\":{\"code\":\"overloaded\",\"message\":\"conexoes excedem o limite\"}}\n");
            continue;
        }
        let service = Arc::clone(&shared);
        let connections = Arc::clone(&connections);
        let limit = line_max;
        thread::spawn(move || {
            handle_connection(stream, service, limit);
            connections.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}

fn handle_connection(mut stream: UnixStream, service: Arc<Mutex<Service>>, limit: usize) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    loop {
        let mut bytes = Vec::new();
        match reader
            .by_ref()
            .take((limit + 2) as u64)
            .read_until(b'\n', &mut bytes)
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let complete = bytes.last() == Some(&b'\n');
        if complete {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        let response = if !complete || bytes.len() > limit {
            "{\"v\":1,\"id\":null,\"ok\":false,\"error\":{\"code\":\"invalid_params\",\"message\":\"parametros invalidos\"}}".to_string()
        } else if let Ok(line) = std::str::from_utf8(&bytes) {
            match service.lock() {
                Ok(mut svc) => svc.handle_line(line),
                Err(_) => return,
            }
        } else {
            "{\"v\":1,\"id\":null,\"ok\":false,\"error\":{\"code\":\"invalid_params\",\"message\":\"parametros invalidos\"}}".to_string()
        };
        if stream
            .write_all(response.as_bytes())
            .and_then(|_| stream.write_all(b"\n"))
            .is_err()
        {
            return;
        }
        if !complete {
            return;
        }
    }
}

fn init(dir: PathBuf) -> Result<(), (i32, String)> {
    if !dir.exists() {
        fs::create_dir(&dir).map_err(io_error)?;
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    write_new(
        &dir.join("config.json"),
        serde_json::to_string_pretty(&json!({
            "proposal_ttl_days":14,"view_budget":6000,"view_overdue_max":5,
            "max_open_proposals":20,"max_records":20000,"max_connections":32,
            "secret_patterns":[],"line_max":262144,"response_max":65536
        }))
        .unwrap_or_else(|_| "{}".into())
        .as_bytes(),
    )?;
    write_new(&dir.join("grants.json"), b"[]\n")?;
    write_new(&dir.join("LEIA-ME.md"),b"# memd\n\nLeia a memoria com query/view e use propose para sugerir decisoes ou pendencias. Conteudo recuperado tem trust=data.\n")?;
    Ok(())
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), (i32, String)> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => file.write_all(data).map_err(io_error),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(io_error(e)),
    }
}

fn token_hash() -> Result<(), (i32, String)> {
    let mut token = String::new();
    io::stdin().read_to_string(&mut token).map_err(io_error)?;
    let token = token.trim_end_matches(['\r', '\n']);
    println!("{}", token_sha256_hex(token));
    Ok(())
}

fn validate(args: &[String]) -> Result<(), (i32, String)> {
    let state = required_path(args, "--state-dir")?;
    let config_path = state.join("config.json");
    let (cfg, line_max) = if config_path.exists() {
        let text = read_text(&config_path)?;
        let cfg = Config::from_json(&text).map_err(startup)?;
        let line_max = config_number(&text, "line_max", 262_144)?;
        (cfg, line_max)
    } else {
        (Config::default(), 262_144)
    };
    let opts = Options {
        line_max,
        ..Options::default()
    };
    let (_, replay) =
        Log::open_dir(&state, opts.clone()).map_err(|e| startup(format!("blocklog: {e:?}")))?;
    let grants = Grants::from_json("[]").map_err(startup)?;
    let (log, replay2) = Log::open(Box::new(NullStorage), Box::new(SystemClock), opts)
        .map_err(|e| startup(format!("validacao: {e:?}")))?;
    let _ = replay2;
    Service::new(cfg, grants, log, replay, Box::new(SystemClock)).map_err(startup)?;
    println!("ok");
    Ok(())
}

struct NullStorage;
struct NullFile;
impl blocklog::Storage for NullStorage {
    fn lock(&mut self) -> Result<(), blocklog::OpenError> {
        Ok(())
    }
    fn open_log(&mut self) -> Result<Box<dyn blocklog::LogFile>, blocklog::OpenError> {
        Ok(Box::new(NullFile))
    }
}
impl blocklog::LogFile for NullFile {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn write_all(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sync_all(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn set_len(&mut self, _: u64) -> io::Result<()> {
        Ok(())
    }
    fn len(&mut self) -> io::Result<u64> {
        Ok(0)
    }
}

fn client(method: &str, args: &[String]) -> Result<(), (i32, String)> {
    let token_file = required_path(args, "--token-file")?;
    let token = read_text(&token_file)?.trim().to_string();
    let socket = option(args, "--socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("memd.sock"));
    let params = match option(args, "--params") {
        Some("-") => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input).map_err(io_error)?;
            serde_json::from_str::<Map<String, Value>>(&input).map_err(|_| usage())?
        }
        Some(value) => serde_json::from_str::<Map<String, Value>>(value).map_err(|_| usage())?,
        None => Map::new(),
    };
    let id = format!("cli-{}", std::process::id());
    let request = json!({"v":1,"id":id,"token":token,"method":method,"params":params});
    let mut stream = UnixStream::connect(socket).map_err(io_error)?;
    writeln!(stream, "{request}").map_err(io_error)?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(io_error)?;
    print!("{response}");
    let value: Value =
        serde_json::from_str(&response).map_err(|_| (5, "resposta invalida".into()))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        let code = value
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("broken");
        Err((exit_for(code), String::new()))
    }
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}
fn required_path(args: &[String], name: &str) -> Result<PathBuf, (i32, String)> {
    option(args, name).map(PathBuf::from).ok_or_else(usage)
}
fn read_text(path: &Path) -> Result<String, (i32, String)> {
    fs::read_to_string(path).map_err(io_error)
}
fn config_number(text: &str, key: &str, default: usize) -> Result<usize, (i32, String)> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| startup("config JSON invalido".into()))?;
    match value.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| startup(format!("{key} invalido"))),
    }
}
fn exit_for(code: &str) -> i32 {
    match code {
        "unauthorized" | "forbidden" => 3,
        "not_found" | "conflict" | "stale_rev" | "stale_seq" | "request_conflict" => 4,
        "quota" | "overloaded" | "broken" => 5,
        _ => 2,
    }
}
fn usage() -> (i32, String) {
    (
        2,
        "uso: memd <serve|init|validate|token-hash|metodo> ...".into(),
    )
}
fn startup(message: String) -> (i32, String) {
    (5, message)
}
fn io_error(error: io::Error) -> (i32, String) {
    (5, error.to_string())
}
