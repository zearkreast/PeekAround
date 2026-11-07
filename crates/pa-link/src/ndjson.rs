//! Split a byte stream into NDJSON lines without buffering the whole body (`ndjsonLines`).

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use pa_jobs::HttpError;

pub struct NdjsonLines<S> {
    src: S,
    buf: Vec<u8>,
    max_line: usize,
    done: bool,
}

impl<S, E> NdjsonLines<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    pub fn new(src: S, max_line: usize) -> Self {
        Self {
            src,
            buf: Vec::new(),
            max_line,
            done: false,
        }
    }

    /// next non-empty (trimmed) line; 413 'NDJSON line too long' past `max_line` bytes
    pub async fn next_line(&mut self) -> Result<Option<String>, HttpError> {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                let s = String::from_utf8_lossy(&line[..line.len() - 1]);
                let t = pa_core::js::trim(&s);
                if !t.is_empty() {
                    return Ok(Some(t.to_string()));
                }
                continue;
            }
            if self.done {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let rest = std::mem::take(&mut self.buf);
                let s = String::from_utf8_lossy(&rest);
                let t = pa_core::js::trim(&s);
                return Ok((!t.is_empty()).then(|| t.to_string()));
            }
            if self.buf.len() > self.max_line {
                return Err(HttpError::new(413, "NDJSON line too long"));
            }
            match self.src.next().await {
                Some(Ok(c)) => self.buf.extend_from_slice(&c),
                Some(Err(e)) => return Err(HttpError::new(502, e.to_string())),
                None => self.done = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn splits_across_chunks() {
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"{\"a\":1}\n{\"b\"")),
            Ok(Bytes::from_static(b":2}\n\n  \n{\"c\":\xc3")),
            Ok(Bytes::from_static(b"\xa7}")),
        ];
        let mut l = NdjsonLines::new(futures_util::stream::iter(chunks), 1024);
        assert_eq!(l.next_line().await.unwrap().unwrap(), "{\"a\":1}");
        assert_eq!(l.next_line().await.unwrap().unwrap(), "{\"b\":2}");
        assert_eq!(l.next_line().await.unwrap().unwrap(), "{\"c\":ç}");
        assert_eq!(l.next_line().await.unwrap(), None);
        let big: Vec<Result<Bytes, std::io::Error>> = vec![Ok(Bytes::from(vec![b'x'; 2000]))];
        let mut l = NdjsonLines::new(futures_util::stream::iter(big), 1024);
        assert_eq!(l.next_line().await.unwrap_err().status, 413);
    }
}
