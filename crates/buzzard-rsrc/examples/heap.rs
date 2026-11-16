//! Print the object tree of a heap block:
//! `cargo run --example heap -- file.vi [BDHb|FPHb]`

#![expect(
    clippy::print_stdout,
    reason = "an example that reports to the terminal"
)]

use buzzard_rsrc::blocks::heap::{Content, Heap, HeapNode};
use buzzard_rsrc::blocks::zlib;
use buzzard_rsrc::RsrcFile;

fn print(node: &HeapNode<'_>, depth: usize) {
    let name = node
        .tag_name()
        .map_or_else(|| format!("tag{}", node.tag), str::to_owned);
    print!("{}{name}", "  ".repeat(depth));
    if let Some(class) = node.class() {
        let class = node
            .class_name()
            .map_or_else(|| class.to_string(), str::to_owned);
        print!(" class={class}");
    }
    for (id, value) in &node.attrs {
        match *id {
            -2 => {}
            -3 => print!(" uid={value}"),
            other => print!(" attr{other}={value}"),
        }
    }
    match node.content {
        Content::None => {}
        Content::Bool(b) => print!(" = {b}"),
        Content::Bytes(b) => {
            print!(" = 0x");
            for x in b.iter().take(24) {
                print!("{x:02x}");
            }
            if b.len() > 24 {
                print!("...");
            }
        }
    }
    println!();
    for child in &node.children {
        print(child, depth + 1);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: heap <file> [BDHb|FPHb]");
    let block = args.next().unwrap_or_else(|| "BDHb".to_owned());
    let block: [u8; 4] = block
        .as_bytes()
        .try_into()
        .expect("block type is four characters");
    let bytes = std::fs::read(&path).expect("read file");
    let file = RsrcFile::parse(&bytes).expect("parse container");
    let data = zlib::decompress(file.first(&block).expect("block not present")).expect("inflate");
    let heap = Heap::parse(&data).expect("parse heap");
    for root in &heap.roots {
        print(root, 0);
    }
}
