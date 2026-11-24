//! Dump the resource directory of RSRC files: `cargo run --example dump -- file.vi`

#![expect(
    clippy::print_stdout,
    reason = "an example that reports to the terminal"
)]

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read file");
        match buzzard_rsrc::RsrcFile::parse(&bytes) {
            Ok(file) => {
                println!("{path}: {} ({} bytes)", file.file_type(), bytes.len());
                for r in file.resources() {
                    print!(
                        "  {} id={} name={:?} len={} ",
                        r.ty,
                        r.id,
                        r.name,
                        r.data.len()
                    );
                    for b in r.data.iter().take(40) {
                        print!("{b:02x}");
                    }
                    println!();
                }
            }
            Err(err) => println!("{path}: error: {err}"),
        }
    }
}
