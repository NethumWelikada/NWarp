mod config;
mod http;
mod logging;
mod proxy;
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

    // The Tokio runtime's worker thread count is config-driven
    // (`worker_threads`), which is only known after loading the config
    // file above - so the runtime is built manually here rather than
    // via the #[tokio::main] macro, which fixes the thread count at
    // compile time.
    let worker_threads = cfg.worker_threads.max(1);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[nwarp] fatal: failed to start async runtime: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = runtime.block_on(server::listener::run(cfg)) {
        eprintln!("[nwarp] fatal: {}", e);
        std::process::exit(1);
    }
}
