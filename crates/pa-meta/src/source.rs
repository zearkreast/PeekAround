//! Random-access byte sources: metadata readers only touch headers / metadata segments,
//! so files are read in ranges instead of being loaded whole.

use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};

/// Random access to the bytes of one file.
pub trait Source {
    /// total length in bytes
    fn len(&self) -> u64;
    /// true when empty
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// read `len` bytes at `off` (shorter at EOF; None on IO errors)
    fn read(&self, off: u64, len: usize) -> Option<Vec<u8>>;
    /// read exactly `len` bytes at `off`
    fn exact(&self, off: u64, len: usize) -> Option<Vec<u8>> {
        self.read(off, len).filter(|b| b.len() == len)
    }
}

impl Source for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read(&self, off: u64, len: usize) -> Option<Vec<u8>> {
        let start = usize::try_from(off).ok()?.min(<[u8]>::len(self));
        let end = start.saturating_add(len).min(<[u8]>::len(self));
        Some(self[start..end].to_vec())
    }
}

impl Source for Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }
    fn read(&self, off: u64, len: usize) -> Option<Vec<u8>> {
        self.as_slice().read(off, len)
    }
}

type CachedBlock = (u64, Vec<u8>);

/// A seekable reader (e.g. `std::fs::File`) as a [`Source`], with a small block cache.
pub struct ReaderSource<R: Read + Seek> {
    inner: RefCell<(R, Vec<CachedBlock>)>,
    len: u64,
}

const BLOCK: u64 = 64 * 1024;
const CACHED_BLOCKS: usize = 16;

impl<R: Read + Seek> ReaderSource<R> {
    /// wrap a reader; its length is taken from seeking to the end
    pub fn new(mut r: R) -> std::io::Result<Self> {
        let len = r.seek(SeekFrom::End(0))?;
        Ok(ReaderSource {
            inner: RefCell::new((r, Vec::new())),
            len,
        })
    }

    fn block(&self, idx: u64) -> Option<Vec<u8>> {
        let mut g = self.inner.borrow_mut();
        if let Some((_, b)) = g.1.iter().find(|(i, _)| *i == idx) {
            return Some(b.clone());
        }
        let start = idx * BLOCK;
        let want = BLOCK.min(self.len.saturating_sub(start)) as usize;
        let mut buf = vec![0u8; want];
        g.0.seek(SeekFrom::Start(start)).ok()?;
        g.0.read_exact(&mut buf).ok()?;
        if g.1.len() >= CACHED_BLOCKS {
            g.1.remove(0);
        }
        g.1.push((idx, buf.clone()));
        Some(buf)
    }
}

impl<R: Read + Seek> Source for ReaderSource<R> {
    fn len(&self) -> u64 {
        self.len
    }
    fn read(&self, off: u64, len: usize) -> Option<Vec<u8>> {
        if off >= self.len {
            return Some(vec![]);
        }
        let end = off.saturating_add(len as u64).min(self.len);
        let mut out = Vec::with_capacity((end - off) as usize);
        let mut pos = off;
        while pos < end {
            let idx = pos / BLOCK;
            let b = self.block(idx)?;
            let from = (pos - idx * BLOCK) as usize;
            let to = ((end - idx * BLOCK) as usize).min(b.len());
            if from >= to {
                break;
            }
            out.extend_from_slice(&b[from..to]);
            pos = idx * BLOCK + to as u64;
        }
        Some(out)
    }
}
