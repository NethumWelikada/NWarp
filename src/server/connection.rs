use crate::config::Config;
use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::router;
use crate::logging::Logger;
use crate::proxy::ProxyTable;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

/// Handles a single client connection end-to-end over a plain TCP socket:
/// parse -> route (proxy or static) -> respond. Runs on its own pooled
/// thread.
pub fn handle(
    mut stream: TcpStream,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ProxyTable>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    handle_generic(&mut stream, peer, cfg, logger, proxy_table);
}

/// Shared request/response cycle, generic over any stream that can be
/// read from and written to. This is what lets the exact same routing
/// and logging logic serve both plain HTTP (TcpStream) and HTTPS
/// (rustls::Stream) without duplicating code - see server/tls.rs.
///
/// If the request path matches a configured proxy route, it is
/// forwarded to an upstream (Phase 3). Otherwise it falls through to
/// static file serving (Phase 1), unchanged.
pub fn handle_generic<S: Read + Write>(
    stream: &mut S,
    peer: String,
    cfg: Arc<Config>,
    logger: Arc<Logger>,
    proxy_table: Arc<ProxyTable>,
) {
    let request = match Request::parse(stream) {
        Ok(r) => r,
        Err(e) => {
            logger.error(&format!("failed to parse request from {}: {}", peer, e));
            let resp = Response::internal_error(&cfg.server_name);
            let _ = resp.send(stream);
            return;
        }
    };

    if let Some(route) = proxy_table.match_route(&request.path) {
        match crate::proxy::relay(&request, route, stream, &peer) {
            Ok(status) => {
                logger.access(&request.method, &request.path, status, &peer);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {
                logger.error(&format!(
                    "no healthy upstream for {} {}",
                    request.method, request.path
                ));
                let resp = Response::service_unavailable(&cfg.server_name);
                let _ = resp.send(stream);
                logger.access(&request.method, &request.path, 503, &peer);
            }
            Err(e) => {
                logger.error(&format!(
                    "proxy upstream error for {} {} -> {}",
                    request.method, request.path, e
                ));
                let resp = Response::bad_gateway(&cfg.server_name);
                let _ = resp.send(stream);
                logger.access(&request.method, &request.path, 502, &peer);
            }
        }
        return;
    }

    let response = router::route(&request, &cfg);
    logger.access(&request.method, &request.path, response.status_code, &peer);

    if let Err(e) = response.send(stream) {
        logger.error(&format!("failed to write response to {}: {}", peer, e));
    }
}
