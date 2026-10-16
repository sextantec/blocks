//! attest: registro de quem verificou o que em qual versao do codigo.

mod cli;
mod commands;
mod log;
mod policy;
mod run_cmd;
mod sha256;
mod tree;

fn main() {
    std::process::exit(commands::execute());
}
