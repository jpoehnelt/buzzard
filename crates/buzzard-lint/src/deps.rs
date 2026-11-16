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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use buzzard_rsrc::{LvPath, PathKind};
use serde::Serialize;

use crate::graph::{candidate_paths, resolve, DependencyGraph};
use crate::rules::member_is_missing;
use crate::workspace::{normalize, FileState, Workspace};

/// How many directories below a symbolic root name a package.
const PACKAGE_DEPTH: usize = 2;
/// The directory below `<vilib>` under which add-ons install one level
/// deeper, as `addons/<vendor>/<product>`.
const ADDONS: &str = "addons";
/// The depth of a package under [`ADDONS`].
const ADDONS_DEPTH: usize = 3;

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
    /// `-` and `.`; when `dir` is not an existing directory; or when the
    /// root was already given another directory.
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
            return Err(format!(
                "the directory given for root {bare} does not exist: {}",
                dir.display()
            ));
        }
        let key = bare.to_lowercase();
        match self.map.get(&key) {
            Some(known) if known != dir => Err(format!(
                "root {bare} was given two directories: {} and {}",
                known.display(),
                dir.display()
            )),
            _ => {
                self.map.insert(key, dir.to_path_buf());
                Ok(())
            }
        }
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
///
/// Letter case is kept as stored. An empty path gives `.`.
pub fn package(components: &[String]) -> String {
    let Some((file, directories)) = components.split_last() else {
        return ".".to_owned();
    };
    if directories.is_empty() {
        return shown(file).to_owned();
    }
    let depth = if directories[0].eq_ignore_ascii_case(ADDONS) {
        ADDONS_DEPTH
    } else {
        PACKAGE_DEPTH
    };
    let mut name: Vec<&str> = Vec::new();
    for directory in directories.iter().take(depth) {
        name.push(shown(directory));
        if has_extension(directory, &["llb", "lvlibp", "lvlib", "lvclass"]) {
            return name.join("/");
        }
    }
    if name.len() < depth && is_file_container(file) {
        name.push(shown(file));
    }
    name.join("/")
}

/// An empty component is a step to the parent directory.
fn shown(component: &str) -> &str {
    if component.is_empty() {
        ".."
    } else {
        component
    }
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

/// Whether there is a file at `path`. A path that leads into a file (a
/// VI inside an `.llb` library, the private data control inside an
/// `.lvclass` file) counts as present when that file exists: the file is
/// not opened to look for the item.
fn on_disk(path: &Path) -> bool {
    path.exists() || path.ancestors().skip(1).any(Path::is_file)
}

/// Look for a stored path below the directory given for its root.
fn status_under(directory: Option<&Path>, components: &[String]) -> Status {
    let Some(directory) = directory else {
        return Status::Unchecked;
    };
    // Only plain names are joined to the directory: a parent step, a
    // separator or a drive in a stored component would leave it.
    let plain = |component: &String| {
        let mut parts = Path::new(component).components();
        matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none()
            && !component.contains(['/', '\0'])
    };
    if components.is_empty() || !components.iter().all(plain) {
        return Status::Unchecked;
    }
    let mut path = directory.to_path_buf();
    path.extend(components);
    if on_disk(&path) {
        Status::Found
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

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Referrers by target, as they are collected.
#[derive(Default)]
struct Collected {
    /// Root name and the components below it.
    symbolic: BTreeMap<(String, Vec<String>), BTreeSet<String>>,
    absolute: BTreeMap<String, BTreeSet<String>>,
    missing: BTreeMap<String, BTreeSet<String>>,
}

impl Collected {
    /// File a link that needs no look at the workspace: one under a
    /// symbolic root or a machine-absolute one. Returns whether it was
    /// either.
    fn stored_link(&mut self, referrer: &str, link: &LvPath) -> bool {
        if let Some(root) = link.symbolic_root().and_then(root_name) {
            self.symbolic
                .entry((root, link.components[1..].to_vec()))
                .or_default()
                .insert(referrer.to_owned());
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
    fn unresolved(&mut self, referrer: &Path, link: &LvPath, name: Option<&str>) {
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
            .any(|path| on_disk(path))
        {
            return;
        }
        let target = text_of(&location);
        self.missing
            .entry(target)
            .or_default()
            .insert(text_of(referrer));
    }

    /// File a member URL that starts with `/`: an installation path such
    /// as `/<vilib>/...` or a path of one machine.
    fn rooted_url(&mut self, referrer: &str, url: &str) {
        let components: Vec<String> = url
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        match components.first().and_then(|first| root_name(first)) {
            Some(root) => {
                self.symbolic
                    .entry((root, components[1..].to_vec()))
                    .or_default()
                    .insert(referrer.to_owned());
            }
            None if !components.is_empty() => {
                self.absolute
                    .entry(url.to_owned())
                    .or_default()
                    .insert(referrer.to_owned());
            }
            None => {}
        }
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

/// Collect the external dependencies of every file of `workspace`.
/// `graph` must have been built from the same workspace.
pub fn scan(workspace: &Workspace, graph: &DependencyGraph, roots: &Roots) -> DepsReport {
    let mut collected = Collected::default();
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
                        collected.unresolved(path, &link.path, link.name());
                    }
                }
            }
            FileState::Project(project) => {
                for member in &project.members {
                    if member.url.starts_with('/') {
                        collected.rooted_url(&referrer, &member.url);
                    } else if !member.dependency && member_is_missing(workspace, member) {
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
                        collected.unresolved(path, &parent.link, Some(&parent.name));
                    }
                }
            }
        }
    }

    let mut by_root: BTreeMap<String, BTreeMap<String, Vec<Target>>> = BTreeMap::new();
    for ((root, components), referrers) in collected.symbolic {
        let target = Target {
            path: stored(&root, &components),
            status: status_under(roots.get(&root), &components),
            referrers: referrers.into_iter().collect(),
        };
        by_root
            .entry(root)
            .or_default()
            .entry(package(&components))
            .or_default()
            .push(target);
    }
    let roots = by_root
        .into_iter()
        .map(|(root, packages)| Root {
            directory: roots.get(&root).map(text_of),
            root,
            packages: packages
                .into_iter()
                .map(|(name, mut targets)| {
                    targets.sort_by(|a, b| a.path.cmp(&b.path));
                    Package { name, targets }
                })
                .collect(),
        })
        .collect();
    DepsReport {
        files: workspace.files().len(),
        roots,
        absolute: links(collected.absolute),
        missing: links(collected.missing),
        unparsed,
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
    if !report.unparsed.is_empty() {
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
    let _ = writeln!(out, "{}", summary_line(report));
    out
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
        let checked = checked(root);
        let mut sentence = checked.chars();
        let first = sentence.next().map(|c| c.to_ascii_uppercase());
        let rest: String = sentence.collect();
        let _ = writeln!(
            out,
            "{}{}.",
            first.map(String::from).unwrap_or_default(),
            rest.replace(['|', '`'], " ")
        );
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
    if !report.unparsed.is_empty() {
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
    out
}

#[cfg(test)]
mod tests;
