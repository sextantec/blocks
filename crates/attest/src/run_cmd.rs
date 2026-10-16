//! Execucao de comandos sem shell, com repasse, hash incremental e limite de tempo.

use crate::cli::Run;
use crate::sha256::Sha256;
use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

pub struct Outcome {
    pub argv: Vec<String>,
    pub exit: Option<i32>,
    pub code: i32,
    pub duration_ms: u64,
    pub output_sha256: String,
    pub output_bytes: u64,
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

pub fn run(args: &Run) -> Outcome {
    let argv: Vec<String> = args
        .argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let empty_hash = Sha256::default().finalize_hex();
    let started = Instant::now();
    let mut command = Command::new(&args.argv[0]);
    command
        .args(&args.argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: setsid runs only in the forked child before exec; success makes the child PID
    // the session and process-group ID, which timeout handling later targets with killpg.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            return Outcome {
                argv,
                exit: None,
                code: 127,
                duration_ms: elapsed_ms(started.elapsed()),
                output_sha256: empty_hash,
                output_bytes: 0,
            };
        }
    };
    match capture(&mut child, args.timeout, started) {
        Ok((status, timed_out, hash, bytes, duration_ms)) => {
            let exit = if timed_out {
                None
            } else {
                status.and_then(|s| s.code())
            };
            let code = if timed_out {
                124
            } else {
                status.map_or(1, shell_code)
            };
            Outcome {
                argv,
                exit,
                code,
                duration_ms,
                output_sha256: hash,
                output_bytes: bytes,
            }
        }
        Err(_) => Outcome {
            argv,
            exit: None,
            code: 127,
            duration_ms: elapsed_ms(started.elapsed()),
            output_sha256: empty_hash,
            output_bytes: 0,
        },
    }
}

fn capture(
    child: &mut Child,
    timeout_secs: u64,
    started: Instant,
) -> io::Result<(Option<ExitStatus>, bool, String, u64, u64)> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("stdout ausente"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("stderr ausente"))?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let out_thread = read_pipe(stdout, Stream::Stdout, sender.clone());
    let err_thread = read_pipe(stderr, Stream::Stderr, sender.clone());
    drop(sender);
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let mut hasher = Sha256::default();
    let mut output_bytes = 0u64;
    let deadline = started + Duration::from_secs(timeout_secs);
    let mut status = None;
    let mut timed_out = false;
    let mut rx_open = true;

    while rx_open {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if !timed_out && Instant::now() >= deadline && (status.is_none() || rx_open) {
            kill_group(child.id());
            let _ = child.kill();
            if status.is_none() {
                status = Some(child.wait()?);
            }
            timed_out = true;
        }
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok((stream, bytes)) => {
                hasher.update(&bytes);
                output_bytes = output_bytes.saturating_add(bytes.len() as u64);
                match stream {
                    Stream::Stdout => {
                        let _ = stdout.write_all(&bytes);
                        let _ = stdout.flush();
                    }
                    Stream::Stderr => {
                        let _ = stderr.write_all(&bytes);
                        let _ = stderr.flush();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                rx_open = !out_thread.is_finished() || !err_thread.is_finished();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => rx_open = false,
        }
    }
    let _ = out_thread.join();
    let _ = err_thread.join();
    if status.is_none() {
        status = Some(child.wait()?);
    }
    let duration_ms = elapsed_ms(started.elapsed());
    Ok((
        status,
        timed_out,
        hasher.finalize_hex(),
        output_bytes,
        duration_ms,
    ))
}

fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
    stream: Stream,
    sender: SyncSender<(Stream, Vec<u8>)>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if sender.send((stream, buffer[..count].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn kill_group(pid: u32) {
    // SAFETY: child executables are launched with setsid, so -pid names only that child's group.
    let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
}

fn shell_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}
