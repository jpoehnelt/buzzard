//! Compressed blocks (`BDHb`, `FPHb`, `VCTP`, ...): a big-endian `u32`
//! uncompressed size followed by a zlib stream.

use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::error::{malformed, Result, RsrcError};
use crate::reader::Reader;

/// Upper bound on a single decompressed block, to bound memory on bad input.
const MAX_DECOMPRESSED: usize = 256 * 1024 * 1024;

pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(data, "compressed block");
    let size = r.u32()? as usize;
    if size > MAX_DECOMPRESSED {
        return Err(malformed("compressed block", format!("declared size {size} is too large")));
    }
    let mut out = Vec::with_capacity(size);
    ZlibDecoder::new(r.take(r.remaining())?)
        .take(size as u64 + 1)
        .read_to_end(&mut out)
        .map_err(RsrcError::Decompress)?;
    if out.len() != size {
        return Err(malformed(
            "compressed block",
            format!("declared {size} bytes but stream holds {}", out.len()),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    fn compress(payload: &[u8], declared: u32) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(declared.to_be_bytes().to_vec(), Compression::default());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn round_trips() {
        let payload = b"hello hello hello hello";
        assert_eq!(decompress(&compress(payload, payload.len() as u32)).unwrap(), payload);
    }

    #[test]
    fn rejects_size_mismatch_and_garbage() {
        assert!(decompress(&compress(b"abcdef", 3)).is_err());
        assert!(decompress(&compress(b"abc", 6)).is_err());
        assert!(decompress(b"\x00\x00\x00\x04nope").is_err());
        assert!(decompress(b"\xff\xff\xff\xff").is_err());
    }
}
