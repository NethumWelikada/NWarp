use crate::config::Config;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::server::connection;
use crate::wasm::WasmTable;
use arc_swap::ArcSwap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// Runs the plain HTTP accept loop. Phase 4: each accepted connection
/// becomes a lightweight Tokio task (`tokio::spawn`) instead of an OS
/// thread - the async runtime multiplexes many tasks onto
/// `worker_threads` real OS threads via an epoll-based reactor
/// (Tokio's default on Linux), so the number of concurrent connections
/// is no longer bounded by how many threads the OS can schedule.
pub async fn run(cfg: Config, config_path: String) -> std::io::Result<()> {
    let logger = Arc::new(Logger::new(&cfg.access_log, &cfg.error_log));
    let proxy_table: Arc<ArcSwap<ProxyTable>> =
        Arc::new(ArcSwap::from_pointee(ProxyTable::from_config(&cfg)));
    let wasm_table: Arc<ArcSwap<WasmTable>> =
        Arc::new(ArcSwap::from_pointee(WasmTable::from_config(&cfg)));
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

    println!("Config hot-reload: watching {} for proxy_route/wasm_route changes", config_path);

    if proxy_table.load().has_routes() {
        crate::proxy::spawn_health_checker(
            proxy_table.load_full(),
            Duration::from_secs(cfg.health_check_interval_secs),
            Duration::from_secs(cfg.health_check_timeout_secs),
        );
    }

    // Phase 7: watch the config file for changes and hot-swap the
    // proxy/WASM routing tables without restarting the server. See
    // docs/ARCHITECTURE.md for exactly what does and doesn't reload.
    spawn_config_watcher(
        config_path,
        Arc::clone(&proxy_table),
        Arc::clone(&wasm_table),
        Arc::clone(&logger),
        cfg.health_check_interval_secs,
        cfg.health_check_timeout_secs,
    );

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

        // Phase 5.5: HTTP/3 over QUIC/UDP, same port number as the TLS
        // TCP listener. Requires TLS to be enabled (QUIC mandates
        // TLS 1.3), reuses the same certs. Runs independently of the
        // TCP-based TLS listener above since QUIC is UDP-based.
        let quic_cfg = Arc::clone(&cfg);
        let quic_logger = Arc::clone(&logger);
        let quic_proxy_table = Arc::clone(&proxy_table);
        let quic_wasm_table = Arc::clone(&wasm_table);
        tokio::spawn(async move {
            if let Err(e) =
                crate::http3::run(quic_cfg, Arc::clone(&quic_logger), quic_proxy_table, quic_wasm_table)
                    .await
            {
                quic_logger.error(&format!("HTTP/3 (QUIC) listener failed to start: {}", e));
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

/// Watches the config file's modification time and, on change,
/// reloads `proxy_route` and `wasm_route` entries into fresh tables
/// swapped in atomically (via `ArcSwap`, lock-free for readers) - in
/// flight requests are unaffected, and the very next request after a
/// reload sees the new routes. A fresh health-checker task is spawned
/// for the new proxy table's targets (starting from the same
/// optimistic "healthy until proven otherwise" state as a cold start).
///
/// Deliberately NOT hot-reloaded: `host`, `port`, `tls_*`,
/// `worker_threads` - these are bound into already-listening sockets
/// and an already-sized runtime, and changing them safely requires a
/// full restart. See docs/ARCHITECTURE.md for the full scope note.
fn spawn_config_watcher(
    config_path: String,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
    logger: Arc<Logger>,
    health_check_interval_secs: u64,
    health_check_timeout_secs: u64,
) {
    tokio::spawn(async move {
        let mut last_modified = std::fs::metadata(&config_path).and_then(|m| m.modified()).ok();

        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;

            let modified = match std::fs::metadata(&config_path).and_then(|m| m.modified()) {
                Ok(m) => m,
                Err(_) => continue,
            };

            if Some(modified) == last_modified {
                continue;
            }
            last_modified = Some(modified);

            let new_cfg = Config::load(&config_path);
            let new_proxy_table = Arc::new(ProxyTable::from_config(&new_cfg));
            let new_wasm_table = WasmTable::from_config(&new_cfg);

            let proxy_route_count = new_cfg.proxy_routes.len();
            let wasm_route_count = new_cfg.wasm_routes.len();

            if new_proxy_table.has_routes() {
                crate::proxy::spawn_health_checker(
                    Arc::clone(&new_proxy_table),
                    Duration::from_secs(health_check_interval_secs),
                    Duration::from_secs(health_check_timeout_secs),
                );
            }

            proxy_table.store(new_proxy_table);
            wasm_table.store(Arc::new(new_wasm_table));

            logger.error(&format!(
                "config reloaded from {} (proxy_routes: {}, wasm_routes: {})",
                config_path, proxy_route_count, wasm_route_count
            ));
        }
    });
}
