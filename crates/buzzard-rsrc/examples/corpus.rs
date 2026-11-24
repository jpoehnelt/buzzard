//! Parse every RSRC file under a directory and report failures, as a quick
//! conformance check against a large body of real files:
//! `cargo run --release --example corpus -- <dir>`

#![expect(
    clippy::print_stdout,
    reason = "an example that reports to the terminal"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use buzzard_rsrc::blocks::heap::{Heap, HeapNode};
use buzzard_rsrc::blocks::zlib;
use buzzard_rsrc::{BlockDiagram, RsrcFile, ViMetadata, RSRC_EXTENSIONS};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| RSRC_EXTENSIONS.iter().any(|k| e.eq_ignore_ascii_case(k)))
        {
            out.push(path);
        }
    }
}

fn classes(node: &HeapNode<'_>, counts: &mut BTreeMap<String, usize>) {
    if let Some(class) = node.class() {
        let name = node
            .class_name()
            .map_or_else(|| format!("class{class}"), str::to_owned);
        *counts.entry(name).or_default() += 1;
    }
    for child in &node.children {
        classes(child, counts);
    }
}

fn main() {
    let mut files = Vec::new();
    for root in std::env::args().skip(1) {
        walk(Path::new(&root), &mut files);
    }
    files.sort();

    let mut versions: BTreeMap<String, usize> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let (mut failures, mut heaps, mut protected) = (0, 0, 0);
    let (mut diagrams, mut nodes, mut wires) = (0, 0, 0);
    for path in &files {
        let bytes = std::fs::read(path).expect("read file");
        let fail = |what: &str, err: &dyn std::fmt::Display| {
            println!("FAIL {what}: {}: {err}", path.display());
        };
        let file = match RsrcFile::parse(&bytes) {
            Ok(file) => file,
            Err(err) => {
                fail("container", &err);
                failures += 1;
                continue;
            }
        };
        match ViMetadata::from_rsrc(&file) {
            Ok(meta) => {
                let version = meta.version.map(|v| format!("{}.{}", v.major, v.minor));
                *versions
                    .entry(version.unwrap_or_else(|| "none".into()))
                    .or_default() += 1;
                protected += usize::from(meta.password_protected == Some(true));
            }
            Err(err) => {
                fail("metadata", &err);
                failures += 1;
            }
        }
        match BlockDiagram::from_rsrc(&file) {
            Ok(Some(diagram)) => {
                diagrams += 1;
                nodes += diagram.nodes().count();
                wires += diagram.wires().count();
            }
            Ok(None) => {}
            Err(err) => {
                fail("diagram", &err);
                failures += 1;
            }
        }
        for ty in [b"BDHb", b"FPHb"] {
            for resource in file.of_type(ty) {
                let parsed = zlib::decompress(resource.data).and_then(|data| {
                    let heap = Heap::parse(&data)?;
                    heap.roots
                        .iter()
                        .for_each(|root| classes(root, &mut counts));
                    Ok(())
                });
                match parsed {
                    Ok(()) => heaps += 1,
                    Err(err) => {
                        fail(&resource.ty.to_string(), &err);
                        failures += 1;
                    }
                }
            }
        }
    }
    println!(
        "{} files, {heaps} heaps decoded, {protected} password-protected, {failures} failures",
        files.len()
    );
    println!("{diagrams} block diagrams, {nodes} nodes, {wires} wires");
    println!("versions: {versions:?}");
    let mut by_count: Vec<_> = counts.into_iter().collect();
    by_count.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (name, count) in by_count {
        println!("{count:>7} {name}");
    }
}
