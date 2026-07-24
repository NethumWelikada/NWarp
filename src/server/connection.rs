use crate::config::Config;
use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::router;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use crate::wasm::WasmTable;
use arc_swap::ArcSwap;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

/// Handles a single client connection end-to-end over a plain TCP
/// socket: parse -> route (proxy, WASM, or static) -> respond. Runs as
/// its own Tokio task rather than its own OS thread (Phase 4) - many
/// thousands of these can be in flight at once on a small pool of
/// worker threads, since each one yields at every `.await` instead of
/// blocking an entire thread while waiting on I/O.
pub async fn handle(
    mut stream: TcpStream,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    handle_generic(&mut stream, peer, cfg, logger, proxy_table, wasm_table).await;
}

/// Shared request/response cycle, generic over any stream that can be
/// read from and written to asynchronously. This is what lets the
/// exact same routing and logging logic serve both plain HTTP
/// (Tokio TcpStream) and HTTPS (tokio-rustls TlsStream) without
/// duplicating code - see server/tls.rs.
///
/// Routing precedence: proxy routes are checked first (Phase 3), then
/// WASM routes (Phase 6), then static file serving (Phase 1) as the
/// final fallback. `proxy_table`/`wasm_table` are `ArcSwap`-wrapped
/// (Phase 7) so a config hot-reload can swap in new routing tables
/// without restarting the server - each request loads the current
/// snapshot at the start of the request, so in-flight requests are
/// never affected by a reload happening mid-request.
pub async fn handle_generic<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    peer: String,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ArcSwap<ProxyTable>>,
    wasm_table: Arc<ArcSwap<WasmTable>>,
) {
    let request = match Request::parse(stream).await {
        Ok(r) => r,
        Err(e) => {
            logger.error(&format!("failed to parse request from {}: {}", peer, e));
            let resp = Response::internal_error(&cfg.server_name);
            let _ = resp.send(stream).await;
            return;
        }
    };

    let proxy_snapshot = proxy_table.load();
    if let Some(route) = proxy_snapshot.match_route(&request.path) {
        match crate::proxy::relay(&request, route, stream, &peer).await {
            Ok(status) => {
                logger.access(&request.method, &request.path, status, &peer);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {
                logger.error(&format!(
                    "no healthy upstream for {} {}",
                    request.method, request.path
                ));
                let resp = Response::service_unavailable(&cfg.server_name);
                let _ = resp.send(stream).await;
                logger.access(&request.method, &request.path, 503, &peer);
            }
            Err(e) => {
                logger.error(&format!(
                    "proxy upstream error for {} {} -> {}",
                    request.method, request.path, e
                ));
                let resp = Response::bad_gateway(&cfg.server_name);
                let _ = resp.send(stream).await;
                logger.access(&request.method, &request.path, 502, &peer);
            }
        }
        return;
    }
    drop(proxy_snapshot);

    let wasm_snapshot = wasm_table.load();
    if let Some(route) = wasm_snapshot.match_route(&request.path) {
        match crate::wasm::invoke(route, &request.method, &request.path) {
            Ok((status, body)) => {
                let mut resp = Response::new(status, status_text(status), &cfg.server_name);
                resp.set_body(body, "text/plain; charset=utf-8");
                logger.access(&request.method, &request.path, status, &peer);
                let _ = resp.send(stream).await;
            }
            Err(e) => {
                logger.error(&format!(
                    "WASM handler error for {} {} -> {}",
                    request.method, request.path, e
                ));
                let resp = Response::internal_error(&cfg.server_name);
                let _ = resp.send(stream).await;
                logger.access(&request.method, &request.path, 500, &peer);
            }
        }
        return;
    }
    drop(wasm_snapshot);

    let response = router::route(&request, &cfg).await;
    logger.access(&request.method, &request.path, response.status_code, &peer);

    if let Err(e) = response.send(stream).await {
        logger.error(&format!("failed to write response to {}: {}", peer, e));
    }
}

/// Minimal status-code-to-reason-phrase mapping for WASM handler
/// responses, which only supply a numeric status code (see
/// wasm::invoke's ABI docs).
fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    }
}
