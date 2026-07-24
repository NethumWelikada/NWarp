use crate::config::Config;
use crate::http::request::Request as NwarpRequest;
use crate::http::router;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::wasm::WasmTable;
use arc_swap::ArcSwap;
use bytes::Bytes;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

/// Builds a QUIC-specific rustls ServerConfig: TLS 1.3 only (QUIC does
/// not support earlier TLS versions at all) and ALPN fixed to `h3`.
/// This is deliberately a separate rustls config from the one used for
/// TCP/TLS (tls::build_tls_config, which advertises h2/http1.1) - QUIC
/// negotiates its own ALPN independently of the TCP listener.
fn build_quic_tls_config(cfg: &Config) -> io::Result<rustls::ServerConfig> {
    let certs = crate::tls::load_certs(&cfg.tls_cert)?;
    let key = crate::tls::load_private_key(&cfg.tls_key)?;

    let mut tls_config = rustls::ServerConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    tls_config.alpn_protocols = vec![b"h3".to_vec()];
    tls_config.max_early_data_size = u32::MAX;

    Ok(tls_config)
}

/// Runs the HTTP/3 (QUIC, over UDP) listener on the same port number
/// as the TLS/TCP listener (`tls_port`) - this mirrors how real-world
/// servers and browsers expect HTTP/3 to be reachable: the same port
/// number, just over UDP instead of TCP, discovered in production via
/// an `Alt-Svc` response header (not yet wired up here - see the
/// limitations note in docs/ARCHITECTURE.md).
pub async fn run(
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
) -> io::Result<()> {
    let quic_tls_config = build_quic_tls_config(&cfg)?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_tls_config));

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.tls_port)
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("bad bind address: {}", e)))?;

    let endpoint = quinn::Endpoint::server(server_config, addr)?;

    println!("{} listening on https://{} (HTTP/3, QUIC/UDP)", cfg.server_name, addr);

    while let Some(connecting) = endpoint.accept().await {
        let cfg = Arc::clone(&cfg);
        let logger = Arc::clone(&logger);
        let proxy_table = Arc::clone(&proxy_table);
        let wasm_table = Arc::clone(&wasm_table);

        tokio::spawn(async move {
            let quinn_conn = match connecting.await {
                Ok(conn) => conn,
                Err(e) => {
                    logger.error(&format!("QUIC handshake failed: {}", e));
                    return;
                }
            };

            let peer = quinn_conn.remote_address().to_string();
            let h3_conn = h3_quinn::Connection::new(quinn_conn);

            let mut h3_conn: h3::server::Connection<h3_quinn::Connection, Bytes> =
                match h3::server::Connection::new(h3_conn).await {
                    Ok(c) => c,
                    Err(e) => {
                        logger.error(&format!("HTTP/3 setup failed for {}: {}", peer, e));
                        return;
                    }
                };

            loop {
                match h3_conn.accept().await {
                    Ok(Some((request, stream))) => {
                        let cfg = Arc::clone(&cfg);
                        let logger = Arc::clone(&logger);
                        let proxy_table = Arc::clone(&proxy_table);
                        let wasm_table = Arc::clone(&wasm_table);
                        let peer = peer.clone();
                        tokio::spawn(async move {
                            handle_stream(request, stream, cfg, logger, proxy_table, wasm_table, peer)
                                .await;
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        logger.error(&format!("HTTP/3 stream error for {}: {}", peer, e));
                        break;
                    }
                }
            }
        });
    }

    Ok(())
}

async fn handle_stream(
    h3_request: http::Request<()>,
    mut stream: h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
    peer: String,
) {
    let request = to_nwarp_request(&h3_request);

    let proxy_snapshot = proxy_table.load();
    if let Some(route) = proxy_snapshot.match_route(&request.path) {
        match crate::proxy::relay_raw(&request, route, &peer).await {
            Ok(raw_bytes) => {
                let (status, body) = crate::proxy::split_raw_response(&raw_bytes);
                logger.access(&request.method, &request.path, status, &peer);
                send_h3_response(&mut stream, status, "application/octet-stream", &body, &cfg).await;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {
                logger.error(&format!("no healthy upstream for {} {}", request.method, request.path));
                logger.access(&request.method, &request.path, 503, &peer);
                let body = format!("<h1>503 Service Unavailable</h1><p>{}</p>", cfg.server_name);
                send_h3_response(&mut stream, 503, "text/html; charset=utf-8", body.as_bytes(), &cfg).await;
            }
            Err(e) => {
                logger.error(&format!("proxy upstream error for {} {} -> {}", request.method, request.path, e));
                logger.access(&request.method, &request.path, 502, &peer);
                let body = format!("<h1>502 Bad Gateway</h1><p>{}</p>", cfg.server_name);
                send_h3_response(&mut stream, 502, "text/html; charset=utf-8", body.as_bytes(), &cfg).await;
            }
        }
        return;
    }
    drop(proxy_snapshot);

    let wasm_snapshot = wasm_table.load();
    if let Some(route) = wasm_snapshot.match_route(&request.path) {
        match crate::wasm::invoke(route, &request.method, &request.path) {
            Ok((status, body)) => {
                logger.access(&request.method, &request.path, status, &peer);
                send_h3_response(&mut stream, status, "text/plain; charset=utf-8", &body, &cfg).await;
            }
            Err(e) => {
                logger.error(&format!("WASM handler error for {} {} -> {}", request.method, request.path, e));
                logger.access(&request.method, &request.path, 500, &peer);
                let body = format!("<h1>500 Internal Server Error</h1><p>{}</p>", cfg.server_name);
                send_h3_response(&mut stream, 500, "text/html; charset=utf-8", body.as_bytes(), &cfg).await;
            }
        }
        return;
    }
    drop(wasm_snapshot);

    let response = router::route(&request, &cfg).await;
    logger.access(&request.method, &request.path, response.status_code, &peer);
    send_h3_response(
        &mut stream,
        response.status_code,
        &response.content_type,
        &response.body,
        &cfg,
    )
    .await;
}

fn to_nwarp_request(req: &http::Request<()>) -> NwarpRequest {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let mut headers = HashMap::new();
    for (name, value) in req.headers() {
        if let Ok(v) = value.to_str() {
            headers.insert(name.as_str().to_lowercase(), v.to_string());
        }
    }
    NwarpRequest {
        method,
        path,
        version: "HTTP/3".to_string(),
        headers,
    }
}

async fn send_h3_response(
    stream: &mut h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
    status: u16,
    content_type: &str,
    body: &[u8],
    cfg: &Config,
) {
    let built = http::Response::builder()
        .status(status)
        .header("server", cfg.server_name.as_str())
        .header("content-type", content_type)
        .body(())
        .expect("building a well-formed h3 response header set");

    if stream.send_response(built).await.is_err() {
        // Client likely reset the stream mid-response; nothing more to do.
        return;
    }
    let _ = stream.send_data(Bytes::copy_from_slice(body)).await;
    let _ = stream.finish().await;
}

