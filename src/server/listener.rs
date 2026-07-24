use crate::config::Config;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::server::connection;
use crate::wasm::WasmTable;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Runs the plain HTTP accept loop. Phase 4: each accepted connection
/// becomes a lightweight Tokio task (`tokio::spawn`) instead of an OS
/// thread - the async runtime multiplexes many tasks onto
/// `worker_threads` real OS threads via an epoll-based reactor
/// (Tokio's default on Linux), so the number of concurrent connections
/// is no longer bounded by how many threads the OS can schedule.
pub async fn run(cfg: Config) -> std::io::Result<()> {
    let logger = Arc::new(Logger::new(&cfg.access_log, &cfg.error_log));
    let proxy_table = Arc::new(ProxyTable::from_config(&cfg));
    let wasm_table = Arc::new(WasmTable::from_config(&cfg));
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = TcpListener::bind(&addr).await?;
    let cfg = Arc::new(cfg);

    println!("{} listening on http://{}", cfg.server_name, addr);
    println!("Serving files from: {}", cfg.document_root);
    println!("Runtime worker threads: {}", cfg.worker_threads);
    println!("Concurrency model: async event loop (Tokio, epoll on Linux)");

    if !cfg.proxy_routes.is_empty() {
        for (prefix, upstreams) in &cfg.proxy_routes {
            println!("Proxying {} -> {}", prefix, upstreams.join(", "));
        }
        println!(
            "Health checks: every {}s, {}s timeout",
            cfg.health_check_interval_secs, cfg.health_check_timeout_secs
        );
    }

    if !cfg.wasm_routes.is_empty() {
        for (prefix, path) in &cfg.wasm_routes {
            println!("WASM handler {} -> {}", prefix, path);
        }
    }

    if proxy_table.has_routes() {
        crate::proxy::spawn_health_checker(
            Arc::clone(&proxy_table),
            std::time::Duration::from_secs(cfg.health_check_interval_secs),
            std::time::Duration::from_secs(cfg.health_check_timeout_secs),
        );
    }

    // Phase 2: if TLS is enabled, run the HTTPS accept loop as its own
    // Tokio task alongside plain HTTP. Failure to start TLS logs an
    // error but does not bring down the plain HTTP listener.
    if cfg.tls_enabled {
        let tls_cfg = Arc::clone(&cfg);
        let tls_logger = Arc::clone(&logger);
        let tls_proxy_table = Arc::clone(&proxy_table);
        let tls_wasm_table = Arc::clone(&wasm_table);
        tokio::spawn(async move {
            if let Err(e) =
                crate::tls::run(tls_cfg, Arc::clone(&tls_logger), tls_proxy_table, tls_wasm_table)
                    .await
            {
                tls_logger.error(&format!("TLS listener failed to start: {}", e));
            }
        });
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let cfg = Arc::clone(&cfg);
                let logger = Arc::clone(&logger);
                let proxy_table = Arc::clone(&proxy_table);
                let wasm_table = Arc::clone(&wasm_table);
                tokio::spawn(async move {
                    connection::handle(stream, cfg, logger, proxy_table, wasm_table).await;
                });
            }
            Err(e) => {
                logger.error(&format!("connection failed: {}", e));
            }
        }
    }
}
