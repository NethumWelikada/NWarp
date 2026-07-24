use crate::config::Config;
use crate::http::request::Request;
use crate::http::response::Response;
use crate::http::router;
use crate::logging::Logger;
use std::net::TcpStream;
use std::sync::Arc;

/// Handles a single client connection end-to-end: parse -> route -> respond.
/// Runs on its own OS thread (see server::listener for the thread-per-connection
/// model used in Phase 1).
pub fn handle(mut stream: TcpStream, cfg: Arc<Config>, logger: Arc<Logger>) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let request = match Request::parse(&stream) {
        Ok(r) => r,
        Err(e) => {
            logger.error(&format!("failed to parse request from {}: {}", peer, e));
            let resp = Response::internal_error(&cfg.server_name);
            let _ = resp.send(&mut stream);
            return;
        }
    };

    let response = router::route(&request, &cfg);
    logger.access(&request.method, &request.path, response.status_code, &peer);

    if let Err(e) = response.send(&mut stream) {
        logger.error(&format!("failed to write response to {}: {}", peer, e));
    }
}
