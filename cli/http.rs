use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bytes::Bytes;

const MAX_HEADER_BYTES: usize = 32 * 1024;

/// A single `read()` is capped well below `max_duration`, so the deadline
/// check in the loops below is reached again shortly after the budget runs
/// out, instead of the loop being stuck inside one read for the whole budget
/// while a client trickles in just enough bytes to keep it from failing.
const READ_POLL_TIMEOUT: Duration = Duration::from_millis(500);

/// What a caller is willing to read from a client it does not trust.
pub struct ReadLimits {
    pub max_body_bytes: usize,
    /// How long one request may take to arrive, however slowly it is fed.
    pub max_duration: Duration,
}

impl ReadLimits {
    /// Reads a body of any size, but still on a clock. A timed-out read is
    /// retried until `max_duration` runs out, so that deadline is the only
    /// thing that ends the read: without a real one, a client that connects
    /// and then says nothing holds the reader forever.
    pub fn any_size(max_duration: Duration) -> Self {
        Self {
            max_body_bytes: usize::MAX,
            max_duration,
        }
    }
}

pub enum RequestError {
    /// The client sent something we will not read. There is a live connection
    /// to answer on, so a caller can turn this into a 400.
    Refused(String),
    /// The connection itself failed; there is nobody left to answer.
    Io(anyhow::Error),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => write!(f, "{reason}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
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
        self.extra_headers.extend(headers);
        self
    }
}

pub fn read_http_request(
    stream: &mut TcpStream,
    limits: &ReadLimits,
) -> Result<HttpRequest, RequestError> {
    stream
        .set_read_timeout(Some(READ_POLL_TIMEOUT.min(limits.max_duration)))
        .map_err(io_failed)?;

    let started = Instant::now();
    let mut buffer = [0u8; 8192];
    let mut request_data = Vec::new();

    loop {
        if started.elapsed() > limits.max_duration {
            return Err(RequestError::Refused(
                "HTTP request took too long to arrive".to_string(),
            ));
        }

        let n = match stream.read(&mut buffer) {
            Ok(n) => n,
            Err(e) if is_read_timeout(&e) => continue,
            Err(e) => return Err(io_failed(e)),
        };
        if n == 0 {
            break;
        }
        // Bytes before this offset hold no terminator, and one can still
        // straddle the last three of them.
        let unscanned = request_data.len().saturating_sub(3);
        request_data.extend_from_slice(&buffer[..n]);

        let Some(header_end) = find_header_end(&request_data, unscanned) else {
            if request_data.len() > MAX_HEADER_BYTES {
                return Err(RequestError::Refused(format!(
                    "HTTP request headers exceed {MAX_HEADER_BYTES} bytes"
                )));
            }
            continue;
        };
        let headers = String::from_utf8_lossy(&request_data[..header_end]);
        // A chunked body has no Content-Length, so reading it as one would
        // silently treat the first chunk header as the body.
        if header_names(&headers).any(|name| name == "transfer-encoding") {
            return Err(RequestError::Refused(
                "Chunked request bodies are not supported; send Content-Length".to_string(),
            ));
        }
        if let Some(content_length) = parse_content_length(&headers) {
            if content_length > limits.max_body_bytes {
                return Err(RequestError::Refused(format!(
                    "Request body of {content_length} bytes exceeds the {} byte limit",
                    limits.max_body_bytes
                )));
            }
            let total_expected =
                request_end(header_end, content_length).map_err(RequestError::Io)?;
            while request_data.len() < total_expected {
                if started.elapsed() > limits.max_duration {
                    return Err(RequestError::Refused(
                        "HTTP request took too long to arrive".to_string(),
                    ));
                }
                let n = match stream.read(&mut buffer) {
                    Ok(n) => n,
                    Err(e) if is_read_timeout(&e) => continue,
                    Err(e) => return Err(io_failed(e)),
                };
                if n == 0 {
                    break;
                }
                request_data.extend_from_slice(&buffer[..n]);
            }
        }
        break;
    }

    parse_http_request(request_data).map_err(RequestError::Io)
}

fn io_failed(e: std::io::Error) -> RequestError {
    RequestError::Io(anyhow!(e))
}

/// A read timing out is not a failure: it just means the poll interval
/// elapsed with no data, so the loop above can re-check its real deadline.
fn is_read_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn header_names(headers: &str) -> impl Iterator<Item = String> + '_ {
    headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim().to_lowercase())
}

/// Takes `data` by value so the body can be sliced out below without a
/// second copy of up to `max_body_bytes` alongside the buffer it came from.
pub fn parse_http_request(data: Vec<u8>) -> Result<HttpRequest> {
    let header_end = find_header_end(&data, 0).ok_or_else(|| anyhow!("Invalid HTTP request"))?;
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
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // `Bytes` shares the same allocation `data` already holds; slicing it
    // just adjusts a pointer and a length, it does not copy the body.
    let body = Bytes::from(data).slice(header_end + 4..);

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
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

/// A client controls Content-Length, so the end of the body has to be computed
/// without trusting it to fit.
fn request_end(header_end: usize, content_length: usize) -> Result<usize> {
    (header_end + 4)
        .checked_add(content_length)
        .ok_or_else(|| anyhow!("HTTP request length overflows: {content_length}"))
}

pub fn format_http_response(resp: &HttpResponse) -> Vec<u8> {
    let status_text = status_text(resp.status);

    let mut header = format!("HTTP/1.1 {} {}\r\n", resp.status, status_text);
    // A body-less content type would claim a media type for content that
    // does not exist.
    if !resp.content_type.is_empty() {
        header.push_str(&format!("Content-Type: {}\r\n", resp.content_type));
    }
    // RFC 9110 section 15.3.5: a 204 response must not carry Content-Length,
    // even as "0" - there is no body to measure, by definition of the status.
    if resp.status != 204 {
        header.push_str(&format!("Content-Length: {}\r\n", resp.body.len()));
    }
    header.push_str("Connection: close\r\n");
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
        503 => "Service Unavailable",
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

    fn read_from_client(raw: &[u8], limits: &ReadLimits) -> Result<HttpRequest, RequestError> {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let raw = raw.to_vec();
        std::thread::spawn(move || {
            let mut client = TcpStream::connect(address).expect("connect");
            let _ = client.write_all(&raw);
            let _ = client.flush();
            std::thread::sleep(Duration::from_millis(200));
        });

        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        read_http_request(&mut stream, limits)
    }

    #[test]
    fn a_silent_client_is_refused_gracefully_once_the_deadline_passes() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            let client = TcpStream::connect(address).expect("connect");
            // Never sends a byte, and stays connected well past the deadline
            // below, instead of closing right away.
            std::thread::sleep(Duration::from_secs(2));
            drop(client);
        });

        let (mut stream, _) = listener.accept().expect("accept");
        let limits = ReadLimits {
            max_body_bytes: 1024,
            max_duration: Duration::from_millis(300),
        };
        // Mirrors what a caller used to set up by hand: a client that sends
        // nothing must be refused with a clear reason once the read poll
        // times out, not treated as a broken connection.
        stream
            .set_read_timeout(Some(limits.max_duration))
            .expect("timeout");

        let error = read_http_request(&mut stream, &limits)
            .err()
            .expect("a silent client is refused");

        assert!(
            matches!(&error, RequestError::Refused(reason) if reason.contains("took too long")),
            "expected a graceful refusal, got: {error}"
        );
    }

    #[test]
    fn a_reader_that_takes_any_body_size_still_gives_up_on_a_silent_client() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            let client = TcpStream::connect(address).expect("connect");
            std::thread::sleep(Duration::from_secs(2));
            drop(client);
        });

        let (mut stream, _) = listener.accept().expect("accept");
        // The sync server reads bodies of any size. That must not also mean
        // any amount of time: it serves one connection at a time, so a reader
        // that never returns takes the whole server down with it.
        let limits = ReadLimits::any_size(Duration::from_millis(300));
        match read_http_request(&mut stream, &limits) {
            Err(RequestError::Refused(reason)) => assert!(
                reason.contains("too long"),
                "expected a timeout refusal, got: {reason}"
            ),
            Err(RequestError::Io(e)) => panic!("expected a refusal, got an I/O error: {e}"),
            Ok(_) => panic!("expected a refusal, got a request"),
        }
    }

    #[test]
    fn a_client_that_stalls_mid_body_is_refused_gracefully_too() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            let mut client = TcpStream::connect(address).expect("connect");
            // Headers announce a 100-byte body but only 5 bytes ever arrive.
            let _ = client
                .write_all(b"POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\nshort");
            let _ = client.flush();
            std::thread::sleep(Duration::from_secs(2));
            drop(client);
        });

        let (mut stream, _) = listener.accept().expect("accept");
        let limits = ReadLimits {
            max_body_bytes: 1024,
            max_duration: Duration::from_millis(300),
        };
        stream
            .set_read_timeout(Some(limits.max_duration))
            .expect("timeout");

        let error = read_http_request(&mut stream, &limits)
            .err()
            .expect("a stalled body is refused");

        assert!(
            matches!(&error, RequestError::Refused(reason) if reason.contains("took too long")),
            "expected a graceful refusal, got: {error}"
        );
    }

    #[test]
    fn a_body_over_the_limit_is_refused_before_it_is_read() {
        let limits = ReadLimits {
            max_body_bytes: 1024,
            max_duration: Duration::from_secs(5),
        };
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: 9999999999\r\n\r\n";

        let error = read_from_client(raw, &limits).err().expect("refused");

        assert!(
            matches!(&error, RequestError::Refused(reason) if reason.contains("exceeds")),
            "expected a refusal, got: {error}"
        );
    }

    #[test]
    fn a_chunked_body_is_refused_rather_than_misread() {
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";

        let error = read_from_client(raw, &ReadLimits::any_size(Duration::from_secs(5)))
            .err()
            .expect("refused");

        assert!(
            matches!(&error, RequestError::Refused(reason) if reason.contains("Chunked")),
            "expected a refusal, got: {error}"
        );
    }

    #[test]
    fn the_sync_server_keeps_its_cors_headers() {
        let response = HttpResponse::text(200, "ok").with_headers(cors_headers());

        let rendered = String::from_utf8(format_http_response(&response)).expect("utf-8");

        assert!(
            rendered.contains("Access-Control-Allow-Origin: *"),
            "{rendered}"
        );
        assert!(rendered.contains("Access-Control-Allow-Methods: GET, POST, OPTIONS"));
    }

    #[test]
    fn parses_headers_case_insensitively() {
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nMCP-Protocol-Version: 2026-07-28\r\n\r\n{}";
        let request = parse_http_request(raw.to_vec()).expect("valid request");

        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/mcp");
        assert_eq!(request.header("mcp-protocol-version"), Some("2026-07-28"));
        assert_eq!(request.header("Missing"), None);
        assert_eq!(&request.body[..], b"{}");
    }

    #[test]
    fn a_204_response_carries_no_content_length() {
        let response = HttpResponse::new(204, "text/plain", Vec::new());

        let rendered = String::from_utf8(format_http_response(&response)).expect("utf-8");

        assert!(
            !rendered.contains("Content-Length"),
            "a 204 must not claim a body length: {rendered}"
        );
    }

    #[test]
    fn an_empty_content_type_is_left_out_of_the_response() {
        let response = HttpResponse::new(202, "", Vec::new());

        let rendered = String::from_utf8(format_http_response(&response)).expect("utf-8");

        assert!(
            !rendered.contains("Content-Type"),
            "a body-less response must not claim a media type: {rendered}"
        );
    }
}
