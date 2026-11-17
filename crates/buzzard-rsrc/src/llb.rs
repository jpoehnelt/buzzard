//! LLB archives (`.llb`): one RSRC container of file type `LVAR` that holds
//! many VIs and controls.
//!
//! Layout, as found in an archive saved by LabVIEW 2017 and in the empty
//! archive of the conformance fixtures (all integers big-endian):
//!
//! ```text
//! ADir resource (one, the directory)
//!   u32            0 in every sample; meaning unknown
//!   "adir"
//!   u32            directory version, 2
//!   u32, u32       0 in every sample; meaning unknown
//!   u32            member count
//!   per member:
//!     u32          id: the resource id of the member's content
//!     fourcc       file type of the member, e.g. LVIN or LVCC
//!     u32          version that saved the member, packed as in `vers`
//!     20 bytes     0 in every sample; meaning unknown
//!     pstr         file name, padded with zeros to a multiple of 4 bytes
//! UCRF resource    a member's file, stored as it is
//! ZCRF resource    a member's file: u32 size, then a zlib stream
//! CPRF resource    a member's file in the compression older than zlib
//! ```
//!
//! A content resource has the id of its directory entry and carries the
//! member's name as its resource name. The archive's other resources
//! (`PALM`, `PLM2`, `CPST`, icons, `STR#`, `CONP`) describe palettes and
//! are not read here.
//!
//! Everything in an archive is untrusted. The member count is checked
//! against the bytes the directory has, the size a compressed member
//! declares is checked against [`MAX_MEMBER_BYTES`] and against what its
//! zlib stream could hold before anything is inflated, and an archive
//! inside an archive is a member like any other: it is never opened.

use std::collections::HashMap;
use std::io::Read;

use flate2::read::ZlibDecoder;
use serde::Serialize;

use crate::container::RsrcFile;
use crate::error::{malformed, Result, RsrcError};
use crate::fourcc::FourCc;
use crate::header::file_type;
use crate::metadata::FileKind;
use crate::reader::{decode_text, Reader};
use crate::version::Version;

/// Largest file a member may inflate to. A member that declares more is
/// refused before anything is inflated.
pub const MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024;

/// The directory version this reader knows.
const DIRECTORY_VERSION: u32 = 2;
/// Bytes of the directory before its first entry.
const DIRECTORY_HEADER: usize = 24;
/// Bytes of an entry before its name.
const ENTRY_FIXED: usize = 32;
/// The smallest entry: the fixed part and an empty, padded name.
const ENTRY_MIN: usize = ENTRY_FIXED + 4;
/// No deflate stream holds more than this many bytes per byte of input
/// (the zlib documentation gives 1032:1 as the limit).
const MAX_DEFLATE_RATIO: u64 = 1032;

/// How a member's file is stored in the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Storage {
    /// As it is (`UCRF`).
    Stored,
    /// As a zlib stream behind its inflated size (`ZCRF`).
    Zlib,
    /// In the compression of archives older than LabVIEW 6 (`CPRF`).
    /// [`Llb::read`] does not decode it.
    Legacy,
}

impl Storage {
    /// The lowercase name, as used in JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Storage::Stored => "stored",
            Storage::Zlib => "zlib",
            Storage::Legacy => "legacy",
        }
    }

    fn block(self) -> &'static str {
        match self {
            Storage::Stored => "UCRF",
            Storage::Zlib => "ZCRF",
            Storage::Legacy => "CPRF",
        }
    }
}

/// One entry of an archive's directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlbMember {
    /// File name as the directory stores it. It is text of the saving
    /// machine's code page, shown one character per byte when it is not
    /// UTF-8. It is data, not a path: it may hold separators or be `..`.
    pub name: String,
    /// Id of the entry and of its content resource.
    pub id: i32,
    /// File type code of the member, e.g. `LVIN`.
    pub file_type: FourCc,
    /// What the member is, decided by [`file_type`](Self::file_type).
    pub kind: FileKind,
    /// Version of the editor that saved the member, as the directory
    /// states it; `None` when the field is zero.
    pub version: Option<Version>,
    /// How the member's file is stored.
    pub storage: Storage,
    /// Bytes the member takes in the archive.
    pub stored_size: u64,
    /// Bytes of the member's file as the archive declares them: the
    /// stored size of a [`Storage::Stored`] member, and the size prefix of
    /// a compressed one, which [`Llb::read`] checks.
    pub size: u64,
    #[serde(skip)]
    index: usize,
}

/// A parsed archive: its directory, and the bytes of its members on
/// request. Member contents borrow from the input buffer.
#[derive(Debug, Clone)]
pub struct Llb<'a> {
    members: Vec<LlbMember>,
    contents: Vec<&'a [u8]>,
    unlisted: usize,
}

impl<'a> Llb<'a> {
    /// Parse an archive's directory. No member is inflated.
    ///
    /// # Errors
    ///
    /// Any error of [`RsrcFile::parse`] or [`from_rsrc`](Self::from_rsrc).
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        Self::from_rsrc(&RsrcFile::parse(bytes)?)
    }

    /// Read the directory of an already parsed container.
    ///
    /// # Errors
    ///
    /// [`RsrcError::MalformedBlock`] if the container is not an archive
    /// (its file type is not `LVAR`), has no `ADir` directory or more than
    /// one, the directory has another version or layout than the one
    /// described in the [module documentation](self), an entry has no
    /// content resource, or two entries or two content resources share an
    /// id; [`RsrcError::OutOfBounds`] if the directory is shorter than its
    /// member count needs.
    pub fn from_rsrc(file: &RsrcFile<'a>) -> Result<Self> {
        if file.file_type() != file_type::LIBRARY {
            return Err(malformed(
                "ADir",
                format!(
                    "not an LLB archive: the file type is `{}`",
                    file.file_type()
                ),
            ));
        }
        let mut directories = file.of_type(b"ADir");
        let Some(directory) = directories.next() else {
            return Err(if file.first(b"LVzp").is_some() {
                malformed(
                    "LVzp",
                    "the archive holds a zipped program tree instead of a directory; \
                     archives built into an application are not opened",
                )
            } else {
                malformed("ADir", "the archive has no directory")
            });
        };
        if directories.next().is_some() {
            return Err(malformed("ADir", "the archive has more than one directory"));
        }

        let mut contents: HashMap<i32, (Storage, &'a [u8])> = HashMap::new();
        let mut content_count = 0usize;
        for storage in [Storage::Stored, Storage::Zlib, Storage::Legacy] {
            for resource in file.resources() {
                if resource.ty.as_bytes() != storage.block().as_bytes() {
                    continue;
                }
                content_count += 1;
                if contents
                    .insert(resource.id, (storage, resource.data))
                    .is_some()
                {
                    return Err(malformed(
                        "ADir",
                        format!("two content resources have the id {}", resource.id),
                    ));
                }
            }
        }

        let entries = read_directory(directory.data)?;
        let mut members = Vec::with_capacity(entries.len());
        let mut data = Vec::with_capacity(entries.len());
        for (index, entry) in entries.into_iter().enumerate() {
            let name = decode_text(entry.name);
            let Some((storage, content)) = contents.remove(&entry.id) else {
                // Either the content is absent or an earlier entry took it.
                return Err(malformed(
                    "ADir",
                    format!(
                        "member `{}` (id {}) has no content resource of its own",
                        printable(&name),
                        entry.id
                    ),
                ));
            };
            let size = match storage {
                Storage::Stored => content.len() as u64,
                Storage::Zlib | Storage::Legacy => content
                    .first_chunk::<4>()
                    .map_or(0, |prefix| u64::from(u32::from_be_bytes(*prefix))),
            };
            members.push(LlbMember {
                name,
                id: entry.id,
                file_type: entry.file_type,
                kind: FileKind::of(entry.file_type),
                version: (entry.version != 0).then(|| Version::from_u32(entry.version)),
                storage,
                stored_size: content.len() as u64,
                size,
                index,
            });
            data.push(content);
        }
        Ok(Self {
            unlisted: content_count - members.len(),
            members,
            contents: data,
        })
    }

    /// The members in directory order.
    pub fn members(&self) -> &[LlbMember] {
        &self.members
    }

    /// How many content resources no directory entry names. LabVIEW cannot
    /// link to them, and they are not read.
    pub fn unlisted(&self) -> usize {
        self.unlisted
    }

    /// The member called `name`: the one with exactly that name, or else
    /// the only one whose name differs from it in letter case alone (the
    /// editor treats member names like file names of Windows). When two
    /// members have exactly that name, the first one is returned.
    pub fn find(&self, name: &str) -> Option<&LlbMember> {
        if let Some(exact) = self.members.iter().find(|m| m.name == name) {
            return Some(exact);
        }
        let wanted = name.to_lowercase();
        let mut folded = self
            .members
            .iter()
            .filter(|m| m.name.to_lowercase() == wanted);
        let first = folded.next()?;
        folded.next().is_none().then_some(first)
    }

    /// The file of one member, inflated if it is stored compressed.
    ///
    /// # Errors
    ///
    /// [`RsrcError::MalformedBlock`] if `member` is not an entry of this
    /// archive, its file is over [`MAX_MEMBER_BYTES`] or declares to be,
    /// it declares more bytes than its zlib stream could hold, the stream
    /// inflates to another size than declared, or it is stored in the
    /// [`Storage::Legacy`] compression; [`RsrcError::Decompress`] if the
    /// zlib stream is corrupt.
    pub fn read(&self, member: &LlbMember) -> Result<Vec<u8>> {
        let own = self.members.get(member.index);
        let (Some(own), Some(&content)) = (own, self.contents.get(member.index)) else {
            return Err(foreign(member));
        };
        if own != member {
            return Err(foreign(member));
        }
        let block = member.storage.block();
        match member.storage {
            Storage::Stored => {
                check_size(block, content.len() as u64)?;
                Ok(content.to_vec())
            }
            Storage::Zlib => inflate(block, content),
            Storage::Legacy => Err(malformed(
                block,
                "the compression of archives saved before LabVIEW 6 is not supported",
            )),
        }
    }
}

fn foreign(member: &LlbMember) -> RsrcError {
    malformed(
        "ADir",
        format!(
            "`{}` is not a member of this archive",
            printable(&member.name)
        ),
    )
}

/// A member name on one line, for an error message.
fn printable(name: &str) -> String {
    name.chars().flat_map(char::escape_debug).collect()
}

fn check_size(block: &'static str, size: u64) -> Result<()> {
    if size > MAX_MEMBER_BYTES as u64 {
        return Err(malformed(
            block,
            format!("the member is {size} bytes, over the limit of {MAX_MEMBER_BYTES}"),
        ));
    }
    Ok(())
}

/// Inflate a `ZCRF` member: a `u32` size and a zlib stream.
///
/// Nothing is allocated from the declared size. It is refused when it is
/// over the limit or more than the stream could inflate to; the output
/// then grows only as the stream delivers bytes, and stops one byte past
/// the declared size.
fn inflate(block: &'static str, content: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(content, block);
    let declared = u64::from(r.u32()?);
    let stream = r.take(r.remaining())?;
    check_size(block, declared)?;
    if declared > (stream.len() as u64).saturating_mul(MAX_DEFLATE_RATIO) {
        return Err(malformed(
            block,
            format!(
                "the member declares {declared} bytes, more than its {}-byte stream can hold",
                stream.len()
            ),
        ));
    }
    let mut out = Vec::new();
    ZlibDecoder::new(stream)
        .take(declared + 1)
        .read_to_end(&mut out)
        .map_err(RsrcError::Decompress)?;
    if out.len() as u64 != declared {
        return Err(malformed(
            block,
            format!(
                "the member declares {declared} bytes but its stream holds {}",
                if out.len() as u64 > declared {
                    "more".to_owned()
                } else {
                    out.len().to_string()
                }
            ),
        ));
    }
    Ok(out)
}

struct Entry<'a> {
    id: i32,
    file_type: FourCc,
    version: u32,
    name: &'a [u8],
}

fn read_directory(data: &[u8]) -> Result<Vec<Entry<'_>>> {
    let mut r = Reader::new(data, "ADir");
    r.u32()?;
    if &r.fourcc()? != b"adir" {
        return Err(malformed(
            "ADir",
            "the directory does not start with `adir`",
        ));
    }
    let version = r.u32()?;
    if version != DIRECTORY_VERSION {
        return Err(malformed(
            "ADir",
            format!("directory version {version} is not known (only {DIRECTORY_VERSION} is)"),
        ));
    }
    r.take(8)?;
    let count = r.u32()? as usize;
    debug_assert_eq!(r.pos(), DIRECTORY_HEADER);
    // Every entry takes at least `ENTRY_MIN` bytes of the directory, so a
    // count the directory has no room for is refused before it is used.
    if count > r.remaining() / ENTRY_MIN {
        return Err(RsrcError::OutOfBounds {
            what: "ADir",
            offset: r.pos() as u64,
            len: count as u64,
            size: data.len(),
        });
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = r.u32()?.cast_signed();
        let file_type = FourCc(r.fourcc()?);
        let version = r.u32()?;
        r.take(ENTRY_FIXED - 12)?;
        let name = r.pstr()?;
        let padding = (4 - r.pos() % 4) % 4;
        if r.take(padding)?.iter().any(|&b| b != 0) {
            return Err(malformed("ADir", "the padding after a name is not zero"));
        }
        entries.push(Entry {
            id,
            file_type,
            version,
            name,
        });
    }
    if r.remaining() != 0 {
        return Err(malformed(
            "ADir",
            format!("{} bytes follow the last entry", r.remaining()),
        ));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{LlbBuilder, RsrcBuilder};
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;
    use std::time::Instant;

    fn vi(version: u32) -> Vec<u8> {
        RsrcBuilder::default()
            .save_record(version, &[0, 0x400])
            .build()
    }

    fn control() -> Vec<u8> {
        RsrcBuilder::new(file_type::CONTROL)
            .save_record(0x1700_8000, &[0, 0x400])
            .build()
    }

    fn names(llb: &Llb<'_>) -> Vec<String> {
        llb.members().iter().map(|m| m.name.clone()).collect()
    }

    fn reason(result: Result<impl std::fmt::Debug>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn several_members_read_back_stored_and_compressed() {
        let a = vi(0x2300_8000);
        let b = control();
        let c = vec![7u8; 5000];
        let bytes = LlbBuilder::default()
            .version(0x1700_8000)
            .member("Main.vi", &a)
            .compressed("Type.ctl", &b)
            .compressed("notes.txt", &c)
            .build();
        let llb = Llb::parse(&bytes).unwrap();
        assert_eq!(names(&llb), ["Main.vi", "Type.ctl", "notes.txt"]);
        let [main, ty, notes] = llb.members() else {
            panic!("three members")
        };
        assert_eq!(
            (main.kind, main.storage, main.size, main.stored_size),
            (
                FileKind::Vi,
                Storage::Stored,
                a.len() as u64,
                a.len() as u64
            )
        );
        assert_eq!(main.version.unwrap().to_string(), "17.0.0");
        assert_eq!(
            (ty.kind, ty.storage, ty.size),
            (FileKind::Control, Storage::Zlib, b.len() as u64)
        );
        assert_eq!((notes.kind, notes.size), (FileKind::Other, 5000));
        assert!(notes.stored_size < 100);
        assert_eq!(llb.read(main).unwrap(), a);
        assert_eq!(llb.read(ty).unwrap(), b);
        assert_eq!(llb.read(notes).unwrap(), c);
        assert_eq!(llb.unlisted(), 0);
        // Ids follow the directory, starting at 1.
        assert_eq!(
            llb.members().iter().map(|m| m.id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn an_empty_archive_has_no_members() {
        let bytes = LlbBuilder::default().build();
        let llb = Llb::parse(&bytes).unwrap();
        assert_eq!(llb.members().len(), 0);
        assert_eq!(llb.find("a.vi"), None);
    }

    #[test]
    fn duplicate_names_are_two_members() {
        let first = vi(0x2300_8000);
        let second = control();
        let bytes = LlbBuilder::default()
            .member("Same.vi", &first)
            .member("Same.vi", &second)
            .member("same.VI", &first)
            .build();
        let llb = Llb::parse(&bytes).unwrap();
        assert_eq!(names(&llb), ["Same.vi", "Same.vi", "same.VI"]);
        assert_eq!(llb.read(&llb.members()[1]).unwrap(), second);
        // An exact name finds the first of its entries.
        assert_eq!(llb.find("Same.vi").unwrap().id, 1);
        assert_eq!(llb.find("same.VI").unwrap().id, 3);
        // Three members differ from this spelling in case alone.
        assert_eq!(llb.find("SAME.VI"), None);
    }

    #[test]
    fn find_ignores_case_when_one_member_matches() {
        let bytes = LlbBuilder::default()
            .member("Simple Error Handler.vi", &vi(0x2300_8000))
            .build();
        let llb = Llb::parse(&bytes).unwrap();
        assert!(llb.find("simple error handler.VI").is_some());
        assert_eq!(llb.find("Simple Error Handler"), None);
    }

    #[test]
    fn odd_names_are_data() {
        // Shift-JIS bytes, separators, parent steps, an empty name and a
        // name of the longest length a Pascal string holds.
        let long = vec![b'x'; 255];
        let odd: [&[u8]; 8] = [
            b"\x83\x65\x83\x58\x83\x67.vi",
            b"../../escape.vi",
            b"..",
            b"a/b.vi",
            b"a\\b.vi",
            b"",
            b"C:evil.vi",
            &long,
        ];
        let mut builder = LlbBuilder::default();
        for name in odd {
            builder = builder.member_bytes(name, &vi(0x2300_8000));
        }
        let bytes = builder.build();
        let llb = Llb::parse(&bytes).unwrap();
        let expected: Vec<String> = odd.iter().map(|name| decode_text(name)).collect();
        assert_eq!(names(&llb), expected);
        assert_eq!(llb.members()[0].name.chars().count(), 9);
        for member in llb.members() {
            assert_eq!(llb.read(member).unwrap(), vi(0x2300_8000));
        }
        assert!(llb.find("../../escape.vi").is_some());
    }

    #[test]
    fn an_archive_in_an_archive_is_a_member_and_is_not_opened() {
        let inner = LlbBuilder::default()
            .member("Deep.vi", &vi(0x2300_8000))
            .build();
        let bytes = LlbBuilder::default()
            .compressed("inner.llb", &inner)
            .build();
        let llb = Llb::parse(&bytes).unwrap();
        assert_eq!(names(&llb), ["inner.llb"]);
        assert_eq!(llb.members()[0].kind, FileKind::Library);
        assert_eq!(llb.read(&llb.members()[0]).unwrap(), inner);
    }

    #[test]
    fn a_member_of_another_archive_is_refused() {
        let one = LlbBuilder::default().member("a.vi", &[1, 2, 3]).build();
        let two = LlbBuilder::default().member("b.vi", &[4, 5, 6]).build();
        let (one, two) = (Llb::parse(&one).unwrap(), Llb::parse(&two).unwrap());
        assert!(reason(one.read(&two.members()[0])).contains("not a member"));
    }

    #[test]
    fn what_is_not_an_archive_is_refused() {
        assert!(reason(Llb::parse(&vi(0x2300_8000))).contains("not an LLB archive"));
        let no_directory = RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"PALM", 0, None, vec![0; 4])
            .build();
        assert!(reason(Llb::parse(&no_directory)).contains("no directory"));
        let zipped = RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"LVzp", 0, None, vec![0; 40])
            .build();
        assert!(reason(Llb::parse(&zipped)).contains("zipped program tree"));
        let two = RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"ADir", 0, None, crate::builder::encode_adir(&[]))
            .resource(b"ADir", 1, None, crate::builder::encode_adir(&[]))
            .build();
        assert!(reason(Llb::parse(&two)).contains("more than one"));
        assert!(matches!(Llb::parse(b"RSRC"), Err(RsrcError::TooSmall(4))));
    }

    fn with_directory(directory: Vec<u8>) -> Vec<u8> {
        RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"ADir", 0, None, directory)
            .resource(b"UCRF", 1, Some("a.vi"), vec![1, 2, 3])
            .build()
    }

    #[test]
    fn a_directory_of_another_shape_is_refused() {
        let good = crate::builder::encode_adir(&[(1, FourCc::new(b"LVIN"), 0, b"a.vi")]);
        assert_eq!(
            Llb::parse(&with_directory(good.clone()))
                .unwrap()
                .members()
                .len(),
            1
        );

        let mut magic = good.clone();
        magic[4..8].copy_from_slice(b"ADIR");
        assert!(reason(Llb::parse(&with_directory(magic))).contains("`adir`"));

        let mut version = good.clone();
        version[11] = 3;
        assert!(reason(Llb::parse(&with_directory(version))).contains("version 3"));

        let mut trailing = good.clone();
        trailing.extend_from_slice(&[0; 4]);
        assert!(reason(Llb::parse(&with_directory(trailing))).contains("follow the last entry"));

        let mut padding = good.clone();
        *padding.last_mut().unwrap() = 1;
        assert!(reason(Llb::parse(&with_directory(padding))).contains("padding"));

        let mut other_id = good.clone();
        other_id[DIRECTORY_HEADER + 3] = 9;
        assert!(reason(Llb::parse(&with_directory(other_id))).contains("no content resource"));

        // Two entries that name one content resource.
        let twice = crate::builder::encode_adir(&[
            (1, FourCc::new(b"LVIN"), 0, b"a.vi"),
            (1, FourCc::new(b"LVIN"), 0, b"b.vi"),
        ]);
        assert!(reason(Llb::parse(&with_directory(twice))).contains("no content resource"));

        for cut in 0..good.len() {
            assert!(
                Llb::parse(&with_directory(good[..cut].to_vec())).is_err(),
                "a directory cut at {cut} was accepted"
            );
        }
    }

    #[test]
    fn two_content_resources_with_one_id_are_refused() {
        let bytes = RsrcBuilder::new(file_type::LIBRARY)
            .resource(
                b"ADir",
                0,
                None,
                crate::builder::encode_adir(&[(1, FourCc::new(b"LVIN"), 0, b"a.vi")]),
            )
            .resource(b"UCRF", 1, None, vec![1])
            .resource(b"ZCRF", 1, None, vec![2])
            .build();
        assert!(reason(Llb::parse(&bytes)).contains("two content resources"));
    }

    #[test]
    fn content_without_an_entry_is_counted() {
        let bytes = RsrcBuilder::new(file_type::LIBRARY)
            .resource(b"ADir", 0, None, crate::builder::encode_adir(&[]))
            .resource(b"UCRF", 5, Some("ghost.vi"), vec![1])
            .build();
        let llb = Llb::parse(&bytes).unwrap();
        assert_eq!(llb.members().len(), 0);
        assert_eq!(llb.unlisted(), 1);
    }

    #[test]
    fn a_member_count_the_directory_has_no_room_for_is_refused() {
        let mut directory = crate::builder::encode_adir(&[]);
        directory[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        let started = Instant::now();
        assert!(matches!(
            Llb::parse(&with_directory(directory)),
            Err(RsrcError::OutOfBounds { what: "ADir", .. })
        ));
        assert!(started.elapsed().as_secs() < 2);
    }

    fn zcrf(declared: u32, payload: &[u8]) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(declared.to_be_bytes().to_vec(), Compression::best());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap()
    }

    fn with_zcrf(content: Vec<u8>) -> Vec<u8> {
        RsrcBuilder::new(file_type::LIBRARY)
            .resource(
                b"ADir",
                0,
                None,
                crate::builder::encode_adir(&[(1, FourCc::new(b"LVIN"), 0, b"a.vi")]),
            )
            .resource(b"ZCRF", 1, Some("a.vi"), content)
            .build()
    }

    fn read_only_member(bytes: &[u8]) -> Result<Vec<u8>> {
        let llb = Llb::parse(bytes)?;
        llb.read(&llb.members()[0])
    }

    #[test]
    fn a_declared_size_of_gigabytes_is_refused_quickly() {
        let started = Instant::now();
        for declared in [u32::MAX, 0x8000_0000, 0x4000_0000] {
            let bytes = with_zcrf(zcrf(declared, &[0; 64]));
            let llb = Llb::parse(&bytes).unwrap();
            assert_eq!(llb.members()[0].size, u64::from(declared));
            assert!(reason(llb.read(&llb.members()[0])).contains("over the limit"));
        }
        assert!(started.elapsed().as_secs() < 2);
    }

    #[test]
    fn a_declared_size_the_stream_cannot_hold_is_refused_without_inflating() {
        // Under the limit, but 100 MiB from a stream of a few bytes.
        let bytes = with_zcrf(zcrf(100 * 1024 * 1024, &[0; 64]));
        assert!(reason(read_only_member(&bytes)).contains("can hold"));
    }

    #[test]
    fn a_stream_that_inflates_beyond_what_it_declares_is_cut_off() {
        // A bomb: 32 MiB of zeros in about 32 KiB, declared as 100 bytes
        // and then as its true size plus and minus one.
        let big = vec![0u8; 32 * 1024 * 1024];
        let started = Instant::now();
        let bytes = with_zcrf(zcrf(100, &big));
        assert!(bytes.len() < 100_000);
        assert!(reason(read_only_member(&bytes)).contains("holds more"));
        assert!(started.elapsed().as_secs() < 5);
        let size = u32::try_from(big.len()).unwrap();
        assert!(read_only_member(&with_zcrf(zcrf(size - 1, &big))).is_err());
        assert!(read_only_member(&with_zcrf(zcrf(size + 1, &big))).is_err());
        assert_eq!(read_only_member(&with_zcrf(zcrf(size, &big))).unwrap(), big);
    }

    #[test]
    fn a_stream_over_the_cap_is_refused_whatever_it_holds() {
        let over = u32::try_from(MAX_MEMBER_BYTES + 1).unwrap();
        // Enough stream bytes that only the cap can refuse it.
        let mut content = over.to_be_bytes().to_vec();
        content.extend(vec![0u8; 200_000]);
        assert!(reason(read_only_member(&with_zcrf(content))).contains("over the limit"));
    }

    #[test]
    fn damaged_streams_are_errors() {
        let good = zcrf(6, b"abcdef");
        assert_eq!(
            read_only_member(&with_zcrf(good.clone())).unwrap(),
            b"abcdef"
        );
        assert!(matches!(
            read_only_member(&with_zcrf(b"\x00\x00\x00\x04nope".to_vec())),
            Err(RsrcError::Decompress(_))
        ));
        assert!(read_only_member(&with_zcrf(vec![0, 0])).is_err());
        assert!(read_only_member(&with_zcrf(Vec::new())).is_err());
        for cut in 4..good.len() {
            assert!(read_only_member(&with_zcrf(good[..cut].to_vec())).is_err());
        }
    }

    #[test]
    fn the_legacy_compression_is_listed_and_not_decoded() {
        let bytes = RsrcBuilder::new(file_type::LIBRARY)
            .resource(
                b"ADir",
                0,
                None,
                crate::builder::encode_adir(&[(1, FourCc::new(b"LVIN"), 0, b"old.vi")]),
            )
            .resource(b"CPRF", 1, Some("old.vi"), vec![0, 0, 0, 8, 0xff, 1, 2])
            .build();
        let llb = Llb::parse(&bytes).unwrap();
        let member = &llb.members()[0];
        assert_eq!((member.storage, member.size), (Storage::Legacy, 8));
        assert!(reason(llb.read(member)).contains("not supported"));
    }
}
