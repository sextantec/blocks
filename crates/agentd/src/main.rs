mod config;
mod daemon;
mod rules;
mod time;
mod tmux;

fn main() {
    if let Err((code, message)) = daemon::run(std::env::args().skip(1).collect()) {
        eprintln!("agentd: {message}");
        std::process::exit(code);
    }
}
