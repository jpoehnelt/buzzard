//! `LIBN`: names of the libraries (`.lvlib`, `.lvclass`) that own this file.

use crate::error::{malformed, Result};
use crate::reader::{decode_text, Reader};

pub fn parse(data: &[u8]) -> Result<Vec<String>> {
    let mut r = Reader::new(data, "LIBN");
    let count = r.u32()? as usize;
    if count > r.remaining() {
        return Err(malformed("LIBN", format!("implausible name count {count}")));
    }
    (0..count).map(|_| Ok(decode_text(r.pstr()?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        let data = b"\x00\x00\x00\x02\x09Foo.lvlib\x0bBar.lvclass";
        assert_eq!(parse(data).unwrap(), ["Foo.lvlib", "Bar.lvclass"]);
    }

    #[test]
    fn rejects_bogus_count() {
        assert!(parse(b"\xff\xff\xff\xff").is_err());
    }
}
