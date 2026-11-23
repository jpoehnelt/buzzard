//! Project-level XML files: `.lvproj`, `.lvlib` and `.lvclass`.
//!
//! All three share one layout: a root element (`Project`, `Library` or
//! `LVClass`) with an `LVVersion` attribute, `Property` children, and a tree
//! of `Item` elements. An item that refers to a file has a `URL` attribute.
//!
//! Two details were established from real files rather than documentation
//! (see `tests/fixtures/project/README.md`):
//!
//! * A relative `URL` is relative to the XML file *itself*, not to its
//!   directory, so a sibling file is `../Sibling.vi`. A URL that does not
//!   leave the file (for example the private data control of a class, stored
//!   as `Name.ctl`) names something embedded in the XML file.
//! * The parent of a class is stored in the `NI.LVClass.ParentClassLinkInfo`
//!   property as a flattened byte string in a printable encoding, decoded by
//!   [`decode_bin`] and [`parse_parent_link`].

use std::path::{Path, PathBuf};

use buzzard_rsrc::{LvPath, Version};
use roxmltree::{Document, Node, ParsingOptions};
use serde::Serialize;

use crate::graph::resolve;
use crate::workspace::normalize;

/// File extensions (lowercase, without the dot) of project-level XML files.
pub const PROJECT_EXTENSIONS: [&str; 3] = ["lvproj", "lvlib", "lvclass"];

/// Largest file that is parsed at all.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest number of XML nodes in one file. Bounds memory on hostile input;
/// real projects with thousands of members stay far below it.
pub const MAX_NODES: u32 = 1_000_000;
/// Longest `ParentClassLinkInfo` text that is decoded.
const MAX_PARENT_LINK_CHARS: usize = 64 * 1024;
const MAX_NAME_PARTS: u32 = 32;

const PARENT_LINK_PROPERTY: &str = "NI.LVClass.ParentClassLinkInfo";

/// Which of the three project-level file types a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// A `.lvproj` project.
    Project,
    /// A `.lvlib` library.
    Library,
    /// A `.lvclass` class.
    Class,
}

impl ProjectKind {
    /// The kind implied by a file extension, if it is a project-level file.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("lvproj") {
            Some(ProjectKind::Project)
        } else if ext.eq_ignore_ascii_case("lvlib") {
            Some(ProjectKind::Library)
        } else if ext.eq_ignore_ascii_case("lvclass") {
            Some(ProjectKind::Class)
        } else {
            None
        }
    }

    /// Name shown in summaries, e.g. `Library`.
    pub fn label(self) -> &'static str {
        match self {
            ProjectKind::Project => "Project",
            ProjectKind::Library => "Library",
            ProjectKind::Class => "Class",
        }
    }
}

/// True if the path has a project-level XML extension.
pub fn is_project_file(path: &Path) -> bool {
    ProjectKind::from_path(path).is_some()
}

/// One `Item` element that has a `URL` attribute.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Member {
    /// The `Name` attribute, e.g. `Run.vi`.
    pub name: String,
    /// The `Type` attribute, e.g. `VI`, `LVClass`, `Library`, `Document`.
    pub item_type: String,
    /// The `URL` attribute exactly as stored.
    pub url: String,
    /// Where the URL points on disk. `None` for installation paths such as
    /// `/<vilib>/...`, other absolute URLs, and items embedded in the XML
    /// file itself.
    pub path: Option<PathBuf>,
    /// True for an item under a project's `Dependencies` node. Those are
    /// computed by the editor, not chosen by the user.
    pub dependency: bool,
}

/// The parent of a class, from `NI.LVClass.ParentClassLinkInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ParentClass {
    /// File name of the parent class, e.g. `Base.lvclass`.
    pub name: String,
    /// Owning libraries followed by the class name.
    pub qualified_name: Vec<String>,
    /// The stored link, as written by the editor.
    pub link: LvPath,
    /// The link resolved against the class file; `None` for installation
    /// paths such as `<vilib>/...`.
    pub path: Option<PathBuf>,
}

/// A parsed `.lvproj`, `.lvlib` or `.lvclass` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectFile {
    /// Which kind of file this is, from its extension rather than its root
    /// element.
    pub kind: ProjectKind,
    /// File name, e.g. `Motor.lvclass`.
    pub name: String,
    /// Editor version that saved the file, e.g. `19.0.0`. The raw attribute
    /// is kept when it is not the usual eight hex digits.
    pub lv_version: Option<String>,
    /// Every item with a URL, in document order.
    pub members: Vec<Member>,
    /// The parent named by `NI.LVClass.ParentClassLinkInfo`. `None` when
    /// the file names no parent or the stored value cannot be decoded.
    pub parent_class: Option<ParentClass>,
}

impl ProjectFile {
    /// Read and parse one file. The kind comes from the extension.
    ///
    /// # Errors
    ///
    /// Returns the reason as text when the file cannot be read, is larger
    /// than [`MAX_FILE_BYTES`], or does not parse (see [`ProjectFile::parse`]).
    pub fn from_path(path: &Path) -> Result<Self, String> {
        if crate::archive::split(path).is_some() {
            return Self::from_bytes(path, &crate::archive::read(path)?);
        }
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > MAX_FILE_BYTES {
            return Err(format!(
                "file is {size} bytes, over the limit of {MAX_FILE_BYTES}"
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::parse_bytes(path, &bytes)
    }

    /// Parse the contents of a project file that did not come from a file
    /// of its own, such as a member of an `.llb` archive at the virtual
    /// path `path`. The size limit of [`from_path`](Self::from_path)
    /// holds.
    ///
    /// # Errors
    ///
    /// As [`from_path`](Self::from_path).
    pub fn from_bytes(path: &Path, bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!(
                "file is {} bytes, over the limit of {MAX_FILE_BYTES}",
                bytes.len()
            ));
        }
        Self::parse_bytes(path, bytes)
    }

    /// Parse file contents that may not be valid UTF-8. Older editor
    /// versions wrote the system code page; those bytes map through Latin-1.
    ///
    /// # Errors
    ///
    /// As for [`ProjectFile::parse`].
    pub fn parse_bytes(path: &Path, bytes: &[u8]) -> Result<Self, String> {
        if let Ok(text) = std::str::from_utf8(bytes) {
            Self::parse(path, text)
        } else {
            let text: String = bytes.iter().map(|&b| b as char).collect();
            Self::parse(path, &text)
        }
    }

    /// Parse XML text. `path` is where the file lives; member URLs and the
    /// parent class link are resolved against it.
    ///
    /// # Errors
    ///
    /// Returns the reason as text when `path` does not have one of the three
    /// extensions; when the text is not well-formed XML, has a DTD, or has
    /// more than [`MAX_NODES`] nodes; or when the root element is not
    /// `Project`, `Library` or `LVClass`.
    pub fn parse(path: &Path, text: &str) -> Result<Self, String> {
        let kind = ProjectKind::from_path(path)
            .ok_or_else(|| "not a .lvproj, .lvlib or .lvclass file".to_owned())?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        // A DTD is never present in real files and is the vehicle for entity
        // expansion attacks, so it stays disallowed.
        let options = ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            ..ParsingOptions::default()
        };
        let document =
            Document::parse_with_options(text, options).map_err(|e| format!("invalid XML: {e}"))?;
        let root = document.root_element();
        let root_name = root.tag_name().name();
        if !matches!(root_name, "Project" | "Library" | "LVClass") {
            return Err(format!("unexpected root element <{root_name}>"));
        }

        let path = normalize(path);
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let lv_version = root.attribute("LVVersion").map(format_version);
        let parent_class = root
            .children()
            .find(|n| {
                n.has_tag_name("Property") && n.attribute("Name") == Some(PARENT_LINK_PROPERTY)
            })
            .and_then(|n| parent_from_text(&path, &node_text(n)));

        Ok(Self {
            kind,
            name,
            lv_version,
            members: collect_members(&path, root),
            parent_class,
        })
    }
}

/// `13008000` -> `13.0.0`; anything else is returned unchanged.
fn format_version(raw: &str) -> String {
    if raw.len() == 8 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        if let Ok(value) = u32::from_str_radix(raw, 16) {
            return Version::from_u32(value).to_string();
        }
    }
    raw.to_owned()
}

fn node_text(node: Node<'_, '_>) -> String {
    node.children()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect()
}

/// Collect every item with a URL. Iterates in document order without
/// recursion, so nesting depth cannot overflow the stack.
fn collect_members(file: &Path, root: Node<'_, '_>) -> Vec<Member> {
    let mut members = Vec::new();
    // Node ids are assigned in document order, so a subtree is a contiguous
    // id range. This is the last id inside the current `Dependencies` item.
    let mut dependencies_end: Option<usize> = None;
    for node in root.descendants().filter(|n| n.has_tag_name("Item")) {
        let id = node.id().get_usize();
        let mut dependency = dependencies_end.is_some_and(|end| id <= end);
        let item_type = node.attribute("Type").unwrap_or_default();
        if !dependency && item_type == "Dependencies" {
            dependencies_end = node.descendants().next_back().map(|n| n.id().get_usize());
            dependency = true;
        }
        if let Some(url) = node.attribute("URL") {
            members.push(Member {
                name: node.attribute("Name").unwrap_or_default().to_owned(),
                item_type: item_type.to_owned(),
                url: url.to_owned(),
                path: resolve_url(file, url),
                dependency,
            });
        }
    }
    members
}

/// Turn an item URL into a file system path, as seen from the XML file.
///
/// Returns `None` for URLs that start with `/` (installation paths such as
/// `/<vilib>/...` and machine-absolute paths), for anything that looks like
/// a drive or scheme, and for items that stay inside the XML file.
pub fn resolve_url(file: &Path, url: &str) -> Option<PathBuf> {
    if url.is_empty() || url.starts_with('/') || url.contains(['\\', ':', '<', '>', '\0']) {
        return None;
    }
    let file = normalize(file);
    let mut out = file.clone();
    for segment in url.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                let can_pop = out.file_name().is_some();
                if can_pop {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    let out = normalize(&out);
    (!out.starts_with(&file)).then_some(out)
}

fn parent_from_text(file: &Path, text: &str) -> Option<ParentClass> {
    let text = text.trim();
    if text.len() > MAX_PARENT_LINK_CHARS {
        return None;
    }
    let (qualified_name, link) = parse_parent_link(&decode_bin(text)?)?;
    let name = qualified_name.last()?.clone();
    let path = resolve(file, &link);
    Some(ParentClass {
        name,
        qualified_name,
        link,
        path,
    })
}

/// Decode the text of a `Type="Bin"` property.
///
/// Each character carries six bits, most significant first, as the byte
/// `0x21 + value` (so `!` is 0 and `` ` `` is 63). There is no padding
/// character: the bits left over after the last whole byte must be zero.
/// Returns `None` for any character outside that range.
pub fn decode_bin(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        if !(0x21..=0x60).contains(&byte) {
            return None;
        }
        acc = (acc << 6) | u32::from(byte - 0x21);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "`acc` is below `1 << (bits + 8)` here, so the shifted value fits in a byte"
            )]
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (acc == 0).then_some(out)
}

/// The type description that precedes the data in every observed
/// `ParentClassLinkInfo` value: two types (a `u8` and a one-dimensional
/// array of it), then the array as the single top-level value.
const PARENT_LINK_TYPES: [u8; 25] = [
    0x00, 0x00, 0x00, 0x02, // two type descriptors
    0x00, 0x05, 0x00, 0x05, 0x00, // u8
    0x00, 0x0c, 0x00, 0x40, 0x00, 0x01, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, // array of #0
    0x00, 0x01, 0x00, 0x01, // one value, of type #1
];

/// Parse a decoded `ParentClassLinkInfo` value into the parent's qualified
/// name and stored path.
///
/// ```text
/// u32      editor version
/// 25 bytes type description (see `PARENT_LINK_TYPES`)
/// u32      length of the byte array that follows
///   u32    qualified-name part count, then that many Pascal strings
///   pad    to a 2-byte boundary
///   PTHx   flattened path, to the end of the array
/// ```
///
/// Anything that deviates from this layout yields `None` rather than a
/// guess.
pub fn parse_parent_link(bytes: &[u8]) -> Option<(Vec<String>, LvPath)> {
    let rest = bytes.get(4..)?;
    let rest = rest.strip_prefix(&PARENT_LINK_TYPES)?;
    let len = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let data = rest.get(4..)?.get(..len)?;

    let count = u32::from_be_bytes(data.get(..4)?.try_into().ok()?);
    if count == 0 || count > MAX_NAME_PARTS {
        return None;
    }
    let mut pos = 4;
    let mut qualified_name = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let part_len = usize::from(*data.get(pos)?);
        let part = data.get(pos + 1..pos + 1 + part_len)?;
        if part.is_empty() {
            return None;
        }
        qualified_name.push(decode_text(part));
        pos += 1 + part_len;
    }
    pos += pos % 2;
    let (link, used) = LvPath::parse(data.get(pos..)?).ok()?;
    if pos + used != data.len() || link.is_empty() {
        return None;
    }
    Some((qualified_name, link))
}

/// Same policy as the binary parser: UTF-8 when valid, else Latin-1.
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Encode bytes the way [`decode_bin`] reads them. Used to build fixtures.
pub fn encode_bin(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 1);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &byte in bytes {
        acc = (acc << 8) | u32::from(byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(char::from(0x21 + ((acc >> bits) & 0x3f) as u8));
            acc &= (1 << bits) - 1;
        }
    }
    if bits > 0 {
        out.push(char::from(0x21 + ((acc << (6 - bits)) & 0x3f) as u8));
    }
    out
}

/// Append a Pascal string, truncated to the 255 bytes its length can hold.
fn push_pstr(out: &mut Vec<u8>, text: &str) {
    let len = u8::try_from(text.len()).unwrap_or(u8::MAX);
    out.push(len);
    out.extend_from_slice(&text.as_bytes()[..usize::from(len)]);
}

/// Build the text of a `ParentClassLinkInfo` property. Used to build
/// fixtures; the output has the layout [`parse_parent_link`] reads.
///
/// # Panics
///
/// If a count or length does not fit its field: more than `u32::MAX` name
/// parts, more than `u16::MAX` path components, or 4 GiB or more of data.
pub fn encode_parent_link(version: u32, qualified_name: &[&str], link: &LvPath) -> String {
    let part_count = u32::try_from(qualified_name.len()).expect("name part count fits in a u32");
    let mut data = part_count.to_be_bytes().to_vec();
    for part in qualified_name {
        push_pstr(&mut data, part);
    }
    if data.len() % 2 == 1 {
        data.push(0);
    }
    let component_count =
        u16::try_from(link.components.len()).expect("path component count fits in a u16");
    let mut body = Vec::new();
    body.extend_from_slice(&u16::from(link.kind == buzzard_rsrc::PathKind::Relative).to_be_bytes());
    body.extend_from_slice(&component_count.to_be_bytes());
    for component in &link.components {
        push_pstr(&mut body, component);
    }
    let body_len = u32::try_from(body.len()).expect("path length fits in a u32");
    data.extend_from_slice(b"PTH0");
    data.extend_from_slice(&body_len.to_be_bytes());
    data.extend(body);

    let data_len = u32::try_from(data.len()).expect("array length fits in a u32");
    let mut out = version.to_be_bytes().to_vec();
    out.extend_from_slice(&PARENT_LINK_TYPES);
    out.extend_from_slice(&data_len.to_be_bytes());
    out.extend(data);
    out.extend_from_slice(&[0; 4]);
    encode_bin(&out)
}

#[cfg(test)]
mod tests;
