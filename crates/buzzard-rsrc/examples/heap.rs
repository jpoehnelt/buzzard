//! Print the object tree of a heap block:
//! `cargo run --example heap -- file.vi [BDHb|FPHb]`

use buzzard_rsrc::blocks::heap::{Content, Heap, HeapNode};
use buzzard_rsrc::blocks::zlib;
use buzzard_rsrc::RsrcFile;

fn print(node: &HeapNode<'_>, depth: usize) {
    let name = node
        .tag_name()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("tag{}", node.tag));
    let mut line = format!("{}{name}", "  ".repeat(depth));
    if let Some(class) = node.class() {
        line += &format!(
            " class={}",
            node.class_name()
                .map(str::to_owned)
                .unwrap_or(class.to_string())
        );
    }
    for (id, value) in &node.attrs {
        match *id {
            -2 => {}
            -3 => line += &format!(" uid={value}"),
            other => line += &format!(" attr{other}={value}"),
        }
    }
    match node.content {
        Content::None => {}
        Content::Bool(b) => line += &format!(" = {b}"),
        Content::Bytes(b) => {
            let hex: String = b.iter().take(24).map(|x| format!("{x:02x}")).collect();
            line += &format!(" = 0x{hex}{}", if b.len() > 24 { "..." } else { "" });
        }
    }
    println!("{line}");
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
