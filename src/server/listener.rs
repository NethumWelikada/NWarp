use crate::config::Config;
use crate::logging::Logger;
use crate::server::connection;
use crate::server::pool::ThreadPool;
use std::net::TcpListener;
use std::sync::Arc;

pub fn run(cfg: Config) -> std::io::Result<()> {
    let logger = Arc::new(Logger::new(&cfg.access_log, &cfg.error_log));
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = TcpListener::bind(&addr)?;
    let pool = ThreadPool::new(cfg.worker_threads);
    let cfg = Arc::new(cfg);

    println!("{} listening on http://{}", cfg.server_name, addr);
    println!("Serving files from: {}", cfg.document_root);
    println!("Worker threads: {}", cfg.worker_threads);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let cfg = Arc::clone(&cfg);
                let logger = Arc::clone(&logger);
                pool.execute(move || {
                    connection::handle(stream, cfg, logger);
                });
            }
            Err(e) => {
                logger.error(&format!("connection failed: {}", e));
            }
        }
    }

    Ok(())
}
