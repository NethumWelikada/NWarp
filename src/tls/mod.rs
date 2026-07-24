use crate::config::Config;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::server::connection::handle_generic;
use crate::server::pool::ThreadPool;
use rustls::{Certificate, PrivateKey, ServerConfig, ServerConnection, Stream};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::fs::File;
use std::io::{self, BufReader};
use std::net::TcpListener;
use std::sync::Arc;

/// Loads a PEM certificate chain from disk.
fn load_certs(path: &str) -> io::Result<Vec<Certificate>> {
    let file = File::open(path).map_err(|e| {
        io::Error::new(e.kind(), format!("could not open TLS cert '{}': {}", path, e))
    })?;
    let mut reader = BufReader::new(file);
    let raw = certs(&mut reader)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid certificate PEM"))?;
    Ok(raw.into_iter().map(Certificate).collect())
}

/// Loads a private key from disk, trying PKCS#8 first, then RSA.
fn load_private_key(path: &str) -> io::Result<PrivateKey> {
    let open = || {
        File::open(path).map_err(|e| {
            io::Error::new(e.kind(), format!("could not open TLS key '{}': {}", path, e))
        })
    };

    let mut reader = BufReader::new(open()?);
    let mut pkcs8 = pkcs8_private_keys(&mut reader)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid PKCS8 key PEM"))?;
    if let Some(key) = pkcs8.pop() {
        return Ok(PrivateKey(key));
    }

    let mut reader2 = BufReader::new(open()?);
    let mut rsa = rsa_private_keys(&mut reader2)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid RSA key PEM"))?;
    if let Some(key) = rsa.pop() {
        return Ok(PrivateKey(key));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("no usable private key found in '{}'", path),
    ))
}

/// Builds the rustls server config from the configured cert/key paths.
pub fn build_tls_config(cfg: &Config) -> io::Result<Arc<ServerConfig>> {
    let certs = load_certs(&cfg.tls_cert)?;
    let key = load_private_key(&cfg.tls_key)?;

    let tls_config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    Ok(Arc::new(tls_config))
}

/// Runs the HTTPS accept loop on cfg.tls_port. Each connection performs
/// a TLS handshake via rustls, then is handed to the same
/// `handle_generic` request/response cycle used by plain HTTP - see
/// server/connection.rs.
pub fn run(cfg: Arc<Config>, logger: Arc<Logger>, proxy_table: Arc<ProxyTable>) -> io::Result<()> {
    let tls_config = build_tls_config(&cfg)?;
    let addr = format!("{}:{}", cfg.host, cfg.tls_port);
    let listener = TcpListener::bind(&addr)?;
    let pool = ThreadPool::new(cfg.worker_threads);

    println!("{} listening on https://{}", cfg.server_name, addr);

    for stream in listener.incoming() {
        match stream {
            Ok(mut tcp) => {
                let cfg = Arc::clone(&cfg);
                let logger = Arc::clone(&logger);
                let tls_config = Arc::clone(&tls_config);
                let proxy_table = Arc::clone(&proxy_table);

                pool.execute(move || {
                    let peer = tcp
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|_| "unknown".to_string());

                    match ServerConnection::new(tls_config) {
                        Ok(mut conn) => {
                            let mut tls_stream = Stream::new(&mut conn, &mut tcp);
                            handle_generic(&mut tls_stream, peer, cfg, logger, proxy_table);
                        }
                        Err(e) => {
                            logger.error(&format!("TLS handshake setup failed for {}: {}", peer, e));
                        }
                    }
                });
            }
            Err(e) => {
                logger.error(&format!("TLS connection accept failed: {}", e));
            }
        }
    }

    Ok(())
}
