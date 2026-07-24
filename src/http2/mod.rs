use crate::config::Config;
use crate::http::request::Request as NwarpRequest;
use crate::http::response::Response as NwarpResponse;
use crate::http::router;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::wasm::WasmTable;
use arc_swap::ArcSwap;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

/// Serves HTTP/2 requests over an already-established connection (a
/// TLS stream that negotiated `h2` via ALPN - see tls/mod.rs). Each
/// HTTP/2 stream (the protocol's term for a single request/response
/// exchange, not to be confused with a Tokio task or a TCP stream) is
/// handled concurrently within the same connection, which is the
/// entire point of HTTP/2 multiplexing: many logical requests share
/// one TCP/TLS connection instead of needing one connection each.
pub async fn serve<S>(
    io: S,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
    peer: String,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut connection = match h2::server::handshake(io).await {
        Ok(conn) => conn,
        Err(e) => {
            logger.error(&format!("HTTP/2 handshake failed for {}: {}", peer, e));
            return;
        }
    };

    while let Some(result) = connection.accept().await {
        let (request, respond) = match result {
            Ok(pair) => pair,
            Err(e) => {
                logger.error(&format!("HTTP/2 stream error for {}: {}", peer, e));
                continue;
            }
        };

        let cfg = Arc::clone(&cfg);
        let logger = Arc::clone(&logger);
        let proxy_table = Arc::clone(&proxy_table);
        let wasm_table = Arc::clone(&wasm_table);
        let peer = peer.clone();

        tokio::spawn(async move {
            handle_stream(request, respond, cfg, logger, proxy_table, wasm_table, peer).await;
        });
    }
}

/// Converts an h2 stream's request into our internal `Request` type,
/// routes it (proxy or static, same logic as HTTP/1.1), and converts
/// our internal `Response` back into an h2 response + data frame.
async fn handle_stream(
    h2_request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
    peer: String,
) {
    let request = to_nwarp_request(&h2_request);

    // Proxying is HTTP/1.1-to-upstream regardless of how the client
    // connected (matches how most reverse proxies operate - the
    // client-facing and upstream-facing protocols are independent).
    // The raw HTTP/1.1 response from proxy::relay_raw is parsed back
    // into status/headers/body here to bridge it onto the h2 stream.
    let proxy_snapshot = proxy_table.load();
    if let Some(route) = proxy_snapshot.match_route(&request.path) {
        match crate::proxy::relay_raw(&request, route, &peer).await {
            Ok(raw_bytes) => {
                let (status, body) = crate::proxy::split_raw_response(&raw_bytes);
                logger.access(&request.method, &request.path, status, &peer);
                send_h2_response(&mut respond, status, "application/octet-stream", &body, &cfg);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {
                logger.error(&format!("no healthy upstream for {} {}", request.method, request.path));
                logger.access(&request.method, &request.path, 503, &peer);
                let body = format!("<h1>503 Service Unavailable</h1><p>{}</p>", cfg.server_name);
                send_h2_response(&mut respond, 503, "text/html; charset=utf-8", body.as_bytes(), &cfg);
            }
            Err(e) => {
                logger.error(&format!("proxy upstream error for {} {} -> {}", request.method, request.path, e));
                logger.access(&request.method, &request.path, 502, &peer);
                let body = format!("<h1>502 Bad Gateway</h1><p>{}</p>", cfg.server_name);
                send_h2_response(&mut respond, 502, "text/html; charset=utf-8", body.as_bytes(), &cfg);
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
                send_h2_response(&mut respond, status, "text/plain; charset=utf-8", &body, &cfg);
            }
            Err(e) => {
                logger.error(&format!("WASM handler error for {} {} -> {}", request.method, request.path, e));
                logger.access(&request.method, &request.path, 500, &peer);
                let body = format!("<h1>500 Internal Server Error</h1><p>{}</p>", cfg.server_name);
                send_h2_response(&mut respond, 500, "text/html; charset=utf-8", body.as_bytes(), &cfg);
            }
        }
        return;
    }
    drop(wasm_snapshot);

    let response = router::route(&request, &cfg).await;
    logger.access(&request.method, &request.path, response.status_code, &peer);
    send_h2_response(
        &mut respond,
        response.status_code,
        &response.content_type,
        &response.body,
        &cfg,
    );
}

fn to_nwarp_request(req: &http::Request<h2::RecvStream>) -> NwarpRequest {
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
        version: "HTTP/2".to_string(),
        headers,
    }
}

fn send_h2_response(
    respond: &mut h2::server::SendResponse<Bytes>,
    status: u16,
    content_type: &str,
    body: &[u8],
    cfg: &Config,
) {
    let built = http::Response::builder()
        .status(status)
        .header("server", cfg.server_name.as_str())
        .header("content-type", content_type)
        .header("content-length", body.len().to_string())
        .body(())
        .expect("building a well-formed h2 response header set");

    match respond.send_response(built, false) {
        Ok(mut send_stream) => {
            let _ = send_stream.send_data(Bytes::copy_from_slice(body), true);
        }
        Err(_) => {
            // Client likely reset the stream mid-response; nothing more
            // to do - h2 handles stream-level errors internally.
        }
    }
}

/// Also usable to build an all-in-one HTTP/2 Response from our
/// internal Response type directly (kept for symmetry/documentation -
/// the static-file and error paths above call send_h2_response inline).
#[allow(dead_code)]
pub fn from_nwarp_response(resp: &NwarpResponse) -> (u16, String, Vec<u8>) {
    (resp.status_code, resp.content_type.clone(), resp.body.clone())
}

