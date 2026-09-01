use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

const MAX_HEADER_BYTES: usize = 32 * 1024;

/// How long one request may take to arrive, however slowly a client feeds
/// it in. A flat `set_read_timeout` on its own is not enough: it resets on
/// every byte received, so a client trickling in one byte just before each
/// timeout elapses can hold the connection open indefinitely. This is the
/// real budget; `read_http_request` re-derives its own socket timeout from
/// it instead of trusting whatever the caller set.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// How long a single `read()` may block before the loop below rechecks its
/// deadline. Capping it well below the deadline itself means that check is
/// reached again shortly after the budget runs out, instead of the loop
/// sitting inside one blocking read for the whole remaining budget while a
/// client trickles in just enough bytes to keep it from failing.
const IO_POLL_TIMEOUT: Duration = Duration::from_millis(500);

pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// Headers beyond the ones every response carries (`Content-Type`,
    /// `Content-Length`, `Connection`). CORS is why this exists: a wildcard
    /// `Access-Control-Allow-Origin` is not safe for every server this code
    /// serves, so each one now opts in to what it sends explicitly.
    pub extra_headers: Vec<(String, String)>,
}

pub fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    read_http_request_by(stream, READ_DEADLINE)
}

/// Split out from `read_http_request` so a test can hand it a deadline of a
/// few hundred milliseconds instead of waiting out the real 30 seconds to
/// prove a silent client is refused.
fn read_http_request_by(stream: &mut TcpStream, deadline: Duration) -> Result<Vec<u8>> {
    stream.set_read_timeout(Some(IO_POLL_TIMEOUT.min(deadline)))?;
    let started = Instant::now();
    let mut buffer = [0u8; 8192];
    let mut request_data = Vec::new();

    loop {
        if started.elapsed() > deadline {
            return Err(anyhow!("HTTP request took too long to arrive"));
        }
        let n = match stream.read(&mut buffer) {
            Ok(n) => n,
            Err(e) if is_io_timeout(&e) => continue,
            Err(e) => return Err(e.into()),
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
                return Err(anyhow!(
                    "HTTP request headers exceed {MAX_HEADER_BYTES} bytes"
                ));
            }
            continue;
        };
        // The terminator can arrive in the very read that also pushes the
        // buffer past the cap, which the check above never sees because it
        // only runs while the terminator is still missing.
        if header_end > MAX_HEADER_BYTES {
            return Err(anyhow!(
                "HTTP request headers exceed {MAX_HEADER_BYTES} bytes"
            ));
        }
        let headers = String::from_utf8_lossy(&request_data[..header_end]);
        if let Some(content_length) = parse_content_length(&headers)? {
            let total_expected = request_end(header_end, content_length)?;
            while request_data.len() < total_expected {
                if started.elapsed() > deadline {
                    return Err(anyhow!("HTTP request took too long to arrive"));
                }
                let n = match stream.read(&mut buffer) {
                    Ok(n) => n,
                    Err(e) if is_io_timeout(&e) => continue,
                    Err(e) => return Err(e.into()),
                };
                if n == 0 {
                    break;
                }
                request_data.extend_from_slice(&buffer[..n]);
            }
            // A single read can return more than this request's body - the
            // start of the next one, say - and everything after the headers
            // is handed on as the body, so bound it to the declared length.
            request_data.truncate(total_expected);
        }
        break;
    }

    Ok(request_data)
}

/// A read timing out is not a failure on its own: it just means the poll
/// interval elapsed with no data, so the loop above can recheck its real
/// deadline instead of treating the timeout as the end of the world.
fn is_io_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn find_header_end(data: &[u8], start: usize) -> Option<usize> {
    (start..data.len().saturating_sub(3)).find(|&i| &data[i..i + 4] == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> Result<Option<usize>> {
    let mut found: Option<usize> = None;
    for line in headers.lines() {
        if !line.to_lowercase().starts_with("content-length:") {
            continue;
        }
        let value = line
            .split_once(':')
            .map(|(_, value)| value.trim())
            .unwrap_or_default();
        let length: usize = value
            .parse()
            .map_err(|_| anyhow!("Invalid Content-Length: {value}"))?;
        // RFC 9110 6.4.1: repeated values that disagree leave the message
        // length ambiguous, and an ambiguous length is unrecoverable - the
        // next request's bytes would be read as this one's body.
        if found.is_some_and(|first| first != length) {
            return Err(anyhow!("Conflicting Content-Length headers"));
        }
        found = Some(length);
    }
    Ok(found)
}

/// A client controls Content-Length, so the end of the body has to be
/// computed without trusting it to fit.
fn request_end(header_end: usize, content_length: usize) -> Result<usize> {
    (header_end + 4)
        .checked_add(content_length)
        .ok_or_else(|| anyhow!("HTTP request length overflows: {content_length}"))
}

/// Method, path, headers (in wire order, name unmangled), and body.
type ParsedHttpRequest = (String, String, Vec<(String, String)>, Vec<u8>);

pub fn parse_http_request(data: &[u8]) -> Result<ParsedHttpRequest> {
    let header_end = find_header_end(data, 0).ok_or_else(|| anyhow!("Invalid HTTP request"))?;
    let headers_text = String::from_utf8_lossy(&data[..header_end]);

    let mut lines = headers_text.lines();
    let first_line = lines.next().ok_or_else(|| anyhow!("Empty request"))?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();

    if parts.len() < 2 {
        return Err(anyhow!("Invalid request line"));
    }

    let method = parts[0].to_string();
    let path = parts[1].to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    let body = data[header_end + 4..].to_vec();

    Ok((method, path, headers, body))
}

pub fn format_http_response(resp: &HttpResponse) -> Vec<u8> {
    let status_text = match resp.status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        500 => "Internal Server Error",
        _ => "Unknown",
    };

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
        header.push_str(name);
        header.push_str(": ");
        header.push_str(value);
        header.push_str("\r\n");
    }
    header.push_str("\r\n");

    let mut result = header.into_bytes();
    result.extend_from_slice(&resp.body);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Every status the MCP transport can answer with needs a reason phrase
    /// here, and nothing else in the suite would notice a missing one: the
    /// transport's own tests all go through `http_response_for`, which
    /// returns a numeric status and never formats a response line. `400`
    /// went out as `HTTP/1.1 400 Unknown` for three slices because of that.
    #[test]
    fn every_status_the_mcp_transport_sends_has_a_reason_phrase() {
        for status in [200, 202, 400, 403, 404, 405, 411] {
            let response = HttpResponse {
                status,
                content_type: "application/json".to_string(),
                body: Vec::new(),
                extra_headers: Vec::new(),
            };
            let formatted = String::from_utf8(format_http_response(&response)).unwrap();
            let line = formatted.lines().next().unwrap();

            assert!(
                !line.contains("Unknown"),
                "status {status} has no reason phrase: {line}"
            );
        }
    }

    /// A value that is not a number leaves the message length unknown. Parsing
    /// it away as "no body" is what makes that dangerous: the body then stays
    /// in the socket and is read as the start of the next request.
    #[test]
    fn a_content_length_that_is_not_a_number_is_refused() {
        assert!(parse_content_length("Content-Length: abc").is_err());
        assert!(parse_content_length("Content-Length: ").is_err());
        assert!(parse_content_length("Content-Length: -1").is_err());
    }

    /// RFC 9110 6.4.1: repeated values that disagree are unrecoverable.
    #[test]
    fn content_length_headers_that_disagree_are_refused() {
        assert!(parse_content_length("Content-Length: 5\r\nContent-Length: 9").is_err());
    }

    /// Repeated but identical is not ambiguous, so it is allowed through.
    #[test]
    fn content_length_repeated_with_one_value_is_allowed() {
        let headers = "Content-Length: 5\r\nContent-Length: 5";
        assert_eq!(parse_content_length(headers).unwrap(), Some(5));
        assert_eq!(parse_content_length("Host: x").unwrap(), None);
    }

    fn serve_one_request(request: Vec<u8>) -> Result<Vec<u8>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");

        let client = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(address).expect("connect");
            let _ = stream.write_all(&request);
            let _ = stream.shutdown(std::net::Shutdown::Write);
        });

        let (mut stream, _) = listener.accept().expect("accept");
        let result = read_http_request(&mut stream);
        client.join().expect("client thread does not panic");
        result
    }

    /// The cap is checked only while the terminator is still missing, so a
    /// request whose terminator lands in the same read that crosses the cap
    /// used to sail past it.
    #[test]
    fn oversized_headers_are_refused_even_when_the_terminator_arrives_with_them() {
        // Just past the 32 KiB cap, not far past it: the terminator has to
        // land in the same read that first crosses the cap. Pad far enough
        // and the old check catches it on an earlier read, before the
        // terminator arrives, and the overshoot is never exercised.
        let mut request = b"POST / HTTP/1.1\r\nX-Pad: ".to_vec();
        request.extend(std::iter::repeat_n(b'a', 33 * 1024));
        request.extend_from_slice(b"\r\n\r\n");

        let error = serve_one_request(request).expect_err("headers over the cap must be refused");
        assert!(
            error.to_string().contains("exceed"),
            "unexpected error: {error}"
        );
    }

    /// Everything after the headers is handed on as the body, so a read that
    /// overshoots - the next pipelined request, say - must not be carried into
    /// this one.
    #[test]
    fn the_body_stops_at_the_declared_length() {
        let request = b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nHELLOEXTRA".to_vec();

        let data = serve_one_request(request).expect("a well-formed request is read");
        let (_, _, _, body) = parse_http_request(&data).expect("request parses");

        assert_eq!(body, b"HELLO", "body must stop at Content-Length");
    }

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

    /// Mirrors what a caller used to set up by hand: a client that connects
    /// and never sends a byte must be refused with a clear reason once the
    /// deadline passes, rather than left to hang or read as a broken
    /// connection. Uses a deadline of a few hundred milliseconds - the real
    /// `READ_DEADLINE` is 30 seconds, and this only needs to prove the
    /// budget is enforced, not spend 30 real seconds doing it.
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

        let error = read_http_request_by(&mut stream, Duration::from_millis(300))
            .expect_err("a silent client is refused");

        assert!(
            error.to_string().contains("took too long"),
            "expected a graceful refusal, got: {error}"
        );
    }

    /// The other direction of the guard above: a client that is merely slow,
    /// not silent, and finishes within its budget must still be served. Each
    /// piece arrives further apart than `IO_POLL_TIMEOUT`, so the read loop
    /// has to resume across more than one timed-out `read()` - but the whole
    /// exchange still finishes well inside the deadline given here. Without
    /// this, a deadline that fires on any pause at all, rather than one that
    /// outlasts the budget, would pass every refusal test while refusing
    /// every real client too.
    #[test]
    fn a_client_that_trickles_bytes_within_the_deadline_is_still_served() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let request = b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nHELLO".to_vec();

        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).expect("connect");
            for chunk in request.chunks(request.len() / 3 + 1) {
                stream.write_all(chunk).expect("write");
                stream.flush().expect("flush");
                std::thread::sleep(Duration::from_millis(700));
            }
        });

        let (mut stream, _) = listener.accept().expect("accept");
        let data = read_http_request_by(&mut stream, Duration::from_secs(5))
            .expect("a slow but complete client must still be served");

        let (_, _, _, body) = parse_http_request(&data).expect("request parses");
        assert_eq!(body, b"HELLO");
    }
}
