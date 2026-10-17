mod config;
mod grants;
mod model;
mod service;

pub use config::Config;
pub use grants::Grants;
pub use service::Service;

pub fn token_sha256_hex(token: &str) -> String {
    blocklog::sha256_hex(token.as_bytes())
}
