mod config;
mod http;
mod logging;
mod server;
mod tls;

use config::Config;
use std::env;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("nwarp {}", VERSION);
        return;
    }

    // Config file path: --config <path>, else default location.
    let config_path = args
        .iter()
        .position(|a| a == "--config" || a == "-c")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "./configs/nwarp.conf".to_string());

    println!("NWarp v{} starting...", VERSION);
    println!("Loading config from: {}", config_path);

    let cfg = Config::load(&config_path);

    if let Err(e) = server::listener::run(cfg) {
        eprintln!("[nwarp] fatal: {}", e);
        std::process::exit(1);
    }
}
