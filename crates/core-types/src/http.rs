#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DecodeError(&'static str);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for DecodeError {}

pub fn decode_response(raw: &[u8]) -> Result<Response, DecodeError> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(DecodeError("hub 返回的 HTTP 响应头不完整"))?;
    let head = std::str::from_utf8(&raw[..head_end])
        .map_err(|_| DecodeError("hub 返回的 HTTP 响应头编码不合法"))?;
    let mut lines = head.split("\r\n");
    let mut status_line = lines.next().unwrap_or_default().split_whitespace();
    if !matches!(status_line.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(DecodeError("hub 返回的 HTTP 协议版本不合法"));
    }
    let status = status_line
        .next()
        .filter(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|s| (100..=599).contains(s))
        .ok_or(DecodeError("hub 返回的 HTTP 状态码不合法"))?;
    let mut encodings = Vec::new();
    let mut length = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or(DecodeError("hub 返回的 HTTP 响应头不合法"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            encodings.extend(value.split(',').map(str::trim));
        } else if name.eq_ignore_ascii_case("content-length") {
            let parsed = (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
                .then(|| value.parse::<usize>().ok())
                .flatten()
                .ok_or(DecodeError("hub 返回的 Content-Length 不合法"))?;
            if length.is_some_and(|n| n != parsed) {
                return Err(DecodeError("hub 返回了冲突的 Content-Length"));
            }
            length = Some(parsed);
        }
    }
    let rest = &raw[head_end + 4..];
    let body = if encodings.is_empty() {
        if length.is_some_and(|n| n != rest.len()) {
            return Err(DecodeError("hub 返回的 HTTP 正文长度不完整或不匹配"));
        }
        rest.to_vec()
    } else {
        if encodings.len() != 1 || !encodings[0].eq_ignore_ascii_case("chunked") {
            return Err(DecodeError("hub 返回了不支持的 Transfer-Encoding"));
        }
        if length.is_some() {
            return Err(DecodeError("hub 返回了冲突的 HTTP 正文长度标记"));
        }
        decode_chunks(rest)?
    };
    Ok(Response { status, body })
}

fn take_line<'a>(rest: &mut &'a [u8]) -> Result<&'a [u8], DecodeError> {
    let end = rest
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or(DecodeError("hub 返回的分块响应不完整"))?;
    let line = &rest[..end];
    *rest = &rest[end + 2..];
    Ok(line)
}

fn decode_chunks(mut rest: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::new();
    loop {
        let line = take_line(&mut rest)?;
        let size = line.split(|b| *b == b';').next().unwrap_or_default();
        if size.is_empty() || !size.iter().all(u8::is_ascii_hexdigit) {
            return Err(DecodeError("hub 返回的分块长度不合法"));
        }
        let size = std::str::from_utf8(size)
            .ok()
            .and_then(|s| usize::from_str_radix(s, 16).ok())
            .ok_or(DecodeError("hub 返回的分块长度超出范围"))?;
        if size == 0 {
            loop {
                let trailer = take_line(&mut rest)?;
                if trailer.is_empty() {
                    return if rest.is_empty() {
                        Ok(out)
                    } else {
                        Err(DecodeError("hub 返回的分块响应结束后仍有数据"))
                    };
                }
                if !trailer.contains(&b':') {
                    return Err(DecodeError("hub 返回的分块尾部不合法"));
                }
            }
        }
        let data = rest
            .get(..size)
            .ok_or(DecodeError("hub 返回的分块正文被截断"))?;
        out.extend_from_slice(data);
        rest = rest[size..]
            .strip_prefix(b"\r\n")
            .ok_or(DecodeError("hub 返回的分块正文缺少结束标记"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_can_split_utf8_and_keep_body_delimiters() {
        let response = decode_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\n\r\n1;ext=value\r\n\xe4\r\n2\r\n\xb8\xad\r\n2\r\n\r\n\r\n0\r\nX-End: true\r\n\r\n").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "中\r\n".as_bytes());
    }

    #[test]
    fn malformed_chunks_never_return_a_partial_success() {
        for body in [
            "",
            "5\r\nhi",
            "1\r\na",
            "1\r\na!\r\n0\r\n\r\n",
            "0\r\n",
            "z\r\n",
            "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF\r\n",
            "0\r\ninvalid\r\n\r\n",
            "0\r\n\r\nextra",
        ] {
            let raw = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{body}");
            assert!(decode_response(raw.as_bytes()).is_err(), "{body:?}");
        }
    }

    #[test]
    fn content_length_counts_bytes_and_rejects_truncation() {
        assert_eq!(
            decode_response("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n中".as_bytes())
                .unwrap()
                .body,
            "中".as_bytes(),
        );
        for raw in [
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nhi",
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n0\r\n\r\n",
            "HTTP/1.1 nope\r\n\r\n",
        ] {
            assert!(decode_response(raw.as_bytes()).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn close_delimited_and_empty_responses_are_supported() {
        assert_eq!(
            decode_response(b"HTTP/1.0 200 OK\r\n\r\nhello")
                .unwrap()
                .body,
            b"hello"
        );
        assert_eq!(
            decode_response(b"HTTP/1.1 204 No Content\r\n\r\n")
                .unwrap()
                .body,
            b""
        );
    }
}
