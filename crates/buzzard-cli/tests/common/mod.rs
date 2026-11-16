#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;

pub const BIN: &str = env!("CARGO_BIN_EXE_buzzard");
pub const V23: u32 = 0x2300_8000;
pub const CLEAN: [u32; 6] = [0, 0x400, 0, 0, 0, 0];
pub const INLINE_CODE: [u32; 6] = [0; 6];

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("buzzard-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Editors and the server exchange canonical paths.
        Self(dir.canonicalize().unwrap())
    }

    pub fn vi(&self, name: &str, words: &[u32], calls: &[&str]) {
        let links: Vec<(&str, LvPath)> = calls.iter().map(|n| (*n, LvPath::sibling(n))).collect();
        let bytes = RsrcBuilder::default()
            .save_record(V23, words)
            .links(&links)
            .build();
        fs::write(self.0.join(name), bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../buzzard-rsrc/tests/fixtures/pylavi")
}
