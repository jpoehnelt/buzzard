//! Checks on a path that a caller gave as an argument.
//!
//! Two rules apply to a path argument, as distinct from a path that a
//! scan of a directory finds:
//!
//! - [`check`] refuses a path that holds an ASCII control character. No
//!   caller types one on purpose, and a path with one is more often the
//!   end of a quoting mistake or of text that was pasted from a file.
//!   Characters such as `?`, `#`, and `%` are legal in a file name and
//!   are never decoded, so they are accepted.
//! - [`Confinement`] holds the directories that a server may read, and
//!   refuses a path that leads anywhere else once links and `..` are
//!   resolved.
//!
//! A file with a control character in its name can still be analyzed: a
//! scan of its directory finds it, and every report prints its name on
//! one line ([`crate::printable`]).

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR_STR};

use crate::failure::{ErrorKind, Failure};
use crate::printable;

/// Check a path argument before it is used.
///
/// # Errors
///
/// Returns a [`Failure`] of kind [`ErrorKind::BadArgument`] when the path
/// is empty or holds an ASCII control character (a NUL, a line break, a
/// tab, an escape).
pub fn check(path: &Path) -> Result<(), Failure> {
    if path.as_os_str().is_empty() {
        return Err(Failure::bad_argument("a path argument is empty"));
    }
    // A byte below 0x80 of the encoded form is always that ASCII
    // character, on every platform.
    let control = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(u8::is_ascii_control);
    if control {
        return Err(Failure::bad_argument(format!(
            "the path holds a control character: {}",
            printable::path(path)
        ))
        .with_path(path));
    }
    Ok(())
}

/// [`check`] for every path of a list, then
/// [`archive::exists`](crate::archive::exists) for every path.
///
/// # Errors
///
/// Returns the [`Failure`] of the first path that [`check`] refuses, or
/// else one of kind [`ErrorKind::PathNotFound`] for the first path that
/// names nothing.
pub fn existing(paths: &[PathBuf]) -> Result<(), Failure> {
    for path in paths {
        check(path)?;
    }
    match paths.iter().find(|path| !crate::archive::exists(path)) {
        Some(missing) => Err(Failure::path_not_found(missing)),
        None => Ok(()),
    }
}

/// The directories that path arguments must stay inside.
///
/// The roots are resolved once, when the value is made
/// ([`std::fs::canonicalize`]). [`Confinement::admit`] resolves each
/// argument the same way and compares whole components, so `/data/a` does
/// not admit `/data/ab`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confinement {
    roots: Vec<PathBuf>,
}

impl Confinement {
    /// Confine path arguments to `roots`.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] of kind [`ErrorKind::BadArgument`] when the
    /// list is empty, or when a root fails [`check`], does not exist, or
    /// is not a directory.
    pub fn new(roots: &[PathBuf]) -> Result<Self, Failure> {
        if roots.is_empty() {
            return Err(Failure::bad_argument("no root directory was given"));
        }
        let mut resolved = Vec::with_capacity(roots.len());
        for root in roots {
            check(root)?;
            let canonical = std::fs::canonicalize(root)
                .ok()
                .filter(|canonical| canonical.is_dir())
                .ok_or_else(|| {
                    Failure::bad_argument(format!(
                        "the root is not an existing directory: {}",
                        printable::path(root)
                    ))
                    .with_path(root)
                })?;
            resolved.push(canonical);
        }
        resolved.sort();
        resolved.dedup();
        Ok(Self { roots: resolved })
    }

    /// The roots, resolved, sorted, and without repeats.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Check that `path` is inside a root and return where it leads: the
    /// path with every link and `..` resolved.
    ///
    /// The path may name something that does not exist, such as a member
    /// of an archive (`lib/util.llb/Init.vi`) or a file that a diff counts
    /// as empty. The part of the path that exists is resolved, and the
    /// rest is appended to it. The rest may hold names only: a `..` after
    /// a name that does not exist is refused, because nothing says where
    /// it would lead once that name exists.
    ///
    /// A relative path is resolved against the current directory of the
    /// process. A caller that resolves it against another directory joins
    /// the two first and passes the path that it is going to open.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] of kind [`ErrorKind::BadArgument`] when
    /// [`check`] refuses the path, and one of kind
    /// [`ErrorKind::OutsideRoot`] when the path leads outside every root
    /// or cannot be resolved.
    pub fn admit(&self, path: &Path) -> Result<PathBuf, Failure> {
        check(path)?;
        let outside = || {
            Failure::new(
                ErrorKind::OutsideRoot,
                format!(
                    "the path is outside the directories this server may read: {}",
                    printable::path(path)
                ),
            )
            .with_path(path)
        };
        let resolved = resolve(path).ok_or_else(outside)?;
        if self.roots.iter().any(|root| resolved.starts_with(root)) {
            Ok(resolved)
        } else {
            Err(outside())
        }
    }
}

/// Whether a name that does not exist yet may be appended to a resolved
/// path as text. On Windows a colon is refused, as
/// [`archive::is_addressable`](crate::archive::is_addressable) refuses
/// it: `C:x` names another drive there, and `a.vi:s` a stream of `a.vi`.
fn appendable(name: &OsStr) -> bool {
    !(cfg!(windows) && name.as_encoded_bytes().contains(&b':'))
}

/// `path` with its existing part resolved by the file system and the
/// names after it appended. `None` when the path cannot be resolved to
/// one place.
fn resolve(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    // The names that follow the existing part, last first.
    let mut rest: Vec<&OsStr> = Vec::new();
    let base = loop {
        let probe = if existing.as_os_str().is_empty() {
            Path::new(".")
        } else {
            existing
        };
        if let Ok(resolved) = std::fs::canonicalize(probe) {
            break resolved;
        }
        // A link that leads nowhere exists as a link. Its target cannot be
        // resolved, so nothing says whether the target is inside a root.
        if std::fs::symlink_metadata(probe).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return None;
        }
        match existing.components().next_back()? {
            Component::Normal(name) if appendable(name) => rest.push(name),
            Component::CurDir => {}
            _ => return None,
        }
        existing = existing.parent()?;
    };
    // The names are appended as text. `Path::join` would let a name that
    // is rooted or starts with a drive replace the resolved part.
    let mut out = OsString::from(base);
    for name in rest.iter().rev() {
        out.push(MAIN_SEPARATOR_STR);
        out.push(name);
    }
    Some(PathBuf::from(out))
}

#[cfg(test)]
mod tests {
    use buzzard_rsrc::builder::LlbBuilder;

    use super::*;

    /// A new directory for one test, with every link resolved so that
    /// paths under it compare equal to what `admit` returns.
    fn sandbox(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("buzzard-patharg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_control_character_is_refused_and_legal_characters_are_not() {
        for bad in [
            "a\nb.vi",
            "a\tb.vi",
            "\u{1b}[2Jx.vi",
            "a\0.vi",
            "a\r.vi",
            "a\u{7f}.vi",
        ] {
            let failure = check(Path::new(bad)).unwrap_err();
            assert_eq!(failure.kind, ErrorKind::BadArgument, "{bad:?}");
            assert_eq!(failure.message.lines().count(), 1, "{bad:?}");
            assert!(!failure.message.contains('\u{1b}'), "{bad:?}");
            assert_eq!(failure.path.as_deref(), Some(bad));
        }
        assert_eq!(
            check(Path::new("")).unwrap_err().kind,
            ErrorKind::BadArgument
        );
        // Legal file-name characters that buzzard never decodes, and
        // letters outside ASCII.
        for good in [
            "50%20done?.vi",
            "a#1.vi",
            "%2e%2e/x.vi",
            "..",
            "lib/util.llb/Init.vi",
            "テスト.vi",
            "caf\u{e9} \u{85}.vi",
            "a b.vi",
        ] {
            assert_eq!(check(Path::new(good)), Ok(()), "{good}");
        }
    }

    #[test]
    fn existing_reports_the_first_path_that_names_nothing() {
        let dir = sandbox("existing");
        let present = dir.join("a.vi");
        std::fs::write(&present, b"x").unwrap();
        let missing = dir.join("b.vi");
        assert_eq!(existing(&[present.clone(), dir.clone()]), Ok(()));
        let failure = existing(&[present.clone(), missing.clone()]).unwrap_err();
        assert_eq!(failure, Failure::path_not_found(&missing));
        // A control character is reported before anything is looked up.
        let failure = existing(&[missing, PathBuf::from("a\nb")]).unwrap_err();
        assert_eq!(failure.kind, ErrorKind::BadArgument);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_root_must_be_an_existing_directory() {
        let dir = sandbox("roots");
        let file = dir.join("a.vi");
        std::fs::write(&file, b"x").unwrap();
        for bad in [file, dir.join("missing"), PathBuf::from("a\nb")] {
            let failure = Confinement::new(std::slice::from_ref(&bad)).unwrap_err();
            assert_eq!(failure.kind, ErrorKind::BadArgument, "{}", bad.display());
        }
        assert_eq!(
            Confinement::new(&[]).unwrap_err().kind,
            ErrorKind::BadArgument
        );
        // Two spellings of one directory are one root.
        let twice = Confinement::new(&[dir.clone(), dir.join(".")]).unwrap();
        assert_eq!(twice.roots(), std::slice::from_ref(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_path_is_admitted_only_inside_a_root() {
        let dir = sandbox("admit");
        let root = dir.join("data");
        let sibling = dir.join("database");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(root.join("sub").join("a.vi"), b"x").unwrap();
        std::fs::write(sibling.join("b.vi"), b"x").unwrap();
        std::fs::write(dir.join("secret.vi"), b"x").unwrap();
        let confined = Confinement::new(std::slice::from_ref(&root)).unwrap();

        let inside = root.join("sub").join("a.vi");
        assert_eq!(confined.admit(&inside), Ok(inside.clone()));
        assert_eq!(confined.admit(&root), Ok(root.clone()));
        // `..` that stays inside is resolved.
        assert_eq!(
            confined.admit(&root.join("sub").join("..").join("sub").join("a.vi")),
            Ok(inside)
        );
        // A file that does not exist yet, and one below a directory that
        // does not exist.
        let new = root.join("new.vi");
        assert_eq!(confined.admit(&new), Ok(new));
        let deep = root.join("x").join("y.vi");
        assert_eq!(confined.admit(&deep), Ok(deep));

        let mut outsides = vec![
            dir.join("secret.vi"),
            root.join("..").join("secret.vi"),
            root.join("sub").join("..").join("..").join("secret.vi"),
            // A root is matched by whole components.
            sibling.join("b.vi"),
            sibling.clone(),
            dir.clone(),
            // `..` after a name that does not exist.
            root.join("missing").join("..").join("..").join("secret.vi"),
            dir.join("missing.vi"),
        ];
        // Where this path leads is inside the root once `missing` exists
        // as a directory, and nothing says that it will. On Windows the
        // system removes `missing\..` from a path before it looks at the
        // disk, and `Path::join` removes it from the resolved root that
        // this test builds on, so the path names `sub\a.vi` there, which
        // is inside.
        if cfg!(unix) {
            outsides.push(root.join("missing").join("..").join("sub").join("a.vi"));
        }
        for outside in outsides {
            let failure = confined.admit(&outside).unwrap_err();
            assert_eq!(
                failure.kind,
                ErrorKind::OutsideRoot,
                "{}",
                outside.display()
            );
            assert_eq!(
                failure.path.as_deref(),
                Some(outside.to_string_lossy().as_ref())
            );
        }
        assert_eq!(
            confined.admit(&root.join("a\nb.vi")).unwrap_err().kind,
            ErrorKind::BadArgument
        );
        assert_eq!(
            confined.admit(Path::new("")).unwrap_err().kind,
            ErrorKind::BadArgument
        );

        // Either of two roots admits a path.
        let both = Confinement::new(&[root, sibling.clone()]).unwrap();
        assert!(both.admit(&sibling.join("b.vi")).is_ok());
        assert!(both.admit(&dir.join("secret.vi")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_relative_path_is_resolved_against_the_current_directory() {
        let here = std::fs::canonicalize(".").unwrap();
        let confined = Confinement::new(&[PathBuf::from(".")]).unwrap();
        assert_eq!(confined.roots(), std::slice::from_ref(&here));
        assert_eq!(
            confined.admit(Path::new("Cargo.toml")),
            Ok(here.join("Cargo.toml"))
        );
        assert_eq!(confined.admit(Path::new(".")), Ok(here.clone()));
        assert_eq!(
            confined.admit(Path::new("no-such-file.vi")),
            Ok(here.join("no-such-file.vi"))
        );
        assert_eq!(
            confined.admit(Path::new("..")).unwrap_err().kind,
            ErrorKind::OutsideRoot
        );
        assert_eq!(
            confined.admit(Path::new("../x.vi")).unwrap_err().kind,
            ErrorKind::OutsideRoot
        );
    }

    #[test]
    fn a_member_path_is_admitted_with_its_archive() {
        let dir = sandbox("member");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let archive = LlbBuilder::default().member("Init.vi", b"x").build();
        std::fs::write(root.join("util.llb"), &archive).unwrap();
        std::fs::write(dir.join("other.llb"), &archive).unwrap();
        let confined = Confinement::new(std::slice::from_ref(&root)).unwrap();

        let member = crate::archive::member_path(&root.join("util.llb"), "Init.vi");
        assert!(crate::archive::split(&member).is_some());
        assert_eq!(confined.admit(&member), Ok(member));
        // A member that the archive does not hold is still inside.
        let absent = crate::archive::member_path(&root.join("util.llb"), "Absent.vi");
        assert_eq!(confined.admit(&absent), Ok(absent));
        // A member of an archive outside the root, by either spelling.
        let outside = crate::archive::member_path(&dir.join("other.llb"), "Init.vi");
        assert_eq!(
            confined.admit(&outside).unwrap_err().kind,
            ErrorKind::OutsideRoot
        );
        let around = root.join("..").join("other.llb").join("Init.vi");
        assert_eq!(
            confined.admit(&around).unwrap_err().kind,
            ErrorKind::OutsideRoot
        );
        // `..` after the archive would be resolved by name only.
        let through = root
            .join("util.llb")
            .join("..")
            .join("..")
            .join("other.llb");
        assert_eq!(
            confined.admit(&through).unwrap_err().kind,
            ErrorKind::OutsideRoot
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_followed_before_the_root_is_compared() {
        use std::os::unix::fs::symlink;

        let dir = sandbox("links");
        let root = dir.join("root");
        let outside = dir.join("outside");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.vi"), b"x").unwrap();
        std::fs::write(root.join("sub").join("a.vi"), b"x").unwrap();
        let archive = LlbBuilder::default().member("Init.vi", b"x").build();
        std::fs::write(outside.join("util.llb"), archive).unwrap();
        symlink(&outside, root.join("out")).unwrap();
        symlink(outside.join("secret.vi"), root.join("secret.vi")).unwrap();
        symlink(outside.join("util.llb"), root.join("util.llb")).unwrap();
        symlink(root.join("sub"), root.join("in")).unwrap();
        symlink(dir.join("nowhere"), root.join("dangling")).unwrap();
        let confined = Confinement::new(std::slice::from_ref(&root)).unwrap();

        for escaping in [
            root.join("out"),
            root.join("out").join("secret.vi"),
            root.join("secret.vi"),
            // A file that does not exist behind a link that leaves.
            root.join("out").join("new.vi"),
            // A member of an archive behind a link.
            root.join("util.llb").join("Init.vi"),
            // A link whose target does not exist, and a name after it.
            root.join("dangling"),
            root.join("dangling").join("x.vi"),
        ] {
            assert_eq!(
                confined.admit(&escaping).unwrap_err().kind,
                ErrorKind::OutsideRoot,
                "{}",
                escaping.display()
            );
        }
        // The file system resolves the link first, so this `..` is the
        // parent of `outside`, not `root`. Where the path leads is inside
        // the root, and that is what counts.
        let around = root.join("out").join("..").join("root").join("sub");
        assert_eq!(
            confined.admit(&around.join("a.vi")),
            Ok(root.join("sub").join("a.vi"))
        );
        // A link that stays inside is admitted, as the place it leads to.
        assert_eq!(
            confined.admit(&root.join("in").join("a.vi")),
            Ok(root.join("sub").join("a.vi"))
        );
        // A root that is a link is the directory it leads to.
        let linked = Confinement::new(&[root.join("out")]).unwrap();
        assert_eq!(linked.roots(), std::slice::from_ref(&outside));
        assert!(linked.admit(&outside.join("secret.vi")).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_name_with_a_colon_is_not_appended_on_windows() {
        let dir = sandbox("colon");
        let confined = Confinement::new(std::slice::from_ref(&dir)).unwrap();
        // `C:x.vi` after a name that does not exist would name another
        // drive once it is joined.
        let path = PathBuf::from(format!("{}\\missing.llb\\C:x.vi", dir.display()));
        assert_eq!(
            confined.admit(&path).unwrap_err().kind,
            ErrorKind::OutsideRoot
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
