//! Positional reads: the only way this crate touches a file.
//!
//! The workspace forbids `unsafe`, so a file cannot be memory-mapped here.
//! Every read is a positional read into a buffer the reader owns, which
//! also means that a file that shrinks while it is open produces an error
//! and never a bus fault. A mapped backend can be added later by
//! implementing [`ReadAt`] in a crate that allows `unsafe`.

use std::fs::File;
use std::io;
use std::sync::Arc;

/// A source of bytes that can be read at any offset from several threads at
/// once.
pub trait ReadAt: Send + Sync {
    /// Total number of bytes. It must not change while the source is in use;
    /// [`ReadAt`] for [`File`] is wrapped so that the size is taken once.
    fn size(&self) -> u64;

    /// Fills `buf` with the bytes at `offset`.
    ///
    /// # Errors
    ///
    /// An error of kind [`io::ErrorKind::UnexpectedEof`] when fewer than
    /// `buf.len()` bytes exist at `offset`, or the error of the operating
    /// system.
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

/// A file with the size it had when it was opened.
#[derive(Debug)]
pub struct FileSource {
    file: File,
    size: u64,
    #[cfg(not(any(unix, windows)))]
    lock: std::sync::Mutex<()>,
}

impl FileSource {
    /// Wraps an open file.
    ///
    /// # Errors
    ///
    /// When the file's size cannot be read.
    pub fn new(file: File) -> io::Result<Self> {
        let size = file.metadata()?.len();
        Ok(Self {
            file,
            size,
            #[cfg(not(any(unix, windows)))]
            lock: std::sync::Mutex::new(()),
        })
    }
}

impl ReadAt for FileSource {
    fn size(&self) -> u64 {
        self.size
    }

    #[cfg(unix)]
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(&self.file, buf, offset)
    }

    #[cfg(windows)]
    fn read_exact_at(&self, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buf.is_empty() {
            match self.file.seek_read(buf, offset) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("a reader thread panicked"))?;
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)
    }
}

fn read_slice(data: &[u8], offset: u64, buf: &mut [u8]) -> io::Result<()> {
    let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
    let end = start
        .checked_add(buf.len())
        .ok_or(io::ErrorKind::UnexpectedEof)?;
    let src = data.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?;
    buf.copy_from_slice(src);
    Ok(())
}

impl ReadAt for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_slice(self, offset, buf)
    }
}

impl ReadAt for Arc<[u8]> {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_slice(self, offset, buf)
    }
}

/// A read-ahead window over a source, for walking headers that sit close
/// together.
pub(crate) struct Window<'a> {
    src: &'a dyn ReadAt,
    start: u64,
    buf: Vec<u8>,
    ahead: usize,
}

impl<'a> Window<'a> {
    pub(crate) fn new(src: &'a dyn ReadAt, ahead: usize) -> Self {
        Self {
            src,
            start: 0,
            buf: Vec::new(),
            ahead,
        }
    }

    /// The `len` bytes at `offset`. The caller has checked that they lie
    /// inside the source, so `len` is bounded by the size of the file.
    pub(crate) fn fetch(&mut self, offset: u64, len: usize) -> io::Result<&[u8]> {
        let inside = offset >= self.start
            && usize::try_from(offset - self.start)
                .ok()
                .and_then(|at| at.checked_add(len))
                .is_some_and(|end| end <= self.buf.len());
        if !inside {
            let left = self.src.size().saturating_sub(offset);
            let want = len.max(self.ahead);
            let want = usize::try_from(left).map_or(want, |left| want.min(left));
            if want < len {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.buf.clear();
            self.buf
                .try_reserve(want)
                .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
            self.buf.resize(want, 0);
            self.start = offset;
            if let Err(e) = self.src.read_exact_at(offset, &mut self.buf) {
                self.buf.clear();
                return Err(e);
            }
        }
        let at = usize::try_from(offset - self.start).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        Ok(&self.buf[at..at + len])
    }
}
