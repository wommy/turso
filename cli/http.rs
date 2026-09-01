use std::io::Read;
use std::net::TcpStream;

use anyhow::{anyhow, Result};

const MAX_HEADER_BYTES: usize = 32 * 1024;

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
                let n = stream.read(&mut buffer)?;
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
        204 => "No Content",
        404 => "Not Found",
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
}
