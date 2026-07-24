use crate::config::Config;
use crate::http::request::Request;
use crate::http::response::Response;
use std::fs;
use std::path::{Path, PathBuf};

/// Resolves a request to a static file under document_root and builds
/// the response. This is Phase 1 behavior (static files only) - the
/// reverse-proxy and module system land in later phases.
pub fn route(req: &Request, cfg: &Config) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        let mut r = Response::new(405, "Method Not Allowed", &cfg.server_name);
        r.set_html_body("<h1>405 Method Not Allowed</h1>");
        return r;
    }

    let mut rel_path = req.path.trim_start_matches('/').to_string();
    if rel_path.is_empty() {
        rel_path = cfg.index_file.clone();
    }

    let root = PathBuf::from(&cfg.document_root);
    let mut full_path = root.join(&rel_path);

    if full_path.is_dir() {
        full_path = full_path.join(&cfg.index_file);
    }

    // Final safety check: resolved path must still live under document_root.
    if !path_is_within(&full_path, &root) {
        return Response::forbidden(&cfg.server_name);
    }

    match fs::read(&full_path) {
        Ok(bytes) => {
            let mime = mime_for(&full_path);
            let mut r = Response::ok(&cfg.server_name);
            r.set_body(bytes, mime);
            r
        }
        Err(_) => Response::not_found(&cfg.server_name),
    }
}

fn path_is_within(target: &Path, root: &Path) -> bool {
    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    match fs::canonicalize(target) {
        Ok(target_abs) => target_abs.starts_with(&root_abs),
        // File may not exist yet (404 case) - fall back to lexical check.
        Err(_) => target.starts_with(root),
    }
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
