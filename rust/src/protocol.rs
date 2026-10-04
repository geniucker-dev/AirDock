// SPDX-License-Identifier: MPL-2.0
use anyhow::{Result, bail};
use plist::{Dictionary, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
};

pub const MAX_BODY: usize = 16 * 1024 * 1024;
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub uri: String,
    pub version: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}
impl Request {
    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name).map(String::as_str).unwrap_or("")
    }
    pub fn path(&self) -> &str {
        let uri = if let Some((_, tail)) = self.uri.split_once("://") {
            tail.find('/').map(|i| &tail[i..]).unwrap_or("/")
        } else {
            &self.uri
        };
        uri.split('?').next().unwrap_or("/")
    }
    pub fn query(&self, name: &str) -> Option<&str> {
        self.uri.split_once('?')?.1.split('&').find_map(|q| {
            let (k, v) = q.split_once('=')?;
            (k == name).then_some(v)
        })
    }
}

#[derive(Default)]
pub struct Reader {
    bytes: Vec<u8>,
}
impl Reader {
    pub fn next(&mut self, stream: &mut TcpStream) -> Result<Option<Request>> {
        loop {
            if let Some(end) = self.bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                if end > 65536 {
                    bail!("Request header too large")
                }
                let text = std::str::from_utf8(&self.bytes[..end])?;
                let mut lines = text.split("\r\n");
                let mut first = lines.next().unwrap_or("").split_whitespace();
                let mut method = first.next().unwrap_or("").to_owned();
                let mut uri = first.next().unwrap_or("").to_owned();
                let mut version = first.next().unwrap_or("").to_owned();
                if method.starts_with("HTTP/") && uri.parse::<u16>().is_ok() {
                    version = method;
                    method = "REVERSE_RESPONSE".into();
                    uri = "/".into();
                } else if first.next().is_some()
                    || method.is_empty()
                    || !["RTSP/1.0", "HTTP/1.1", "HTTP/1.0"].contains(&version.as_str())
                {
                    bail!("Invalid request line")
                }
                let mut headers = BTreeMap::new();
                for line in lines {
                    let (key, value) = line
                        .split_once(':')
                        .ok_or_else(|| anyhow::anyhow!("Invalid header"))?;
                    let key = key.trim().to_ascii_lowercase();
                    if key.is_empty() || headers.insert(key, value.trim().to_owned()).is_some() {
                        bail!("Duplicate header")
                    }
                }
                if headers.contains_key("transfer-encoding") {
                    bail!("Chunked requests are unsupported")
                }
                let len = headers
                    .get("content-length")
                    .map(|s| s.parse::<usize>())
                    .transpose()?
                    .unwrap_or(0);
                if len > MAX_BODY {
                    bail!("Request body too large")
                }
                if self.bytes.len() >= end + 4 + len {
                    let body = self.bytes[end + 4..end + 4 + len].to_vec();
                    self.bytes.drain(..end + 4 + len);
                    return Ok(Some(Request {
                        method,
                        uri,
                        version,
                        headers,
                        body,
                    }));
                }
            } else if self.bytes.len() > 65536 {
                bail!("Request header too large")
            }
            let mut chunk = [0; 8192];
            match stream.read(&mut chunk) {
                Ok(0) => bail!("Peer closed connection"),
                Ok(n) => self.bytes.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Response {
    pub fn ok() -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
    pub fn header(mut self, k: &str, v: impl ToString) -> Self {
        self.headers.push((k.into(), v.to_string()));
        self
    }
    pub fn bytes(mut self, content_type: &str, body: Vec<u8>) -> Self {
        self.body = body;
        self.header("Content-Type", content_type)
    }
    pub fn plist(self, value: Value) -> Result<Self> {
        Ok(self.bytes("application/x-apple-binary-plist", binary(&value)?))
    }
    pub fn write(&self, req: &Request, stream: &mut TcpStream) -> Result<()> {
        let reason = match self.status {
            200 => "OK",
            206 => "Partial Content",
            101 => "Switching Protocols",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            453 => "Not Enough Bandwidth",
            455 => "Method Not Valid in This State",
            501 => "Not Implemented",
            503 => "Service Unavailable",
            _ => "Internal Server Error",
        };
        let mut head = format!(
            "{} {} {}\r\nServer: AirTunes/220.68\r\nContent-Length: {}\r\n",
            req.version,
            self.status,
            reason,
            self.body.len()
        );
        if !req.header("cseq").is_empty() {
            head.push_str(&format!("CSeq: {}\r\n", req.header("cseq")));
        }
        for (k, v) in &self.headers {
            if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
                bail!("Invalid response header")
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        Ok(())
    }
}
pub fn binary(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    value.to_writer_binary(&mut out)?;
    Ok(out)
}
pub fn dict(entries: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
    Value::Dictionary(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub fn uint(value: u64) -> Value {
    Value::Integer(value.into())
}
pub fn integer(d: &Dictionary, k: &str) -> Option<u64> {
    d.get(k)?.as_unsigned_integer()
}
pub fn stream_connection_id(d: &Dictionary) -> Option<u64> {
    let value = d.get("streamConnectionID")?;
    // Eight-byte binary plist integers are read as i64. This opaque AirPlay ID
    // uses all 64 bits; its unsigned decimal form participates in key derivation.
    // Keep this reinterpretation separate from numeric fields such as ports.
    value
        .as_unsigned_integer()
        .or_else(|| value.as_signed_integer().map(|id| id as u64))
}
pub fn string<'a>(d: &'a Dictionary, k: &str) -> Option<&'a str> {
    d.get(k)?.as_string()
}

pub fn metadata(bytes: &[u8], depth: usize, out: &mut BTreeMap<String, String>) {
    if depth > 8 {
        return;
    }
    let mut tail = bytes;
    while tail.len() >= 8 {
        let tag = &tail[..4];
        let len = u32::from_be_bytes(tail[4..8].try_into().unwrap()) as usize;
        let Some(body) = tail.get(8..8 + len) else {
            return;
        };
        match tag {
            b"minm" | b"asar" | b"asal" => {
                if let Ok(s) = std::str::from_utf8(body) {
                    out.insert(String::from_utf8_lossy(tag).into_owned(), s.into());
                }
            }
            b"mlit" | b"mlcl" | b"mdcl" => metadata(body, depth + 1, out),
            b"astm" if body.len() == 4 => {
                out.insert(
                    "astm".into(),
                    u32::from_be_bytes(body.try_into().unwrap()).to_string(),
                );
            }
            _ => {}
        }
        tail = &tail[8 + len..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mirror_id_preserves_all_wire_bits() {
        for id in [
            0,
            123456,
            i64::MAX as u64,
            1 << 63,
            0xfedcba9876543210,
            u64::MAX,
        ] {
            // Independent minimal binary plist: a single eight-byte integer.
            let mut bytes = b"bplist00".to_vec();
            bytes.push(0x13);
            bytes.extend_from_slice(&id.to_be_bytes());
            bytes.push(8); // Object offset table.
            bytes.extend_from_slice(&[0; 6]);
            bytes.extend_from_slice(&[1, 1]); // Offset/ref widths.
            for field in [1u64, 0, 17] {
                // Object count, root index, table offset.
                bytes.extend_from_slice(&field.to_be_bytes());
            }
            let value = Value::from_reader(std::io::Cursor::new(bytes)).unwrap();
            let mut d = Dictionary::new();
            d.insert("streamConnectionID".into(), value.clone());
            assert_eq!(stream_connection_id(&d), Some(id));
            if id > i64::MAX as u64 {
                assert_eq!(integer(&d, "streamConnectionID"), None);
            }
            // Also accept a positive ID encoded in a wider plist integer.
            let positive =
                Value::from_reader(std::io::Cursor::new(binary(&uint(id)).unwrap())).unwrap();
            d.insert("streamConnectionID".into(), positive);
            assert_eq!(stream_connection_id(&d), Some(id));
            d.insert("timingPort".into(), value);
            assert_eq!(
                integer(&d, "timingPort"),
                (id <= i64::MAX as u64).then_some(id)
            );
        }
        let mut d = Dictionary::new();
        assert_eq!(stream_connection_id(&d), None);
        d.insert("streamConnectionID".into(), Value::String("123".into()));
        assert_eq!(stream_connection_id(&d), None);
    }
    #[test]
    fn absolute_uri_and_query() {
        let r = Request {
            method: "POST".into(),
            uri: "rtsp://[::1]:7000/pair-verify?x=2".into(),
            version: "RTSP/1.0".into(),
            headers: BTreeMap::new(),
            body: vec![],
        };
        assert_eq!(r.path(), "/pair-verify");
        assert_eq!(r.query("x"), Some("2"));
    }
    #[test]
    fn malformed_metadata_stays_bounded() {
        let mut m = BTreeMap::new();
        metadata(b"minm\xff\xff\xff\xff", 0, &mut m);
        assert!(m.is_empty());
    }
}
