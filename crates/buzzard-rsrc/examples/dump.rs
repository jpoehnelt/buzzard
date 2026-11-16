//! Dump the resource directory of RSRC files: `cargo run --example dump -- file.vi`

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read file");
        match buzzard_rsrc::RsrcFile::parse(&bytes) {
            Ok(file) => {
                println!("{path}: {} ({} bytes)", file.file_type(), bytes.len());
                for r in file.resources() {
                    let head: String = r.data.iter().take(40).map(|b| format!("{b:02x}")).collect();
                    println!("  {} id={} name={:?} len={} {head}", r.ty, r.id, r.name, r.data.len());
                }
            }
            Err(err) => println!("{path}: error: {err}"),
        }
    }
}
