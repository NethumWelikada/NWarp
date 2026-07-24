use tokio::io::{AsyncWrite, AsyncWriteExt};

pub struct Response {
    pub status_code: u16,
    pub status_text: &'static str,
    pub content_type: String,
    pub body: Vec<u8>,
    pub server_name: String,
}

impl Response {
    pub fn new(status_code: u16, status_text: &'static str, server_name: &str) -> Self {
        Response {
            status_code,
            status_text,
            content_type: "text/plain".to_string(),
            body: Vec::new(),
            server_name: server_name.to_string(),
        }
    }

    pub fn ok(server_name: &str) -> Self {
        Response::new(200, "OK", server_name)
    }

    pub fn not_found(server_name: &str) -> Self {
        let mut r = Response::new(404, "Not Found", server_name);
        r.set_html_body(&page(404, "Not Found", server_name));
        r
    }

    pub fn forbidden(server_name: &str) -> Self {
        let mut r = Response::new(403, "Forbidden", server_name);
        r.set_html_body(&page(403, "Forbidden", server_name));
        r
    }

    pub fn internal_error(server_name: &str) -> Self {
        let mut r = Response::new(500, "Internal Server Error", server_name);
        r.set_html_body(&page(500, "Internal Server Error", server_name));
        r
    }

    pub fn bad_gateway(server_name: &str) -> Self {
        let mut r = Response::new(502, "Bad Gateway", server_name);
        r.set_html_body(&page(502, "Bad Gateway", server_name));
        r
    }

    pub fn service_unavailable(server_name: &str) -> Self {
        let mut r = Response::new(503, "Service Unavailable", server_name);
        r.set_html_body(&page(503, "Service Unavailable", server_name));
        r
    }

    pub fn set_html_body(&mut self, html: &str) {
        self.content_type = "text/html; charset=utf-8".to_string();
        self.body = html.as_bytes().to_vec();
    }

    pub fn set_body(&mut self, bytes: Vec<u8>, content_type: &str) {
        self.content_type = content_type.to_string();
        self.body = bytes;
    }

    /// Serializes and writes the response (headers + body) to any
    /// async Write stream (a Tokio TcpStream, or a TLS-wrapped stream).
    pub async fn send<W: AsyncWrite + Unpin>(&self, stream: &mut W) -> std::io::Result<()> {
        let headers = format!(
            "HTTP/1.1 {} {}\r\n\
             Server: {}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n",
            self.status_code,
            self.status_text,
            self.server_name,
            self.content_type,
            self.body.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(&self.body).await?;
        stream.flush().await
    }
}

fn page(code: u16, text: &str, server_name: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html>\n<head><title>{code} {text}</title></head>\n\
         <body style=\"font-family:'Geist',ui-sans-serif,-apple-system,'Segoe UI',sans-serif;\
         background:#F7F8F9;color:#1A1A1A;text-align:center;margin:0;padding-top:12%;\">\n\
         <h1 style=\"font-size:2.2rem;\">{code} <span style=\"color:#0066FF;\">{text}</span></h1>\n\
         <hr style=\"border-color:#FF7A1A;width:120px;border-width:2px 0 0;\">\n\
         <p style=\"color:#6B7280;\">{server_name}</p>\n\
         <p style=\"color:#9CA3AF;font-size:0.85em;\"><a href=\"https://github.com/NethumWelikada\" style=\"color:#9CA3AF;\">Nethum Welikada</a> &middot; Master of Engineering in Internetworking<br>Dalhousie University &middot; Halifax, Nova Scotia, Canada</p>\n\
         </body>\n</html>\n"
    )
}

