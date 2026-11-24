#![allow(
    dead_code,
    reason = "each test crate uses a different subset of these helpers"
)]

use std::fs;
use std::path::PathBuf;

use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;

pub(crate) const BIN: &str = env!("CARGO_BIN_EXE_buzzard");
pub(crate) const V23: u32 = 0x2300_8000;
pub(crate) const CLEAN: [u32; 6] = [0, 0x400, 0, 0, 0, 0];
pub(crate) const INLINE_CODE: [u32; 6] = [0; 6];

pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("buzzard-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the temp dir");
        // Editors and the server exchange canonical paths.
        let canonical = dir.canonicalize().expect("canonicalize the temp dir");
        let text = canonical.to_string_lossy();
        Self(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)))
    }

    pub(crate) fn vi(&self, name: &str, words: &[u32], calls: &[&str]) {
        let links: Vec<(&str, LvPath)> = calls.iter().map(|n| (*n, LvPath::sibling(n))).collect();
        let bytes = RsrcBuilder::default()
            .save_record(V23, words)
            .links(&links)
            .build();
        fs::write(self.0.join(name), bytes).expect("write the VI");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../buzzard-rsrc/tests/fixtures/pylavi")
}
