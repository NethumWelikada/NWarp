use crate::config::Config;
use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::router;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

/// Handles a single client connection end-to-end over a plain TCP
/// socket: parse -> route (proxy or static) -> respond. Runs as its
/// own Tokio task rather than its own OS thread (Phase 4) - many
/// thousands of these can be in flight at once on a small pool of
/// worker threads, since each one yields at every `.await` instead of
/// blocking an entire thread while waiting on I/O.
pub async fn handle(
    mut stream: TcpStream,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ProxyTable>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    handle_generic(&mut stream, peer, cfg, logger, proxy_table).await;
}

/// Shared request/response cycle, generic over any stream that can be
/// read from and written to asynchronously. This is what lets the
/// exact same routing and logging logic serve both plain HTTP
/// (Tokio TcpStream) and HTTPS (tokio-rustls TlsStream) without
/// duplicating code - see server/tls.rs.
///
/// If the request path matches a configured proxy route, it is
/// forwarded to an upstream (Phase 3, with health-checked upstream
/// selection from Phase 3.5). Otherwise it falls through to static
/// file serving (Phase 1), unchanged.
pub async fn handle_generic<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    peer: String,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ProxyTable>,
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

    if let Some(route) = proxy_table.match_route(&request.path) {
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

    let response = router::route(&request, &cfg).await;
    logger.access(&request.method, &request.path, response.status_code, &peer);

    if let Err(e) = response.send(stream).await {
        logger.error(&format!("failed to write response to {}: {}", peer, e));
    }
}
