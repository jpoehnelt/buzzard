//! The external dependencies of a workspace: what must be installed, or
//! fixed, before its files can be opened.
//!
//! Three kinds of stored link are reported:
//!
//! * Links under a symbolic root such as `<vilib>` or `<userlib>`. They
//!   name files of the installation, grouped here by root and by package
//!   (see [`package`]).
//! * Machine-absolute links, which only work on a machine laid out like
//!   the one that saved the file.
//! * Relative links at whose location there is no file, neither in the
//!   workspace nor on disk. A link that does not leave the file holding it
//!   (a bare name such as `user32.dll`) has no location and is not listed.
//!
//! Links come from the link info of binary files and from the member URLs
//! and parent class links of project XML files. Whether a relative link
//! has a target is decided by the functions the dependency graph and rule
//! `BZ006` use, so the report never disagrees with them.
//!
//! Nothing is reported as present or absent without a look at the disk: a
//! target under a root that was given no directory is [`Status::Unchecked`].
//!
//! Stored paths that differ only in letter case, or that print the same,
//! are one target: links are saved on file systems that ignore case, and
//! the same file is stored in several spellings.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use buzzard_rsrc::{LvPath, PathKind};
use serde::Serialize;

use crate::archive::{is_archive, Archives};
use crate::graph::{candidate_paths, code_page_readings, fold, resolve, DependencyGraph};
use crate::rules::member_is_missing;
use crate::workspace::{normalize, FileState, Workspace};

/// How many directories below a symbolic root name a package.
const PACKAGE_DEPTH: usize = 2;
/// The directory below `<vilib>` under which add-ons install one level
/// deeper, as `addons/<vendor>/<product>`.
const ADDONS: &str = "addons";
/// The depth of a package under [`ADDONS`].
const ADDONS_DEPTH: usize = 3;
/// The package of a stored path that has no plain directory or file name
/// to name one: a bare root, or a path that starts with a parent step.
pub const IRREGULAR: &str = "(irregular path)";

/// Whether a target under a symbolic root is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The root was given a directory and the target is in it.
    Found,
    /// The root was given a directory and the target is not in it.
    Missing,
    /// Nobody looked: the root was given no directory, or the stored path
    /// has a component that is not a plain file name.
    Unchecked,
}

impl Status {
    /// The word printed in every format: `found`, `missing` or `unchecked`.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Found => "found",
            Status::Missing => "missing",
            Status::Unchecked => "unchecked",
        }
    }
}

/// The directories given for symbolic roots, by lowercase root name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    map: BTreeMap<String, PathBuf>,
}

impl Roots {
    /// Split a `NAME=DIR` specification, e.g. `vilib=/opt/labview/vi.lib`.
    /// The name is returned as written; [`Roots::add`] checks it.
    ///
    /// # Errors
    ///
    /// When the text has no `=`, or nothing on one side of it.
    pub fn split(spec: &str) -> Result<(&str, &str), String> {
        match spec.split_once('=') {
            Some((name, dir)) if !name.trim().is_empty() && !dir.is_empty() => Ok((name, dir)),
            _ => Err(format!(
                "root `{spec}` is not of the form NAME=DIR, for example vilib=/opt/labview/vi.lib"
            )),
        }
    }

    /// Map the symbolic root `name` to `dir`. The name is matched without
    /// regard to case and may be written with or without angle brackets.
    ///
    /// # Errors
    ///
    /// When the name is empty or holds anything but letters, digits, `_`,
    /// `-` and `.`; when `dir` does not exist or is not a directory; or
    /// when the root was already given another directory. The same
    /// directory in another spelling (`src` and `./src`) is not another.
    pub fn add(&mut self, name: &str, dir: &Path) -> Result<(), String> {
        let trimmed = name.trim();
        let bare = trimmed
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or(trimmed);
        let plain = !bare.is_empty()
            && bare
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !plain {
            return Err(format!(
                "`{name}` is not the name of a symbolic root; write it like vilib or userlib"
            ));
        }
        if !dir.is_dir() {
            let problem = if dir.exists() {
                "is not a directory"
            } else {
                "does not exist"
            };
            return Err(format!(
                "the directory given for root {bare} {problem}: {}",
                dir.display()
            ));
        }
        let key = bare.to_lowercase();
        match self.map.get(&key) {
            Some(known) if !same_directory(known, dir) => Err(format!(
                "root {bare} was given two directories: {} and {}",
                known.display(),
                dir.display()
            )),
            // The first spelling of a directory is the one reported.
            Some(_) => Ok(()),
            None => {
                self.map.insert(key, dir.to_path_buf());
                Ok(())
            }
        }
    }

    /// The names of the roots that were given a directory, in lowercase
    /// and sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }

    fn get(&self, root: &str) -> Option<&Path> {
        self.map.get(root).map(PathBuf::as_path)
    }
}

/// The report of [`scan`]. Every list is sorted, so the same files give
/// the same report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DepsReport {
    /// Number of files that were scanned, parsed or not.
    pub files: usize,
    /// The symbolic roots that links start with, sorted by name.
    pub roots: Vec<Root>,
    /// Machine-absolute links, sorted by target.
    pub absolute: Vec<Link>,
    /// Relative links without a file at their location, sorted by target.
    pub missing: Vec<Link>,
    /// Files that could not be read or parsed. Their links are unknown.
    pub unparsed: Vec<Unparsed>,
    /// The roots that were given a directory although no link is under
    /// them, in lowercase and sorted: the directory was not used.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unused_roots: Vec<String>,
}

/// Everything linked under one symbolic root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Root {
    /// The root's name in lowercase without angle brackets, e.g. `vilib`.
    pub root: String,
    /// The directory the root was given, when it was given one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// The packages under the root, sorted by name.
    pub packages: Vec<Package>,
}

/// The targets that share a package directory; see [`package`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Package {
    /// The package directory below the root, e.g. `Utility/error.llb`.
    pub name: String,
    /// The files linked in the package, sorted by path.
    pub targets: Vec<Target>,
}

/// One file under a symbolic root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    /// The stored path, e.g. `<vilib>/Utility/error.llb/Clear Errors.vi`.
    /// Of stored paths that differ only in letter case, the one that
    /// sorts first.
    pub path: String,
    /// Whether the file is in the directory given for its root.
    pub status: Status,
    /// The workspace files that link to it, sorted, each listed once.
    pub referrers: Vec<String>,
}

/// A link target outside any symbolic root, with the files that use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    /// For a machine-absolute link, the stored path. For a missing
    /// relative link, the location it resolves to.
    pub target: String,
    /// The workspace files that hold the link, sorted, each listed once.
    pub referrers: Vec<String>,
}

/// A file whose links are unknown because it could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unparsed {
    /// The file.
    pub path: String,
    /// Why it could not be read or parsed.
    pub message: String,
}

/// Counts over a report, as printed in its last line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// Packages under all roots.
    pub packages: usize,
    /// Targets under all roots.
    pub targets: usize,
    /// Targets with [`Status::Found`].
    pub found: usize,
    /// Targets with [`Status::Missing`].
    pub missing: usize,
    /// Targets with [`Status::Unchecked`].
    pub unchecked: usize,
}

impl DepsReport {
    /// Counts of the packages and targets under the symbolic roots.
    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();
        for root in &self.roots {
            totals.add(root);
        }
        totals
    }

    /// Rewrite every path of the file system in the report: referrers,
    /// the targets of missing relative links, unparsed files and root
    /// directories. Stored paths (targets under a root, machine-absolute
    /// targets) stay as they are. The lists are sorted again afterwards.
    pub fn map_paths(&mut self, show: impl Fn(&str) -> String) {
        let rewrite = |paths: &mut Vec<String>| {
            for path in paths.iter_mut() {
                *path = show(path);
            }
            paths.sort();
            paths.dedup();
        };
        for root in &mut self.roots {
            root.directory = root.directory.as_deref().map(&show);
            for package in &mut root.packages {
                for target in &mut package.targets {
                    rewrite(&mut target.referrers);
                }
            }
        }
        for link in &mut self.absolute {
            rewrite(&mut link.referrers);
        }
        for link in &mut self.missing {
            link.target = show(&link.target);
            rewrite(&mut link.referrers);
        }
        self.missing.sort_by(|a, b| a.target.cmp(&b.target));
        for file in &mut self.unparsed {
            file.path = show(&file.path);
        }
        self.unparsed.sort_by(|a, b| a.path.cmp(&b.path));
    }
}

impl Totals {
    fn add(&mut self, root: &Root) {
        self.packages += root.packages.len();
        for target in root.packages.iter().flat_map(|p| &p.targets) {
            self.targets += 1;
            match target.status {
                Status::Found => self.found += 1,
                Status::Missing => self.missing += 1,
                Status::Unchecked => self.unchecked += 1,
            }
        }
    }
}

/// The package a path below a symbolic root belongs to: the directory a
/// person would install or copy as one unit. `components` is the stored
/// path without its root, ending in the file name.
///
/// The rule reports directories and knows no product names:
///
/// 1. The package is the first two directories of the path: `Utility/x.vi`
///    is in `Utility` and `JKI/VIPM API/sub/x.vi` in `JKI/VIPM API`.
/// 2. Below a first directory called `addons` it is the first three,
///    because add-ons install as `addons/<vendor>/<product>`.
/// 3. A library ends the name early: a component ending in `.llb` or
///    `.lvlibp` (a file that holds other files), or a directory ending in
///    `.lvlib` or `.lvclass`. `Utility/error.llb/x.vi` and a link to
///    `Utility/error.llb` itself are both in `Utility/error.llb`.
/// 4. A file directly below the root is its own package.
/// 5. The name stops in front of a component that is not a plain name: a
///    parent step (stored as an empty component, or `.` or `..` in a
///    member URL) or text with a path separator. A path with no plain name
///    to start from, such as the bare root, is in [`IRREGULAR`].
///
/// Letter case is kept as stored; [`scan`] groups packages whose names
/// differ only in case.
pub fn package(components: &[String]) -> String {
    let Some((file, directories)) = components.split_last() else {
        return IRREGULAR.to_owned();
    };
    let depth = if directories
        .first()
        .is_some_and(|first| first.eq_ignore_ascii_case(ADDONS))
    {
        ADDONS_DEPTH
    } else {
        PACKAGE_DEPTH
    };
    let mut name: Vec<&str> = Vec::new();
    let mut cut = false;
    for directory in directories.iter().take(depth) {
        if !is_plain(directory) {
            cut = true;
            break;
        }
        name.push(directory);
        if has_extension(directory, &["llb", "lvlibp", "lvlib", "lvclass"]) {
            return name.join("/");
        }
    }
    let file_ends_it = directories.is_empty() || is_file_container(file);
    if !cut && name.len() < depth && file_ends_it && is_plain(file) {
        name.push(file);
    }
    if name.is_empty() {
        IRREGULAR.to_owned()
    } else {
        name.join("/")
    }
}

/// True for a component that is the name of one file or directory: not
/// empty, not a step (`.`, `..`) and without a path separator.
fn is_plain(component: &str) -> bool {
    !matches!(component, "" | "." | "..") && !component.contains(['/', '\\', '\0'])
}

fn has_extension(name: &str, extensions: &[&str]) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        extensions
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// True for a name of a single file that holds other files.
fn is_file_container(name: &str) -> bool {
    has_extension(name, &["llb", "lvlibp"])
}

/// Whether there may be a file at `path`. A path that leads into an
/// `.llb` archive is present when the archive holds that member, and also
/// when the archive cannot be read: it is then not known to be absent. A
/// path that leads into any other file (the private data control inside
/// an `.lvclass` file) counts as present when that file exists: the file
/// is not opened to look for the item.
fn on_disk(archives: &Archives, path: &Path) -> bool {
    archives.present(path, true) != Some(false)
}

/// Whether `components` name something below `directory`; `None` when
/// they lead into an `.llb` archive that cannot be read. A name that is
/// not there as written is looked for without regard to letter case:
/// links are saved on file systems that ignore case, and are stored in
/// more than one spelling. A path that leads into an archive is there
/// when the archive holds that member; one that leads into any other
/// file counts when that file exists.
fn below(archives: &Archives, directory: &Path, components: &[String]) -> Option<bool> {
    let Some((first, rest)) = components.split_first() else {
        return Some(true);
    };
    let reaches = |path: &Path| {
        if path.is_file() {
            if rest.is_empty() || !is_archive(path) {
                Some(true)
            } else {
                let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
                archives.contains(path, &rest)
            }
        } else if path.is_dir() {
            below(archives, path, rest)
        } else {
            Some(false)
        }
    };
    let mut unknown = false;
    match reaches(&directory.join(first)) {
        Some(true) => return Some(true),
        Some(false) => {}
        None => unknown = true,
    }
    let wanted = fold(first);
    let mut others: Vec<_> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| {
            let name = name.to_string_lossy();
            name != first.as_str() && fold(&name) == wanted
        })
        .collect();
    others.sort();
    for name in others {
        match reaches(&directory.join(name)) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    (!unknown).then_some(false)
}

/// Look for a stored path below the directory given for its root.
/// `components` is the path without its root.
///
/// The path is looked for as stored and then, like a relative link, under
/// each legacy code page its text may be the bytes of.
fn status_under(
    archives: &Archives,
    directory: Option<&Path>,
    kind: PathKind,
    components: &[String],
) -> Status {
    let Some(directory) = directory else {
        return Status::Unchecked;
    };
    // Only plain names are joined to the directory: a parent step, a
    // separator or a drive in a stored component would leave it. A path
    // that is not stored as absolute is not one of the installation.
    let plain = |component: &String| {
        let mut parts = Path::new(component).components();
        is_plain(component)
            && matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none()
    };
    let checkable = |components: &[String]| !components.is_empty() && components.iter().all(plain);
    if kind != PathKind::Absolute || !checkable(components) {
        return Status::Unchecked;
    }
    let mut looks = vec![below(archives, directory, components)];
    if looks[0] != Some(true) {
        looks.extend(
            code_page_readings(components)
                .iter()
                .filter(|reading| checkable(reading))
                .map(|reading| below(archives, directory, reading)),
        );
    }
    if looks.contains(&Some(true)) {
        Status::Found
    } else if looks.contains(&None) {
        // An archive on the way cannot be read, so nobody has looked.
        Status::Unchecked
    } else {
        Status::Missing
    }
}

/// The lowercase name of a symbolic root component: `<VILIB>` is `vilib`.
fn root_name(component: &str) -> Option<String> {
    let inner = component.strip_prefix('<')?.strip_suffix('>')?;
    (!inner.is_empty()).then(|| inner.to_lowercase())
}

/// The path of a target as stored, with its root in lowercase.
fn stored(root: &str, components: &[String]) -> String {
    let mut all = vec![format!("<{root}>")];
    all.extend(components.iter().cloned());
    LvPath {
        kind: PathKind::Absolute,
        components: all,
    }
    .to_string()
}

/// Whether two existing directories are one, however they are spelled.
fn same_directory(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A target under a symbolic root, as it is collected.
#[derive(Default)]
struct Stored {
    /// Every spelling the target is stored in: the components below the
    /// root and the kind of the path.
    spellings: BTreeSet<(Vec<String>, PathKind)>,
    referrers: BTreeSet<String>,
}

/// Referrers by target, as they are collected.
#[derive(Default)]
struct Collected {
    /// By root name and the stored path without its letter case, so that
    /// spellings of one file, and paths that print the same, are one
    /// target.
    symbolic: BTreeMap<(String, String), Stored>,
    absolute: BTreeMap<String, BTreeSet<String>>,
    missing: BTreeMap<String, BTreeSet<String>>,
}

impl Collected {
    /// File a link that needs no look at the workspace: one under a
    /// symbolic root or a machine-absolute one. Returns whether it was
    /// either.
    fn stored_link(&mut self, referrer: &str, link: &LvPath) -> bool {
        if let Some(root) = link.symbolic_root().and_then(root_name) {
            let components = &link.components[1..];
            let key = fold(&stored(&root, components));
            let target = self.symbolic.entry((root, key)).or_default();
            target.spellings.insert((components.to_vec(), link.kind));
            target.referrers.insert(referrer.to_owned());
            true
        } else if link.is_machine_absolute() {
            self.absolute
                .entry(link.to_string())
                .or_default()
                .insert(referrer.to_owned());
            true
        } else {
            false
        }
    }

    /// File a relative link that names no workspace file, unless one of
    /// the places it may name is on disk.
    fn unresolved(
        &mut self,
        archives: &Archives,
        referrer: &Path,
        link: &LvPath,
        name: Option<&str>,
    ) {
        if link.kind != PathKind::Relative || link.is_empty() {
            return;
        }
        let Some(location) = resolve(referrer, link) else {
            return;
        };
        // A link without a parent step stays inside the file that holds
        // it. It is a name, not a location: an item of the file itself, a
        // generated instance, or a shared library the system finds by
        // name. There is no place on disk to look for it.
        if location.starts_with(normalize(referrer)) {
            return;
        }
        if candidate_paths(referrer, link, name)
            .iter()
            .any(|path| on_disk(archives, path))
        {
            return;
        }
        let target = text_of(&location);
        self.missing
            .entry(target)
            .or_default()
            .insert(text_of(referrer));
    }

    /// File a member URL that the project parser does not resolve to a
    /// path beside the project file, when it has one of the shapes of a
    /// stored path: `/<vilib>/...` or `/C/Users/...` as LabVIEW writes
    /// them, or `<vilib>/...`, `C:/...`, `C:\...` and `\\server\...` as a
    /// hand-edited file may hold them. It is filed like the same path
    /// stored in a binary file, so that both give one target.
    fn unresolved_url(&mut self, referrer: &str, url: &str) {
        let mut components: Vec<String> = url
            .split(['/', '\\'])
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        let Some(first) = components.first_mut() else {
            return;
        };
        let rooted = url.starts_with('/');
        let unc = url.starts_with("\\\\");
        let drive = !rooted && first.len() == 2 && first.ends_with(':');
        if drive {
            first.pop();
        }
        if !(rooted || unc || drive || root_name(first).is_some()) {
            return;
        }
        let kind = if unc {
            PathKind::Unc
        } else {
            PathKind::Absolute
        };
        self.stored_link(referrer, &LvPath { kind, components });
    }
}

fn links(collected: BTreeMap<String, BTreeSet<String>>) -> Vec<Link> {
    collected
        .into_iter()
        .map(|(target, referrers)| Link {
            target,
            referrers: referrers.into_iter().collect(),
        })
        .collect()
}

/// The packages of one root by name without its letter case: the
/// spellings of the name and the targets.
type Packages = BTreeMap<String, (BTreeSet<String>, Vec<Target>)>;

/// Group the collected targets by root and package, with their statuses.
/// Also returns the roots that were given a directory and have no target.
fn grouped(
    archives: &Archives,
    symbolic: BTreeMap<(String, String), Stored>,
    roots: &Roots,
) -> (Vec<Root>, Vec<String>) {
    let mut by_root: BTreeMap<String, Packages> = BTreeMap::new();
    for ((root, _), target) in symbolic {
        let directory = roots.get(&root);
        let status = target
            .spellings
            .iter()
            .map(|(components, kind)| status_under(archives, directory, *kind, components))
            .min()
            .unwrap_or(Status::Unchecked);
        let Some((components, _)) = target.spellings.first() else {
            continue;
        };
        let name = package(components);
        let (names, targets) = by_root
            .entry(root.clone())
            .or_default()
            .entry(fold(&name))
            .or_default();
        names.insert(name);
        targets.push(Target {
            path: stored(&root, components),
            status,
            referrers: target.referrers.into_iter().collect(),
        });
    }
    let unused_roots = roots
        .names()
        .filter(|name| !by_root.contains_key(*name))
        .map(str::to_owned)
        .collect();
    let listed = by_root
        .into_iter()
        .map(|(root, packages)| {
            let mut packages: Vec<Package> = packages
                .into_values()
                .map(|(names, mut targets)| {
                    targets.sort_by(|a, b| a.path.cmp(&b.path));
                    Package {
                        name: names.into_iter().next().unwrap_or_default(),
                        targets,
                    }
                })
                .collect();
            packages.sort_by(|a, b| a.name.cmp(&b.name));
            Root {
                directory: roots.get(&root).map(text_of),
                root,
                packages,
            }
        })
        .collect();
    (listed, unused_roots)
}

/// Collect the external dependencies of every file of `workspace`.
/// `graph` must have been built from the same workspace.
pub fn scan(workspace: &Workspace, graph: &DependencyGraph, roots: &Roots) -> DepsReport {
    let mut collected = Collected::default();
    let archives = Archives::default();
    let mut unparsed = Vec::new();
    for (path, state) in workspace.files() {
        let referrer = text_of(path);
        match state {
            FileState::Failed(message) => unparsed.push(Unparsed {
                path: referrer,
                message: message.clone(),
            }),
            FileState::Parsed(analysis) => {
                for link in &analysis.metadata.links {
                    if !collected.stored_link(&referrer, &link.path)
                        && graph.link_target(path, &link.path, link.name()).is_none()
                    {
                        collected.unresolved(&archives, path, &link.path, link.name());
                    }
                }
            }
            FileState::Project(project) => {
                for member in &project.members {
                    if member.path.is_none() {
                        collected.unresolved_url(&referrer, &member.url);
                    } else if !member.dependency && member_is_missing(workspace, &archives, member)
                    {
                        // The members rule `BZ006` reports, by its test.
                        let target = member.path.as_deref().map_or_else(String::new, text_of);
                        collected
                            .missing
                            .entry(target)
                            .or_default()
                            .insert(referrer.clone());
                    }
                }
                if let Some(parent) = &project.parent_class {
                    if !collected.stored_link(&referrer, &parent.link)
                        && graph.parent_target(path, parent).is_none()
                    {
                        collected.unresolved(&archives, path, &parent.link, Some(&parent.name));
                    }
                }
            }
            // An archive holds no links; its members are files of their own.
            FileState::Archive(_) => {}
        }
    }

    let (roots, unused_roots) = grouped(&archives, collected.symbolic, roots);
    DepsReport {
        files: workspace.files().len(),
        roots,
        absolute: links(collected.absolute),
        missing: links(collected.missing),
        unparsed,
        unused_roots,
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Keep file-supplied text on one line.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// File-supplied text as a Markdown code span inside a table cell.
fn code(text: &str) -> String {
    let inner: String = text
        .chars()
        .map(|c| match c {
            '|' | '`' => ' ',
            c if c.is_control() => ' ',
            other => other,
        })
        .collect();
    format!("`{inner}`")
}

/// What was looked up under a root, in words.
fn checked(root: &Root) -> String {
    let mut totals = Totals::default();
    totals.add(root);
    match &root.directory {
        None => format!(
            "not checked: no directory was given for root {}",
            clean(&root.root)
        ),
        Some(directory) => {
            let mut text = format!(
                "checked in {}: {} found, {} missing",
                clean(directory),
                totals.found,
                totals.missing
            );
            if totals.unchecked > 0 {
                let _ = write!(text, ", {} unchecked", totals.unchecked);
            }
            text
        }
    }
}

fn summary_line(report: &DepsReport) -> String {
    let totals = report.totals();
    format!(
        "Scanned {}: {} in {} under {} ({} found, {} missing, {} unchecked), {}, {}, {}.",
        plural(report.files, "file", "files"),
        plural(totals.targets, "external file", "external files"),
        plural(totals.packages, "package", "packages"),
        plural(report.roots.len(), "symbolic root", "symbolic roots"),
        totals.found,
        totals.missing,
        totals.unchecked,
        plural(
            report.absolute.len(),
            "machine-absolute link",
            "machine-absolute links"
        ),
        plural(
            report.missing.len(),
            "missing relative link",
            "missing relative links"
        ),
        plural(report.unparsed.len(), "unparsed file", "unparsed files"),
    )
}

/// The plain-text report.
pub fn to_text(report: &DepsReport) -> String {
    let mut out = String::new();
    for root in &report.roots {
        let _ = writeln!(out, "<{}> ({})", clean(&root.root), checked(root));
        for package in &root.packages {
            let _ = writeln!(
                out,
                "  {} ({})",
                clean(&package.name),
                plural(package.targets.len(), "file", "files")
            );
            for target in &package.targets {
                let _ = writeln!(
                    out,
                    "    {:<9} {}",
                    target.status.as_str(),
                    clean(&target.path)
                );
                for referrer in &target.referrers {
                    let _ = writeln!(out, "      used by {}", clean(referrer));
                }
            }
        }
        out.push('\n');
    }
    let mut section = |title: &str, links: &[Link]| {
        if links.is_empty() {
            return;
        }
        let _ = writeln!(out, "{title} ({})", links.len());
        for link in links {
            let _ = writeln!(out, "  {}", clean(&link.target));
            for referrer in &link.referrers {
                let _ = writeln!(out, "    used by {}", clean(referrer));
            }
        }
        out.push('\n');
    };
    section("Machine-absolute links", &report.absolute);
    section("Missing relative links", &report.missing);
    text_unparsed(&mut out, report);
    text_tail(&mut out, report);
    out
}

fn text_unparsed(out: &mut String, report: &DepsReport) {
    if report.unparsed.is_empty() {
        return;
    }
    let _ = writeln!(out, "Unparsed files ({})", report.unparsed.len());
    for file in &report.unparsed {
        let _ = writeln!(
            out,
            "  {}: cannot analyze file: {}",
            clean(&file.path),
            clean(&file.message)
        );
    }
    out.push('\n');
}

/// What no link is under, in words.
fn unused(root: &str) -> String {
    format!(
        "{}: no link is under <{}>, so its directory was not used",
        clean(root),
        clean(root)
    )
}

/// The end of both text reports: unparsed files, unused roots, summary.
fn text_tail(out: &mut String, report: &DepsReport) {
    if !report.unused_roots.is_empty() {
        let _ = writeln!(out, "Unused roots ({})", report.unused_roots.len());
        for root in &report.unused_roots {
            let _ = writeln!(out, "  {}", unused(root));
        }
        out.push('\n');
    }
    let _ = writeln!(out, "{}", summary_line(report));
}

/// The report as Markdown.
pub fn to_markdown(report: &DepsReport) -> String {
    let used_by = |referrers: &[String]| {
        referrers
            .iter()
            .map(|r| code(r))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::from("# External dependencies\n\n");
    let _ = writeln!(out, "{}", summary_line(report));
    for root in &report.roots {
        let _ = writeln!(out, "\n## {}\n", code(&format!("<{}>", root.root)));
        let _ = writeln!(out, "{}", checked_sentence(root));
        let _ = writeln!(
            out,
            "\n| Package | File | Status | Used by |\n| --- | --- | --- | --- |"
        );
        for package in &root.packages {
            for target in &package.targets {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} |",
                    code(&package.name),
                    code(&target.path),
                    target.status.as_str(),
                    used_by(&target.referrers)
                );
            }
        }
    }
    let mut section = |title: &str, column: &str, links: &[Link]| {
        if links.is_empty() {
            return;
        }
        let _ = writeln!(out, "\n## {title}\n\n| {column} | Used by |\n| --- | --- |");
        for link in links {
            let _ = writeln!(
                out,
                "| {} | {} |",
                code(&link.target),
                used_by(&link.referrers)
            );
        }
    };
    section("Machine-absolute links", "Stored path", &report.absolute);
    section("Missing relative links", "Location", &report.missing);
    markdown_unparsed(&mut out, report);
    markdown_unused(&mut out, report);
    out
}

fn markdown_unparsed(out: &mut String, report: &DepsReport) {
    if report.unparsed.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\n## Unparsed files\n\n| File | Reason |\n| --- | --- |"
    );
    for file in &report.unparsed {
        let _ = writeln!(
            out,
            "| {} | {} |",
            code(&file.path),
            clean(&file.message).replace(['|', '`'], " ")
        );
    }
}

/// [`checked`] as a sentence of a Markdown paragraph.
fn checked_sentence(root: &Root) -> String {
    let checked = checked(root);
    let mut sentence = checked.chars();
    let first = sentence.next().map(|c| c.to_ascii_uppercase());
    let rest: String = sentence.collect();
    format!(
        "{}{}.",
        first.map(String::from).unwrap_or_default(),
        rest.replace(['|', '`'], " ")
    )
}

fn markdown_unused(out: &mut String, report: &DepsReport) {
    if report.unused_roots.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n## Unused roots\n");
    for root in &report.unused_roots {
        let _ = writeln!(out, "- {}", unused(root).replace(['|', '`'], " "));
    }
}

/// A report without its files: each package with counts, each link with
/// the number of files that use it. Its size follows the number of
/// packages, not the number of links, so it suits a reader with little
/// room. See [`DepsReport::overview`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overview {
    /// Number of files that were scanned, parsed or not.
    pub files: usize,
    /// The symbolic roots that links start with, sorted by name.
    pub roots: Vec<RootOverview>,
    /// Machine-absolute links, sorted by target.
    pub absolute: Vec<LinkOverview>,
    /// Relative links without a file at their location, sorted by target.
    pub missing: Vec<LinkOverview>,
    /// Files that could not be read or parsed.
    pub unparsed: Vec<Unparsed>,
    /// As [`DepsReport::unused_roots`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unused_roots: Vec<String>,
}

/// The packages under one symbolic root, with counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootOverview {
    /// As [`Root::root`].
    pub root: String,
    /// As [`Root::directory`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// The packages under the root, sorted by name.
    pub packages: Vec<PackageOverview>,
}

/// One package, with counts in place of its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageOverview {
    /// As [`Package::name`].
    pub name: String,
    /// Number of files linked in the package.
    pub files: usize,
    /// How many of them are [`Status::Found`].
    pub found: usize,
    /// How many of them are [`Status::Missing`].
    pub missing: usize,
    /// How many of them are [`Status::Unchecked`].
    pub unchecked: usize,
    /// Number of workspace files that link into the package.
    pub referrers: usize,
}

/// One link target, with the number of files that use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkOverview {
    /// As [`Link::target`].
    pub target: String,
    /// Number of workspace files that hold the link.
    pub referrers: usize,
}

impl DepsReport {
    /// The report with counts in place of files and referrers.
    pub fn overview(&self) -> Overview {
        let links = |links: &[Link]| {
            links
                .iter()
                .map(|link| LinkOverview {
                    target: link.target.clone(),
                    referrers: link.referrers.len(),
                })
                .collect()
        };
        let package = |package: &Package| {
            let count = |status: Status| {
                package
                    .targets
                    .iter()
                    .filter(|t| t.status == status)
                    .count()
            };
            let referrers: BTreeSet<&String> =
                package.targets.iter().flat_map(|t| &t.referrers).collect();
            PackageOverview {
                name: package.name.clone(),
                files: package.targets.len(),
                found: count(Status::Found),
                missing: count(Status::Missing),
                unchecked: count(Status::Unchecked),
                referrers: referrers.len(),
            }
        };
        Overview {
            files: self.files,
            roots: self
                .roots
                .iter()
                .map(|root| RootOverview {
                    root: root.root.clone(),
                    directory: root.directory.clone(),
                    packages: root.packages.iter().map(package).collect(),
                })
                .collect(),
            absolute: links(&self.absolute),
            missing: links(&self.missing),
            unparsed: self.unparsed.clone(),
            unused_roots: self.unused_roots.clone(),
        }
    }
}

/// The statuses of a package's files, in words: `2 found, 1 missing`.
fn statuses(package: &PackageOverview) -> String {
    [
        (package.found, Status::Found),
        (package.missing, Status::Missing),
        (package.unchecked, Status::Unchecked),
    ]
    .iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, status)| format!("{count} {}", status.as_str()))
    .collect::<Vec<_>>()
    .join(", ")
}

/// The overview of a report as plain text: one line per package and per
/// link, and the summary line of [`to_text`].
pub fn overview_to_text(report: &DepsReport) -> String {
    let overview = report.overview();
    let mut out = String::new();
    for (root, counted) in report.roots.iter().zip(&overview.roots) {
        let _ = writeln!(out, "<{}> ({})", clean(&root.root), checked(root));
        for package in &counted.packages {
            let _ = writeln!(
                out,
                "  {}: {} ({}), used by {}",
                clean(&package.name),
                plural(package.files, "file", "files"),
                statuses(package),
                plural(package.referrers, "file", "files")
            );
        }
        out.push('\n');
    }
    let mut section = |title: &str, links: &[LinkOverview]| {
        if links.is_empty() {
            return;
        }
        let _ = writeln!(out, "{title} ({})", links.len());
        for link in links {
            let _ = writeln!(
                out,
                "  {} (used by {})",
                clean(&link.target),
                plural(link.referrers, "file", "files")
            );
        }
        out.push('\n');
    };
    section("Machine-absolute links", &overview.absolute);
    section("Missing relative links", &overview.missing);
    text_unparsed(&mut out, report);
    text_tail(&mut out, report);
    out
}

/// The overview of a report as Markdown.
pub fn overview_to_markdown(report: &DepsReport) -> String {
    let overview = report.overview();
    let mut out = String::from("# External dependencies\n\n");
    let _ = writeln!(out, "{}", summary_line(report));
    for (root, counted) in report.roots.iter().zip(&overview.roots) {
        let _ = writeln!(out, "\n## {}\n", code(&format!("<{}>", root.root)));
        let _ = writeln!(out, "{}", checked_sentence(root));
        let _ = writeln!(
            out,
            "\n| Package | Files | Status | Used by |\n| --- | --- | --- | --- |"
        );
        for package in &counted.packages {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                code(&package.name),
                package.files,
                statuses(package),
                plural(package.referrers, "file", "files")
            );
        }
    }
    let mut section = |title: &str, column: &str, links: &[LinkOverview]| {
        if links.is_empty() {
            return;
        }
        let _ = writeln!(out, "\n## {title}\n\n| {column} | Used by |\n| --- | --- |");
        for link in links {
            let _ = writeln!(
                out,
                "| {} | {} |",
                code(&link.target),
                plural(link.referrers, "file", "files")
            );
        }
    };
    section("Machine-absolute links", "Stored path", &overview.absolute);
    section("Missing relative links", "Location", &overview.missing);
    markdown_unparsed(&mut out, report);
    markdown_unused(&mut out, report);
    out
}

#[cfg(test)]
mod tests;
