use crate::config::Config;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::server::connection::handle_generic;
use crate::wasm::WasmTable;
use arc_swap::ArcSwap;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::fs::File;
use std::io::{self, BufReader};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Loads a PEM certificate chain from disk. Runs once at startup
/// (before the accept loop begins), so a blocking read here is fine.
/// Exposed to http3/mod.rs, which needs the same certs/key for its
/// QUIC-specific rustls config (see http3::build_quic_tls_config).
pub(crate) fn load_certs(path: &str) -> io::Result<Vec<Certificate>> {
    let file = File::open(path).map_err(|e| {
        io::Error::new(e.kind(), format!("could not open TLS cert '{}': {}", path, e))
    })?;
    let mut reader = BufReader::new(file);
    let raw = certs(&mut reader)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid certificate PEM"))?;
    Ok(raw.into_iter().map(Certificate).collect())
}

/// Loads a private key from disk, trying PKCS#8 first, then RSA.
pub(crate) fn load_private_key(path: &str) -> io::Result<PrivateKey> {
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
/// Advertises both `h2` and `http/1.1` via ALPN so TLS clients that
/// support HTTP/2 negotiate it automatically during the handshake -
/// see server::run below for how the negotiated protocol is used to
/// choose the HTTP/1.1 or HTTP/2 code path per connection.
pub fn build_tls_config(cfg: &Config) -> io::Result<Arc<ServerConfig>> {
    let certs = load_certs(&cfg.tls_cert)?;
    let key = load_private_key(&cfg.tls_key)?;

    let mut tls_config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(Arc::new(tls_config))
}

/// Runs the HTTPS accept loop on cfg.tls_port. Each connection is
/// accepted as a Tokio task (Phase 4), performs an async TLS handshake
/// via tokio-rustls, then is handed to the same `handle_generic`
/// request/response cycle used by plain HTTP - see
/// server/connection.rs.
pub async fn run(
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
) -> io::Result<()> {
    let tls_config = build_tls_config(&cfg)?;
    let acceptor = TlsAcceptor::from(tls_config);
    let addr = format!("{}:{}", cfg.host, cfg.tls_port);
    let listener = TcpListener::bind(&addr).await?;

    println!("{} listening on https://{}", cfg.server_name, addr);

    loop {
        match listener.accept().await {
            Ok((tcp, _)) => {
                let cfg = Arc::clone(&cfg);
                let logger = Arc::clone(&logger);
                let proxy_table = Arc::clone(&proxy_table);
                let wasm_table = Arc::clone(&wasm_table);
                let acceptor = acceptor.clone();

                tokio::spawn(async move {
                    let peer = tcp
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|_| "unknown".to_string());

                    match acceptor.accept(tcp).await {
                        Ok(mut tls_stream) => {
                            // ALPN negotiated during the TLS handshake tells us
                            // whether the client wants HTTP/2 or HTTP/1.1 (Phase 5).
                            let negotiated_h2 = tls_stream
                                .get_ref()
                                .1
                                .alpn_protocol()
                                == Some(b"h2".as_ref());

                            if negotiated_h2 {
                                crate::http2::serve(tls_stream, cfg, logger, proxy_table, wasm_table, peer)
                                    .await;
                            } else {
                                handle_generic(&mut tls_stream, peer, cfg, logger, proxy_table, wasm_table)
                                    .await;
                            }
                        }
                        Err(e) => {
                            logger.error(&format!("TLS handshake failed for {}: {}", peer, e));
                        }
                    }
                });
            }
            Err(e) => {
                logger.error(&format!("TLS connection accept failed: {}", e));
            }
        }
    }
}

