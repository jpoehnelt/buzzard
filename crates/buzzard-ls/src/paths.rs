//! Which paths on disk belong to the analyzed workspace.
//!
//! The initial scan uses [`buzzard_lint::workspace::discover`]. Incremental
//! updates must agree with it exactly, or the results would depend on the
//! history of file events. [`Roots::presence`] answers for a single path
//! what `discover` answers for a whole tree. The tests at the end of this
//! file check that the two give the same answer.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use buzzard_lint::workspace::{is_analyzable, normalize};

/// How a fresh scan would treat one path right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    /// A file that a fresh scan would analyze.
    File,
    /// A directory that a fresh scan would walk into.
    Dir,
    /// Anything a fresh scan would not see: missing, hidden, reached
    /// through a symbolic link, outside every root, or not analyzable.
    Absent,
}

#[derive(Debug, Clone)]
struct Root {
    /// The root exactly as the client spelled it, lexically normalized.
    /// Every workspace key and every published URI starts with it.
    given: PathBuf,
    /// The same directory with symbolic links resolved, when that differs.
    /// Some file watchers (`FSEvents` on macOS) report paths in this form.
    canonical: Option<PathBuf>,
}

/// The workspace roots, in the client's spelling and in canonical form.
#[derive(Debug, Clone, Default)]
pub(crate) struct Roots(Vec<Root>);

impl Roots {
    pub(crate) fn new(roots: &[PathBuf]) -> Self {
        Self(
            roots
                .iter()
                .map(|root| {
                    let given = normalize(root);
                    let canonical = fs::canonicalize(&given)
                        .ok()
                        .filter(|canonical| *canonical != given);
                    Root { given, canonical }
                })
                .collect(),
        )
    }

    /// Rewrite `path` into the spelling the client used for its root, so
    /// that one file never gets two names. Returns `None` if the path is
    /// outside every root.
    pub(crate) fn localize(&self, path: &Path) -> Option<PathBuf> {
        let path = normalize(path);
        if self.0.iter().any(|root| path.starts_with(&root.given)) {
            return Some(path);
        }
        self.0.iter().find_map(|root| {
            let rest = path.strip_prefix(root.canonical.as_ref()?).ok()?;
            let mut local = root.given.clone();
            local.extend(rest.components());
            Some(local)
        })
    }

    /// How a fresh scan would treat `path`, which must be in the client's
    /// spelling (see [`Roots::localize`]).
    pub(crate) fn presence(&self, path: &Path) -> Presence {
        self.0
            .iter()
            .map(|root| presence_in(&root.given, path))
            .find(|presence| *presence != Presence::Absent)
            .unwrap_or(Presence::Absent)
    }
}

/// Discovery skips every entry below a root whose name starts with a dot.
fn is_hidden(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with('.'))
}

fn presence_in(root: &Path, path: &Path) -> Presence {
    let Ok(rest) = path.strip_prefix(root) else {
        return Presence::Absent;
    };
    let mut components = rest.components().peekable();
    if components.peek().is_none() {
        // The path is the root itself. For a root, links are followed, and
        // a root that is a file is analyzed whatever its name.
        return match fs::metadata(root) {
            Ok(meta) if meta.is_file() => Presence::File,
            Ok(meta) if meta.is_dir() => Presence::Dir,
            _ => Presence::Absent,
        };
    }
    let mut current = root.to_path_buf();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Presence::Absent;
        };
        if is_hidden(name) {
            return Presence::Absent;
        }
        current.push(name);
        // `symlink_metadata` does not follow links, so a link is neither a
        // file nor a directory here.
        let Ok(meta) = fs::symlink_metadata(&current) else {
            return Presence::Absent;
        };
        let last = components.peek().is_none();
        if meta.is_dir() {
            if last {
                return Presence::Dir;
            }
        } else if last && meta.is_file() && is_analyzable(&current) {
            return Presence::File;
        } else {
            return Presence::Absent;
        }
    }
    Presence::Absent
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzzard_lint::workspace::discover;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buzzard-ls-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    /// Every path below `dir`, following links, so that the predicate is
    /// also offered the paths that discovery must reject.
    fn everything(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            out.push(path.clone());
            if path.is_dir() {
                everything(&path, depth - 1, out);
            }
        }
    }

    #[test]
    fn agrees_with_discovery_on_every_path() {
        let base = temp("paths-agree");
        let root = base.join("root");
        touch(&root.join("a.vi"));
        touch(&root.join("notes.txt"));
        touch(&root.join(".secret.vi"));
        touch(&root.join("sub/b.ctl"));
        touch(&root.join("sub/deep/App.lvproj"));
        touch(&root.join("sub/.cache/c.vi"));
        touch(&root.join(".git/objects/d.vi"));
        touch(&root.join(".git/nested/e.lvlib"));
        touch(&base.join("outside/o.vi"));
        // A directory named like a file, and a second root that is a file.
        fs::create_dir_all(root.join("folder.vi")).unwrap();
        let lone = base.join("lone.txt");
        touch(&lone);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(root.join("a.vi"), root.join("link.vi")).unwrap();
            symlink(base.join("outside"), root.join("linked")).unwrap();
            symlink(root.join("sub"), root.join("sub-again")).unwrap();
        }
        // A root inside a hidden directory of another root is still a root.
        let nested = root.join(".git/nested");
        let given = vec![root.clone(), lone.clone(), nested.clone()];
        let roots = Roots::new(&given);

        let mut offered = vec![root.clone(), lone.clone(), base.join("outside/o.vi")];
        everything(&root, 6, &mut offered);
        let mut admitted: Vec<PathBuf> = offered
            .into_iter()
            .filter(|path| roots.presence(path) == Presence::File)
            .collect();
        admitted.sort();
        admitted.dedup();
        let discovered = discover(&given);
        assert_eq!(admitted, discovered);
        // Guard against the test passing on an empty tree.
        assert!(discovered.contains(&root.join("sub/deep/App.lvproj")));
        assert!(discovered.contains(&nested.join("e.lvlib")));
        assert!(discovered.contains(&lone));
        assert!(!discovered.contains(&root.join(".git/objects/d.vi")));

        assert_eq!(roots.presence(&root.join("sub")), Presence::Dir);
        assert_eq!(roots.presence(&root.join("folder.vi")), Presence::Dir);
        assert_eq!(roots.presence(&root.join(".git")), Presence::Absent);
        assert_eq!(roots.presence(&root.join("gone")), Presence::Absent);
        assert_eq!(roots.presence(&base.join("outside")), Presence::Absent);
        #[cfg(unix)]
        {
            assert_eq!(roots.presence(&root.join("linked")), Presence::Absent);
            assert_eq!(roots.presence(&root.join("link.vi")), Presence::Absent);
            let through_link = root.join("sub-again/b.ctl");
            assert_eq!(roots.presence(&through_link), Presence::Absent);
        }
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn localize_rejects_paths_outside_the_roots() {
        let base = temp("paths-outside");
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        let roots = Roots::new(std::slice::from_ref(&root));
        assert_eq!(roots.localize(&base.join("other/a.vi")), None);
        assert_eq!(roots.localize(&base.join("rootling/a.vi")), None);
        assert_eq!(
            roots.localize(&root.join("x/../a.vi")),
            Some(root.join("a.vi"))
        );
        assert_eq!(roots.localize(&root), Some(root.clone()));
        assert_eq!(Roots::default().localize(&root.join("a.vi")), None);
        fs::remove_dir_all(&base).unwrap();
    }

    /// A watcher that reports resolved paths (`FSEvents`) must not give a
    /// file a second name. The file keeps the client's spelling of the root.
    #[cfg(unix)]
    #[test]
    fn localize_maps_canonical_paths_back_to_the_given_root() {
        let base = temp("paths-canonical");
        fs::create_dir_all(base.join("actual/ws/sub")).unwrap();
        std::os::unix::fs::symlink(base.join("actual"), base.join("link")).unwrap();
        let given = base.join("link/ws");
        let canonical = base.join("actual/ws");
        let roots = Roots::new(std::slice::from_ref(&given));

        assert_eq!(
            roots.localize(&canonical.join("sub/a.vi")),
            Some(given.join("sub/a.vi"))
        );
        assert_eq!(roots.localize(&canonical), Some(given.clone()));
        assert_eq!(
            roots.localize(&given.join("sub/a.vi")),
            Some(given.join("sub/a.vi"))
        );
        // Files below a root reached through a link are still discovered.
        touch(&canonical.join("sub/a.vi"));
        assert_eq!(roots.presence(&given.join("sub/a.vi")), Presence::File);
        fs::remove_dir_all(&base).unwrap();
    }
}
