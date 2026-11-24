//! Reading the two files of a diff from disk.

use std::io::ErrorKind as IoErrorKind;
use std::path::Path;

use buzzard_rsrc::RsrcError;

use super::{diff, DiffOptions, ViDiff};
use crate::failure::{ErrorKind, Failure};
use crate::{archive, printable, Inspection};

/// One side of the comparison: the parsed file, or `None` with an optional
/// note when it does not exist or may not be opened. Anything else that
/// goes wrong (not an RSRC file, a truncated file) is an error.
type Side = (Option<Inspection>, Option<String>);

/// Whether `path` names the null device, which git passes for the side of
/// an added or deleted file: `/dev/null`, or `nul` on Windows.
pub fn is_null_device(path: &Path) -> bool {
    path == Path::new("/dev/null")
        || (cfg!(windows)
            && path
                .to_str()
                .is_some_and(|path| path.eq_ignore_ascii_case("nul")))
}

fn load(path: &Path, same_bytes: bool) -> Result<Side, Failure> {
    if is_null_device(path) {
        return Ok((None, None));
    }
    // A member that its archive does not hold counts as a file that is not
    // on disk, so the other side was added or deleted.
    if let Some((archive, name)) = archive::split(path) {
        return match archive::read_member(&archive, &name) {
            Ok(Some(bytes)) => match Inspection::from_bytes(&bytes) {
                Ok(inspection) => Ok((Some(inspection), None)),
                Err(err) => Err(Failure::unreadable(path, err)),
            },
            Ok(None) => Ok((None, None)),
            Err(err) => Err(Failure::unreadable(path, err)),
        };
    }
    // Two archives are not compared member by member. Because their members
    // were not compared, they are reported as the same only when their bytes
    // are the same. An archive without a directory (one built into an
    // application) is a resource file like any other.
    if !same_bytes && archive::is_archive(path) && path.is_file() {
        match archive::list(path) {
            Ok(None) => {}
            Ok(Some(_)) => {
                let shown = printable::path(path);
                return Err(Failure::unsupported(
                    path,
                    format!(
                        "an archive is not compared as a whole; name one of its members on \
                         each side, as in `{shown}/<member name>` (buzzard inspect lists them)"
                    ),
                ));
            }
            Err(err) => return Err(Failure::unreadable(path, err)),
        }
    }
    let bytes = match buzzard_rsrc::read_file(path) {
        Ok(bytes) => bytes,
        Err(RsrcError::Io(err)) if err.kind() == IoErrorKind::NotFound => return Ok((None, None)),
        Err(RsrcError::Io(err)) if err.kind() == IoErrorKind::PermissionDenied => {
            return Ok((
                None,
                Some(format!("{} cannot be read: {err}", printable::path(path))),
            ));
        }
        Err(err) => return Err(Failure::unreadable(path, err)),
    };
    match Inspection::from_bytes(&bytes) {
        Ok(inspection) => Ok((Some(inspection), None)),
        Err(err) => Err(Failure::unreadable(path, err)),
    }
}

/// Compare the files at `old` and `new`, as `buzzard diff` does.
///
/// Either path may be a member path. A file that does not exist, a member
/// that its archive does not hold, and the null device count as empty, so
/// everything in the other file is reported as added or removed. A file
/// that exists and may not be opened counts as empty too, with a note in
/// [`ViDiff::notes`]. Two `.llb` archives are not compared member by
/// member: they are the same when their bytes are the same, and an error
/// otherwise.
///
/// # Errors
///
/// Returns a [`Failure`] of kind [`ErrorKind::UnreadableFile`] when a
/// file cannot be read or does not parse, of kind
/// [`ErrorKind::Unsupported`] for an archive that would have to be
/// compared as a whole, and of kind [`ErrorKind::PathNotFound`] when
/// neither side can be read. The message is the line that `buzzard diff`
/// prints, with each path on one line ([`printable::path`]).
pub fn compare(old: &Path, new: &Path, options: DiffOptions) -> Result<ViDiff, Failure> {
    let is_archive = |path: &Path| archive::is_archive(path) && path.is_file();
    let same_bytes = is_archive(old)
        && is_archive(new)
        && matches!(
            (buzzard_rsrc::read_file(old), buzzard_rsrc::read_file(new)),
            (Ok(old), Ok(new)) if old == new
        );
    let ((old_file, old_note), (new_file, new_note)) =
        (load(old, same_bytes)?, load(new, same_bytes)?);
    if old_file.is_none() && new_file.is_none() {
        return Err(Failure::new(
            ErrorKind::PathNotFound,
            format!(
                "neither {} nor {} can be read",
                printable::path(old),
                printable::path(new)
            ),
        ));
    }
    let mut result = diff(old_file.as_ref(), new_file.as_ref(), options);
    result.notes.extend(old_note.into_iter().chain(new_note));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use buzzard_rsrc::builder::{LlbBuilder, RsrcBuilder};

    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buzzard-diff-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn vi(flags: u32) -> Vec<u8> {
        RsrcBuilder::default()
            .save_record(0x2300_8000, &[0, flags])
            .build()
    }

    #[test]
    fn null_device_is_recognised_on_every_platform() {
        assert!(is_null_device(Path::new("/dev/null")));
        // `nul` is the null device on Windows only. Elsewhere it is an
        // ordinary file name.
        assert_eq!(is_null_device(Path::new("nul")), cfg!(windows));
        assert_eq!(is_null_device(Path::new("NUL")), cfg!(windows));
        assert!(!is_null_device(Path::new("null.vi")));
        assert!(!is_null_device(Path::new("nul.vi")));
        assert!(load(Path::new("/dev/null"), false).unwrap().0.is_none());
    }

    #[test]
    fn files_and_members_are_compared_and_a_missing_side_is_empty() {
        let dir = sandbox("compare");
        let (a, b) = (dir.join("a.vi"), dir.join("b.vi"));
        std::fs::write(&a, vi(0x400)).unwrap();
        std::fs::write(&b, vi(0)).unwrap();
        let options = DiffOptions::default();
        assert!(compare(&a, &a, options).unwrap().identical);
        assert!(!compare(&a, &b, options).unwrap().identical);
        // A file that is not there, and the null device, are empty.
        let absent = dir.join("absent.vi");
        let added = compare(&absent, &a, options).unwrap();
        assert!(
            added.notes[0].starts_with("old file is missing"),
            "{:?}",
            added.notes
        );
        let removed = compare(&a, Path::new("/dev/null"), options).unwrap();
        assert!(
            removed.notes[0].starts_with("new file is missing"),
            "{:?}",
            removed.notes
        );

        let archive_path = dir.join("lib.llb");
        std::fs::write(
            &archive_path,
            LlbBuilder::default().member("A.vi", &vi(0x400)).build(),
        )
        .unwrap();
        let member = archive::member_path(&archive_path, "A.vi");
        assert!(compare(&member, &a, options).unwrap().identical);
        assert!(!compare(&member, &b, options).unwrap().identical);
        let no_member = archive::member_path(&archive_path, "Absent.vi");
        let added = compare(&no_member, &a, options).unwrap();
        assert!(
            added.notes[0].starts_with("old file is missing"),
            "{:?}",
            added.notes
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn each_error_has_its_kind_and_the_line_the_command_prints() {
        let dir = sandbox("errors");
        let a = dir.join("a.vi");
        std::fs::write(&a, vi(0)).unwrap();
        let options = DiffOptions::default();

        let (x, y) = (dir.join("x.vi"), dir.join("y.vi"));
        let neither = compare(&x, &y, options).unwrap_err();
        assert_eq!(neither.kind, ErrorKind::PathNotFound);
        assert_eq!(
            neither.message,
            format!("neither {} nor {} can be read", x.display(), y.display())
        );

        let garbage = dir.join("garbage.vi");
        std::fs::write(&garbage, b"not a resource file at all").unwrap();
        let unreadable = compare(&garbage, &a, options).unwrap_err();
        assert_eq!(unreadable.kind, ErrorKind::UnreadableFile);
        assert!(unreadable
            .message
            .starts_with(&format!("{}: ", garbage.display())));
        assert_eq!(
            unreadable.path.as_deref(),
            Some(garbage.to_string_lossy().as_ref())
        );

        let (one, two) = (dir.join("one.llb"), dir.join("two.llb"));
        std::fs::write(&one, LlbBuilder::default().member("A.vi", &vi(0)).build()).unwrap();
        std::fs::write(&two, LlbBuilder::default().member("B.vi", &vi(0)).build()).unwrap();
        // The same bytes are the same archive.
        assert!(compare(&one, &one, options).unwrap().identical);
        let whole = compare(&one, &two, options).unwrap_err();
        assert_eq!(whole.kind, ErrorKind::Unsupported);
        assert_eq!(
            whole.message,
            format!(
                "{0}: an archive is not compared as a whole; name one of its members on each \
                 side, as in `{0}/<member name>` (buzzard inspect lists them)",
                one.display()
            )
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_name_cannot_add_a_line_to_an_error_or_a_note() {
        let forged = Path::new("a.vi\nerror: forged");
        let failure = compare(forged, Path::new("b\u{1b}[2J.vi"), DiffOptions::default());
        let failure = failure.unwrap_err();
        assert_eq!(failure.message.lines().count(), 1);
        assert!(!failure.message.contains('\u{1b}'));
    }
}
