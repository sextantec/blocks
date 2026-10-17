use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use blocklog::{Log, Options, SystemClock};
use grantd::{Config, Grants, Service, token_sha256_hex};
use serde_json::{Map, Value, json};

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err((code, message)) => {
            if !message.is_empty() {
                eprintln!("{message}");
            }
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
        "check" | "use" | "explain" | "list" | "show" | "request" | "delegate" | "grant"
        | "deny-rule" | "revoke" | "approve-request" | "reject-request" | "whoami" | "status" => {
            client(&command, &rest)
        }
        _ => Err(usage()),
    }
}

fn serve(args: &[String]) -> Result<(), (i32, String)> {
    let state = required_path(args, "--state-dir")?;
    let config_path = required_path(args, "--config")?;
    let grants_path = required_path(args, "--grants")?;
    let socket = option(args, "--socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| state.join("grantd.sock"));
    let config_text = read_text(&config_path)?;
    let config = Config::from_json(&config_text).map_err(startup)?;
    let grants = Grants::from_json(&read_text(&grants_path)?).map_err(startup)?;
    let opts = Options {
        line_max: config.line_max,
        ..Options::default()
    };
    let (log, replay) =
        Log::open_dir(&state, opts).map_err(|e| startup(format!("blocklog: {e}")))?;
    let service = Service::new(config.clone(), grants, log, replay, Box::new(SystemClock))
        .map_err(startup)?;
    let shared = Arc::new(Mutex::new(service));
    install_reload_handler(Arc::clone(&shared), grants_path)?;

    if socket.exists() {
        return Err(startup("socket ja existe".into()));
    }
    let listener = UnixListener::bind(&socket).map_err(io_error)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(io_error)?;

    let connections = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let mut stream = incoming.map_err(io_error)?;
        if connections
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < config.max_connections).then_some(count + 1)
            })
            .is_err()
        {
            let _ = stream.write_all(b"{\"v\":1,\"id\":null,\"ok\":false,\"error\":{\"code\":\"overloaded\",\"message\":\"connection limit\"}}\n");
            continue;
        }
        let service = Arc::clone(&shared);
        let active = Arc::clone(&connections);
        let line_max = config.line_max;
        thread::spawn(move || {
            handle_connection(stream, service, line_max);
            active.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}

fn install_reload_handler(
    service: Arc<Mutex<Service>>,
    grants_path: PathBuf,
) -> Result<(), (i32, String)> {
    let mut signals =
        signal_hook::iterator::Signals::new([signal_hook::consts::SIGHUP]).map_err(io_error)?;
    thread::spawn(move || {
        for _ in signals.forever() {
            let Ok(contents) = fs::read_to_string(&grants_path) else {
                eprintln!("grantd: SIGHUP grants reload failed; keeping previous profiles");
                continue;
            };
            let Ok(grants) = Grants::from_json(&contents) else {
                eprintln!("grantd: SIGHUP grants reload failed; keeping previous profiles");
                continue;
            };
            match service.lock() {
                Ok(mut service) => service.reload_grants(grants),
                Err(_) => return,
            }
        }
    });
    Ok(())
}

fn handle_connection(mut stream: UnixStream, service: Arc<Mutex<Service>>, limit: usize) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    loop {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take((limit.saturating_add(2)) as u64)
            .read_until(b'\n', &mut bytes);
        match read {
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
            invalid_line_response()
        } else if let Ok(line) = std::str::from_utf8(&bytes) {
            match service.lock() {
                Ok(mut service) => service.handle_line(line),
                Err(_) => return,
            }
        } else {
            invalid_line_response()
        };
        if stream
            .write_all(response.as_bytes())
            .and_then(|()| stream.write_all(b"\n"))
            .is_err()
        {
            return;
        }
        if !complete {
            return;
        }
    }
}

fn invalid_line_response() -> String {
    json!({"v":1,"id":null,"ok":false,"error":{"code":"invalid_params","message":"invalid request"}}).to_string()
}

fn init(dir: PathBuf) -> Result<(), (i32, String)> {
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(io_error)?;
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    let config = json!({
        "max_pending_requests":5,
        "line_max":65536,
        "response_max":65536,
        "max_connections":32
    });
    write_new(
        &dir.join("config.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".into())
        )
        .as_bytes(),
    )?;
    write_new(&dir.join("grants.json"), b"[]\n")?;
    write_new(
        &dir.join("LEIA-ME.md"),
        b"# grantd\n\nEste diretorio contem a configuracao e os perfis de acesso do grantd. Cadastre perfis com tokens gerados fora deste modelo; o grantd guarda apenas o SHA-256 do token. Concessoes ficam no log.jsonl.\n",
    )?;
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
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(io_error(error)),
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
    let json_mode = args.iter().any(|arg| arg == "--json");
    match validate_inner(args) {
        Ok(t_max) => {
            if json_mode {
                println!("{}", json!({"ok":true,"t_max":t_max}));
            } else {
                println!("ok t_max={t_max}");
            }
            Ok(())
        }
        Err((code, error)) => {
            if json_mode {
                println!("{}", json!({"ok":false,"error":error}));
                Err((code, String::new()))
            } else {
                Err((code, error))
            }
        }
    }
}

fn validate_inner(args: &[String]) -> Result<String, (i32, String)> {
    let state = required_path(args, "--state-dir")?;
    let config_path = state.join("config.json");
    let config = if config_path.exists() {
        Config::from_json(&read_text(&config_path)?).map_err(|error| (4, error))?
    } else {
        Config::default()
    };
    let options = Options {
        line_max: config.line_max,
        ..Options::default()
    };
    let (log, replay) =
        Log::open_dir(&state, options).map_err(|error| (4, format!("blocklog: {error}")))?;
    let t_max = replay
        .last()
        .map(|commit| commit.ts.clone())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let service = Service::new(
        config,
        Grants::default(),
        log,
        replay,
        Box::new(SystemClock),
    )
    .map_err(|error| (4, error))?;
    if service.is_broken() {
        return Err((4, "broken".into()));
    }
    Ok(t_max)
}

fn client(method: &str, args: &[String]) -> Result<(), (i32, String)> {
    let token_path = option(args, "--token-file")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("GRANTD_TOKEN_FILE").map(PathBuf::from))
        .ok_or_else(usage)?;
    let token = read_text(&token_path)?.trim().to_owned();
    let socket = option(args, "--socket")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("GRANTD_SOCKET").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("grantd.sock"));
    let params = params_from_args(method, args)?;
    let id = format!("cli-{}", std::process::id());
    let method = method.replace('-', "_");
    let request = json!({"v":1,"id":id,"token":token,"method":method,"params":params});
    let mut stream = UnixStream::connect(socket).map_err(io_error)?;
    writeln!(stream, "{request}").map_err(io_error)?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(65_538)
        .read_line(&mut response)
        .map_err(io_error)?;
    let value: Value =
        serde_json::from_str(&response).map_err(|_| (5, "invalid response".into()))?;
    let ok = value.get("ok").and_then(Value::as_bool) == Some(true);
    let json_mode = args.iter().any(|a| a == "--json");
    if json_mode || !ok || method != "check" {
        println!("{}", value);
    } else {
        print_check(&value);
    }
    if ok {
        if method == "check"
            && value.pointer("/result/decision").and_then(Value::as_str) == Some("deny")
        {
            return Err((3, String::new()));
        }
        Ok(())
    } else {
        let code = value
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("broken");
        Err((exit_for(code), String::new()))
    }
}

fn params_from_args(method: &str, args: &[String]) -> Result<Map<String, Value>, (i32, String)> {
    let mut params = Map::new();
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--json" {
            i += 1;
            continue;
        }
        if key == "--token-file" || key == "--socket" {
            i += 2;
            continue;
        }
        if key == "--params" {
            let value = args.get(i + 1).ok_or_else(usage)?;
            let stdin_params;
            let contents = if value == "-" {
                let mut input = String::new();
                io::stdin().read_to_string(&mut input).map_err(io_error)?;
                stdin_params = input;
                stdin_params.as_str()
            } else {
                value.as_str()
            };
            let parsed =
                serde_json::from_str::<Map<String, Value>>(contents).map_err(|_| usage())?;
            params.extend(parsed);
            i += 2;
            continue;
        }
        let Some(name) = key.strip_prefix("--") else {
            return Err(usage());
        };
        if !matches!(method, "check" | "use" | "explain")
            || !matches!(
                name,
                "agent" | "action" | "resource" | "qty" | "unit" | "request-id"
            )
        {
            return Err(usage());
        }
        let value = args.get(i + 1).ok_or_else(usage)?;
        let parsed =
            serde_json::from_str::<Value>(value).unwrap_or_else(|_| Value::String(value.clone()));
        params.insert(name.replace('-', "_"), parsed);
        i += 2;
    }
    Ok(params)
}

fn print_check(value: &Value) {
    let Some(result) = value.get("result") else {
        println!("deny no_grant");
        return;
    };
    if result.get("decision").and_then(Value::as_str) == Some("allow") {
        let id = result
            .get("grant")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if let Some(until) = result.get("until").and_then(Value::as_str) {
            println!("allow {id} ate {until}");
        } else {
            println!("allow {id}");
        }
    } else {
        let reason = result
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("no_grant");
        println!("deny {reason}");
    }
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].as_str())
}

fn required_path(args: &[String], name: &str) -> Result<PathBuf, (i32, String)> {
    option(args, name).map(PathBuf::from).ok_or_else(usage)
}

fn read_text(path: &Path) -> Result<String, (i32, String)> {
    fs::read_to_string(path).map_err(io_error)
}

fn exit_for(code: &str) -> i32 {
    match code {
        "unauthorized" | "forbidden" | "denied" => 3,
        "not_found" | "request_conflict" | "conflict" | "exceeds_parent" => 4,
        "quota" | "overloaded" | "broken" => 5,
        _ => 2,
    }
}

fn usage() -> (i32, String) {
    (
        2,
        "usage: grantd <serve|init|validate|token-hash|check|use|explain|request|delegate|grant|deny-rule|revoke|approve-request|reject-request|list|show|whoami|status> ...".into(),
    )
}

fn startup(message: String) -> (i32, String) {
    (5, message)
}

fn io_error(error: io::Error) -> (i32, String) {
    (5, error.to_string())
}
