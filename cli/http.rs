use std::io::Read;
use std::net::TcpStream;

use anyhow::{anyhow, Result};

const MAX_HEADER_BYTES: usize = 32 * 1024;

pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub extra_headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.to_string(),
            extra_headers: Vec::new(),
            body,
        }
    }

    pub fn text(status: u16, body: &str) -> Self {
        Self::new(status, "text/plain", body.as_bytes().to_vec())
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }
}

pub fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = [0u8; 8192];
    let mut request_data = Vec::new();

    loop {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        // Bytes before this offset hold no terminator, and one can still
        // straddle the last three of them.
        let unscanned = request_data.len().saturating_sub(3);
        request_data.extend_from_slice(&buffer[..n]);

        let Some(header_end) = find_header_end(&request_data, unscanned) else {
            if request_data.len() > MAX_HEADER_BYTES {
                return Err(anyhow!(
                    "HTTP request headers exceed {MAX_HEADER_BYTES} bytes"
                ));
            }
            continue;
        };
        let headers = String::from_utf8_lossy(&request_data[..header_end]);
        if let Some(content_length) = parse_content_length(&headers) {
            let total_expected = request_end(header_end, content_length)?;
            while request_data.len() < total_expected {
                let n = stream.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                request_data.extend_from_slice(&buffer[..n]);
            }
        }
        break;
    }

    parse_http_request(&request_data)
}

pub fn parse_http_request(data: &[u8]) -> Result<HttpRequest> {
    let header_end = find_header_end(data, 0).ok_or_else(|| anyhow!("Invalid HTTP request"))?;
    let head = String::from_utf8_lossy(&data[..header_end]);

    let mut lines = head.lines();
    let first_line = lines.next().ok_or_else(|| anyhow!("Empty request"))?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();

    if parts.len() < 2 {
        return Err(anyhow!("Invalid request line"));
    }

    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();

    Ok(HttpRequest {
        method: parts[0].to_string(),
        path: parts[1].to_string(),
        headers,
        body: data[header_end + 4..].to_vec(),
    })
}

fn find_header_end(data: &[u8], start: usize) -> Option<usize> {
    (start..data.len().saturating_sub(3)).find(|&i| &data[i..i + 4] == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            let value = line.split(':').nth(1)?.trim();
            return value.parse().ok();
        }
    }
    None
}

fn request_end(header_end: usize, content_length: usize) -> Result<usize> {
    (header_end + 4)
        .checked_add(content_length)
        .ok_or_else(|| anyhow!("HTTP request length overflows: {content_length}"))
}

pub fn format_http_response(resp: &HttpResponse) -> Vec<u8> {
    let status_text = status_text(resp.status);

    let mut header = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n",
        resp.status,
        status_text,
        resp.content_type,
        resp.body.len()
    );
    for (name, value) in &resp.extra_headers {
        header.push_str(&format!("{name}: {value}\r\n"));
    }
    header.push_str("\r\n");

    let mut result = header.into_bytes();
    result.extend_from_slice(&resp.body);
    result
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

pub fn cors_headers() -> Vec<(String, String)> {
    [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
        ("Access-Control-Allow-Headers", "*"),
        ("Access-Control-Expose-Headers", "*"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the read loop: the terminator must be found whatever the chunk
    /// boundaries, including when it straddles two reads.
    #[test]
    fn finds_header_end_across_read_boundaries() {
        let request = b"POST / HTTP/1.1\r\nHost: x\r\n\r\nbody".to_vec();
        let expected = find_header_end(&request, 0).expect("terminator is present");

        for chunk in 1..=request.len() {
            let mut data = Vec::new();
            let mut found = None;
            for piece in request.chunks(chunk) {
                let unscanned = data.len().saturating_sub(3);
                data.extend_from_slice(piece);
                if let Some(end) = find_header_end(&data, unscanned) {
                    found = Some(end);
                    break;
                }
            }
            assert_eq!(found, Some(expected), "missed terminator at chunk {chunk}");
        }
    }

    #[test]
    fn rejects_content_length_that_overflows() {
        assert!(request_end(0, usize::MAX).is_err());
        assert_eq!(request_end(10, 5).unwrap(), 19);
    }

    #[test]
    fn parses_headers_case_insensitively() {
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nMCP-Protocol-Version: 2026-07-28\r\n\r\n{}";
        let request = parse_http_request(raw).expect("valid request");

        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/mcp");
        assert_eq!(request.header("mcp-protocol-version"), Some("2026-07-28"));
        assert_eq!(request.header("Missing"), None);
        assert_eq!(request.body, b"{}");
    }
}
