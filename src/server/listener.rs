use crate::config::Config;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::server::connection;
use crate::server::pool::ThreadPool;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

pub fn run(cfg: Config) -> std::io::Result<()> {
    let logger = Arc::new(Logger::new(&cfg.access_log, &cfg.error_log));
    let proxy_table = Arc::new(ProxyTable::from_config(&cfg));
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = TcpListener::bind(&addr)?;
    let pool = ThreadPool::new(cfg.worker_threads);
    let cfg = Arc::new(cfg);

    println!("{} listening on http://{}", cfg.server_name, addr);
    println!("Serving files from: {}", cfg.document_root);
    println!("Worker threads: {}", cfg.worker_threads);
    if !cfg.proxy_routes.is_empty() {
        for (prefix, upstreams) in &cfg.proxy_routes {
            println!("Proxying {} -> {}", prefix, upstreams.join(", "));
        }
        println!(
            "Health checks: every {}s, {}s timeout",
            cfg.health_check_interval_secs, cfg.health_check_timeout_secs
        );
    }

    if proxy_table.has_routes() {
        crate::proxy::spawn_health_checker(
            Arc::clone(&proxy_table),
            std::time::Duration::from_secs(cfg.health_check_interval_secs),
            std::time::Duration::from_secs(cfg.health_check_timeout_secs),
        );
    }

    // Phase 2: if TLS is enabled, run the HTTPS accept loop on its own
    // thread alongside plain HTTP. Failure to start TLS logs an error
    // but does not bring down the plain HTTP listener.
    if cfg.tls_enabled {
        let tls_cfg = Arc::clone(&cfg);
        let tls_logger = Arc::clone(&logger);
        let tls_proxy_table = Arc::clone(&proxy_table);
        thread::spawn(move || {
            if let Err(e) = crate::tls::run(tls_cfg, Arc::clone(&tls_logger), tls_proxy_table) {
                tls_logger.error(&format!("TLS listener failed to start: {}", e));
            }
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let cfg = Arc::clone(&cfg);
                let logger = Arc::clone(&logger);
                let proxy_table = Arc::clone(&proxy_table);
                pool.execute(move || {
                    connection::handle(stream, cfg, logger, proxy_table);
                });
            }
            Err(e) => {
                logger.error(&format!("connection failed: {}", e));
            }
        }
    }

    Ok(())
}
