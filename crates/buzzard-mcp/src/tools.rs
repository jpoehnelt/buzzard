//! The tools: their schemas, argument checking and implementations.
//!
//! Every tool is read-only. A failure (missing file, unreadable file, bad
//! argument) is an `Err(String)`, which the protocol layer reports as a
//! tool result with `isError` set so that the calling model can read the
//! reason and correct itself.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use buzzard_lint::archive;
use buzzard_lint::deps::{self as dependencies, Roots};
use buzzard_lint::project::is_project_file;
use buzzard_lint::verify::report::{self, Verified};
use buzzard_lint::workspace::{discover, normalize};
use buzzard_lint::{
    lint, render, summary, DependencyGraph, Diagnostic, FileState, Inspection, LintConfig,
    ProjectFile, ProjectKind, Severity, Workspace, RULES,
};
use buzzard_rsrc::FileKind;
use serde_json::{json, Map, Value};

pub(crate) const NAMES: [&str; 9] = [
    "buzzard_check",
    "buzzard_verify",
    "buzzard_deps",
    "buzzard_inspect",
    "buzzard_diagram",
    "buzzard_callers",
    "buzzard_callees",
    "buzzard_symbols",
    "buzzard_rules",
];

pub(crate) const INSTRUCTIONS: &str =
    "buzzard reads LabVIEW .vi/.ctl binaries and .lvproj/.lvlib/.lvclass \
files directly from disk, without the LabVIEW IDE. All tools are read-only. Relative paths are \
resolved against the directory the server was started in. Start with buzzard_symbols to find \
files, buzzard_inspect and buzzard_diagram to read one, buzzard_callers and buzzard_callees to \
navigate, buzzard_check to lint, buzzard_verify to check the state machines on block \
diagrams, and buzzard_deps to list what must be installed before the files can be opened.";

/// Longest accepted path or query argument, in bytes.
const MAX_TEXT_BYTES: usize = 4096;
/// Most entries accepted in an array argument.
const MAX_LIST_ITEMS: usize = 256;
const DEFAULT_DIAGNOSTIC_LIMIT: usize = 200;
const MAX_DIAGNOSTIC_LIMIT: usize = 5000;
const DEFAULT_SYMBOL_LIMIT: usize = 50;
const MAX_SYMBOL_LIMIT: usize = 1000;

const CACHE_NOTE: &str = "The parsed workspace is cached in the server process. Every call \
compares the size and modification time of each file with the cached scan and re-parses \
everything when a file was added, removed or changed, so results normally follow the disk. Set \
refresh to true to force a re-parse if a file was rewritten without its size or modification \
time changing.";

/// What a successful tool call returns.
pub(crate) struct Output {
    pub text: String,
    /// Machine-readable form of the same answer; always a JSON object.
    pub structured: Option<Value>,
}

/// Size and modification time of one file; `None` when it cannot be read.
type Stamp = Option<(u64, Option<SystemTime>)>;

#[derive(Debug)]
struct Scan {
    roots: Vec<PathBuf>,
    stamps: Vec<(PathBuf, Stamp)>,
    workspace: Workspace,
    graph: DependencyGraph,
}

/// Session state: the base directory and the most recent workspace scan.
#[derive(Debug)]
pub(crate) struct State {
    base: PathBuf,
    scan: Option<Scan>,
    /// How many times a workspace was parsed from scratch.
    parses: u64,
}

impl State {
    pub(crate) fn new(base: &Path) -> Self {
        Self {
            base: normalize(base),
            scan: None,
            parses: 0,
        }
    }

    /// Drop the cached scan.
    pub(crate) fn forget(&mut self) {
        self.scan = None;
    }

    /// Resolve a path argument against the base directory.
    fn resolve(&self, raw: &str) -> PathBuf {
        normalize(&self.base.join(raw))
    }

    /// Path as reported to the client: relative to the base directory when
    /// inside it, always with forward slashes.
    fn show(&self, path: &Path) -> String {
        let shown = match path.strip_prefix(&self.base) {
            Ok(relative) if relative.as_os_str().is_empty() => Path::new("."),
            Ok(relative) => relative,
            Err(_) => path,
        };
        shown.to_string_lossy().replace('\\', "/")
    }

    /// The scan of `roots`, reused when the files on disk are unchanged.
    /// Hands back the state as well, so that callers can keep using it.
    fn scan(&mut self, mut roots: Vec<PathBuf>, refresh: bool) -> (&Self, &Scan) {
        roots.sort();
        roots.dedup();
        let stamps: Vec<(PathBuf, Stamp)> = discover(&roots)
            .into_iter()
            .map(|path| {
                // A member changes when its archive does.
                let on_disk = archive::split(&path).map_or_else(|| path.clone(), |(file, _)| file);
                let stamp = fs::metadata(on_disk)
                    .ok()
                    .map(|meta| (meta.len(), meta.modified().ok()));
                (path, stamp)
            })
            .collect();
        let fresh = !refresh
            && self
                .scan
                .as_ref()
                .is_some_and(|scan| scan.roots == roots && scan.stamps == stamps);
        if !fresh {
            self.parses += 1;
            let workspace = Workspace::scan(&roots);
            let graph = DependencyGraph::build(&workspace);
            self.scan = Some(Scan {
                roots,
                stamps,
                workspace,
                graph,
            });
        }
        let this = &*self;
        (this, this.scan.as_ref().expect("scan was just stored"))
    }
}

/// Checked access to the arguments object of one call.
struct Args<'a> {
    map: &'a Map<String, Value>,
}

impl<'a> Args<'a> {
    fn new(map: &'a Map<String, Value>, allowed: &[&str]) -> Result<Self, String> {
        if let Some(unknown) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
            let shown: String = unknown.chars().take(64).collect();
            return Err(format!(
                "unknown argument `{shown}`; accepted arguments: {}",
                if allowed.is_empty() {
                    "none".to_owned()
                } else {
                    allowed.join(", ")
                }
            ));
        }
        Ok(Self { map })
    }

    fn text(&self, key: &str) -> Result<&'a str, String> {
        match self.map.get(key) {
            Some(Value::String(text)) => check_text(key, text),
            Some(_) => Err(format!("argument `{key}` must be a string")),
            None => Err(format!("missing required argument `{key}`")),
        }
    }

    /// A non-empty path argument.
    fn path(&self, key: &str) -> Result<&'a str, String> {
        let text = self.text(key)?;
        if text.is_empty() {
            return Err(format!("argument `{key}` must not be empty"));
        }
        Ok(text)
    }

    fn text_list(&self, key: &str, required: bool) -> Result<Vec<&'a str>, String> {
        let items = match self.map.get(key) {
            Some(Value::Array(items)) => items,
            Some(_) => return Err(format!("argument `{key}` must be an array of strings")),
            None if required => return Err(format!("missing required argument `{key}`")),
            None => return Ok(Vec::new()),
        };
        if items.len() > MAX_LIST_ITEMS {
            return Err(format!(
                "argument `{key}` has {} entries; at most {MAX_LIST_ITEMS} are accepted",
                items.len()
            ));
        }
        items
            .iter()
            .map(|item| match item {
                Value::String(text) => check_text(key, text),
                _ => Err(format!("argument `{key}` must be an array of strings")),
            })
            .collect()
    }

    fn flag(&self, key: &str) -> Result<bool, String> {
        match self.map.get(key) {
            Some(Value::Bool(flag)) => Ok(*flag),
            Some(_) => Err(format!("argument `{key}` must be true or false")),
            None => Ok(false),
        }
    }

    /// An optional count between 1 and `max`; larger values are clamped.
    fn limit(&self, key: &str, default: usize, max: usize) -> Result<usize, String> {
        match self.map.get(key) {
            None => Ok(default),
            Some(value) => match value.as_u64() {
                Some(0) | None => Err(format!("argument `{key}` must be a positive integer")),
                Some(n) => Ok(usize::try_from(n).unwrap_or(max).min(max)),
            },
        }
    }
}

fn check_text<'a>(key: &str, text: &'a str) -> Result<&'a str, String> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "argument `{key}` is {} bytes long; at most {MAX_TEXT_BYTES} are accepted",
            text.len()
        ));
    }
    if text.contains('\0') {
        return Err(format!("argument `{key}` contains a NUL character"));
    }
    Ok(text)
}

/// Run one tool. `name` must be one of [`NAMES`].
pub(crate) fn call(
    state: &mut State,
    name: &str,
    arguments: &Map<String, Value>,
) -> Result<Output, String> {
    match name {
        "buzzard_check" => check(state, arguments),
        "buzzard_verify" => verify(state, arguments),
        "buzzard_deps" => deps(state, arguments),
        "buzzard_inspect" => inspect(state, arguments),
        "buzzard_diagram" => diagram(state, arguments),
        "buzzard_callers" => neighbors(state, arguments, Direction::Callers),
        "buzzard_callees" => neighbors(state, arguments, Direction::Callees),
        "buzzard_symbols" => symbols(state, arguments),
        "buzzard_rules" => rules(arguments),
        other => Err(format!("unknown tool `{other}`")),
    }
}

fn check(state: &mut State, arguments: &Map<String, Value>) -> Result<Output, String> {
    let args = Args::new(arguments, &["paths", "ignore", "limit", "refresh"])?;
    let raw_paths = args.text_list("paths", true)?;
    if raw_paths.is_empty() {
        return Err("argument `paths` must name at least one file or directory".to_owned());
    }
    let ignore = args.text_list("ignore", false)?;
    let limit = args.limit("limit", DEFAULT_DIAGNOSTIC_LIMIT, MAX_DIAGNOSTIC_LIMIT)?;
    let refresh = args.flag("refresh")?;

    if let Some(unknown) = ignore
        .iter()
        .find(|code| buzzard_lint::rules::rule(code).is_none())
    {
        return Err(format!(
            "unknown rule code `{unknown}` in `ignore`; call buzzard_rules for the list"
        ));
    }
    let mut roots = Vec::new();
    for raw in raw_paths {
        if raw.is_empty() {
            return Err("argument `paths` contains an empty path".to_owned());
        }
        let root = state.resolve(raw);
        if !archive::exists(&root) {
            return Err(format!("path does not exist: {}", state.show(&root)));
        }
        roots.push(root);
    }
    let config = LintConfig {
        ignore: ignore.into_iter().map(str::to_owned).collect(),
        ..LintConfig::default()
    };
    let (state, scan) = state.scan(roots, refresh);
    let diagnostics = lint(&scan.workspace, &scan.graph, &config);
    let files = scan.workspace.files().len();

    let count = |severity| {
        diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    };
    let (errors, warnings, infos) = (
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    );
    let mut by_code: BTreeMap<&str, usize> = BTreeMap::new();
    for diagnostic in &diagnostics {
        *by_code.entry(diagnostic.code).or_default() += 1;
    }
    let shown = diagnostics.len().min(limit);
    let truncated = shown < diagnostics.len();

    let mut text = String::new();
    for d in &diagnostics[..shown] {
        let _ = writeln!(
            text,
            "{}: {}[{}]: {}",
            state.show(&d.path),
            d.severity.as_str(),
            d.code,
            d.message
        );
    }
    if truncated {
        let _ = writeln!(
            text,
            "... {} more not shown; raise `limit` or narrow `paths`.",
            diagnostics.len() - shown
        );
    }
    if diagnostics.is_empty() {
        let _ = writeln!(text, "No problems found in {files} file(s).");
    } else {
        let _ = writeln!(
            text,
            "Found {} problem(s) ({errors} error(s), {warnings} warning(s), {infos} info) in {files} file(s).",
            diagnostics.len()
        );
    }
    let structured = json!({
        "diagnostics": diagnostics[..shown]
            .iter()
            .map(|d| diagnostic_json(state, d))
            .collect::<Vec<_>>(),
        "summary": {
            "files": files,
            "problems": diagnostics.len(),
            "errors": errors,
            "warnings": warnings,
            "infos": infos,
            "by_code": by_code,
            "shown": shown,
            "truncated": truncated,
        },
    });
    Ok(Output {
        text,
        structured: Some(structured),
    })
}

/// The state machines of the files under `paths`, as `buzzard verify`
/// reports them. Files are read from disk on every call.
fn verify(state: &mut State, arguments: &Map<String, Value>) -> Result<Output, String> {
    const FORMATS: [&str; 3] = ["text", "json", "dot"];
    let args = Args::new(arguments, &["paths", "format"])?;
    let raw_paths = args.text_list("paths", true)?;
    if raw_paths.is_empty() {
        return Err("argument `paths` must name at least one file or directory".to_owned());
    }
    let format = match arguments.get("format") {
        None => "text",
        Some(_) => args.text("format")?,
    };
    if !FORMATS.contains(&format) {
        return Err(format!(
            "argument `format` must be one of: {}",
            FORMATS.join(", ")
        ));
    }
    let mut roots = Vec::new();
    for raw in raw_paths {
        if raw.is_empty() {
            return Err("argument `paths` contains an empty path".to_owned());
        }
        let root = state.resolve(raw);
        if !archive::exists(&root) {
            return Err(format!("path does not exist: {}", state.show(&root)));
        }
        roots.push(root);
    }
    roots.sort();
    roots.dedup();
    let mut verified = Verified::scan(&roots);
    // Report paths the way every other tool does.
    for file in &mut verified.files {
        file.path = PathBuf::from(state.show(&file.path));
    }
    for (path, _) in &mut verified.unreadable {
        *path = PathBuf::from(state.show(path));
    }
    for path in &mut verified.incomplete {
        *path = PathBuf::from(state.show(path));
    }
    Ok(match format {
        "dot" => plain(report::to_dot(&verified)),
        "json" => {
            let value = to_value(&verified)?;
            Output {
                text: pretty(&value),
                structured: Some(value),
            }
        }
        _ => Output {
            text: report::to_text(&verified),
            structured: Some(to_value(&verified)?),
        },
    })
}

/// The `roots` argument of `buzzard_deps`: an object of root name to
/// directory, or a list of `NAME=DIR` strings as the command line takes.
fn root_directories(state: &State, args: &Args<'_>) -> Result<Roots, String> {
    let pairs: Vec<(&str, &str)> = match args.map.get("roots") {
        None => Vec::new(),
        Some(Value::Object(map)) => {
            if map.len() > MAX_LIST_ITEMS {
                return Err(format!(
                    "argument `roots` has {} entries; at most {MAX_LIST_ITEMS} are accepted",
                    map.len()
                ));
            }
            map.iter()
                .map(|(name, dir)| match dir {
                    Value::String(dir) => {
                        Ok((check_text("roots", name)?, check_text("roots", dir)?))
                    }
                    _ => Err(
                        "argument `roots` must map each root name to a directory string".to_owned(),
                    ),
                })
                .collect::<Result<_, _>>()?
        }
        Some(Value::Array(_)) => args
            .text_list("roots", false)?
            .into_iter()
            .map(|spec| Roots::split(spec).map_err(|err| format!("argument `roots`: {err}")))
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(
                "argument `roots` must be an object of root name to directory, or an array of \
                 NAME=DIR strings"
                    .to_owned(),
            )
        }
    };
    let mut roots = Roots::default();
    for (name, dir) in pairs {
        if dir.is_empty() {
            return Err(format!("argument `roots` has no directory for `{name}`"));
        }
        roots
            .add(name, &state.resolve(dir))
            .map_err(|err| format!("argument `roots`: {err}"))?;
    }
    Ok(roots)
}

/// The external dependencies of the files under `paths`, as `buzzard
/// deps` reports them.
fn deps(state: &mut State, arguments: &Map<String, Value>) -> Result<Output, String> {
    const FORMATS: [&str; 3] = ["text", "json", "markdown"];
    const DETAILS: [&str; 2] = ["packages", "files"];
    let args = Args::new(
        arguments,
        &["paths", "format", "detail", "roots", "refresh"],
    )?;
    let raw_paths = args.text_list("paths", true)?;
    if raw_paths.is_empty() {
        return Err("argument `paths` must name at least one file or directory".to_owned());
    }
    let format = match arguments.get("format") {
        None => "text",
        Some(_) => args.text("format")?,
    };
    if !FORMATS.contains(&format) {
        return Err(format!(
            "argument `format` must be one of: {}",
            FORMATS.join(", ")
        ));
    }
    let detail = match arguments.get("detail") {
        None => "packages",
        Some(_) => args.text("detail")?,
    };
    if !DETAILS.contains(&detail) {
        return Err(format!(
            "argument `detail` must be one of: {}",
            DETAILS.join(", ")
        ));
    }
    let refresh = args.flag("refresh")?;
    let roots = root_directories(state, &args)?;
    let mut scan_roots = Vec::new();
    for raw in raw_paths {
        if raw.is_empty() {
            return Err("argument `paths` contains an empty path".to_owned());
        }
        let root = state.resolve(raw);
        if !archive::exists(&root) {
            return Err(format!("path does not exist: {}", state.show(&root)));
        }
        scan_roots.push(root);
    }
    let (state, scan) = state.scan(scan_roots, refresh);
    let mut found = dependencies::scan(&scan.workspace, &scan.graph, &roots);
    // Report paths the way every other tool does.
    found.map_paths(|path| state.show(Path::new(path)));
    let (text, value) = if detail == "files" {
        let text = match format {
            "json" => ordered_json(&found)?,
            "markdown" => dependencies::to_markdown(&found),
            _ => dependencies::to_text(&found),
        };
        (text, to_value(&found)?)
    } else {
        // Counts per package: the size of the answer follows the number
        // of packages, where the full report lists every referrer of
        // every file and is megabytes for a large workspace.
        let overview = found.overview();
        let text = match format {
            "json" => ordered_json(&overview)?,
            "markdown" => dependencies::overview_to_markdown(&found) + "\n" + DETAIL_NOTE + "\n",
            _ => dependencies::overview_to_text(&found) + DETAIL_NOTE + "\n",
        };
        (text, to_value(&overview)?)
    };
    Ok(Output {
        text,
        structured: Some(value),
    })
}

/// The last line of an answer of `buzzard_deps` that holds counts only.
const DETAIL_NOTE: &str = "Counts only. Call again with `detail: \"files\"`, and narrower \
                           `paths` in a large workspace, for each file and the files that use it.";

/// Pretty JSON of a report, with its fields in the order of the struct,
/// as the command line prints them.
fn ordered_json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|text| text + "\n")
        .map_err(|err| format!("cannot serialize result: {err}"))
}

fn diagnostic_json(state: &State, diagnostic: &Diagnostic) -> Value {
    json!({
        "code": diagnostic.code,
        "severity": diagnostic.severity.as_str(),
        "path": state.show(&diagnostic.path),
        "message": diagnostic.message,
        "related": diagnostic.related.iter().map(|p| state.show(p)).collect::<Vec<_>>(),
    })
}

/// Resolve the `path` argument to an existing regular file, or to the
/// path of a member of an `.llb` archive that is one. Whether the archive
/// has that member is found out when it is read.
fn existing_file(state: &State, args: &Args<'_>) -> Result<PathBuf, String> {
    let path = state.resolve(args.path("path")?);
    if path.is_dir() {
        return Err(format!(
            "{} is a directory; this tool takes a single file",
            state.show(&path)
        ));
    }
    if !path.is_file() && archive::split(&path).is_none() {
        return Err(format!("file does not exist: {}", state.show(&path)));
    }
    Ok(path)
}

fn title(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn to_value(value: &impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| format!("cannot serialize result: {err}"))
}

fn pretty(value: &Value) -> String {
    // Serializing a `Value` cannot fail; its keys are always strings.
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

fn inspect(state: &mut State, arguments: &Map<String, Value>) -> Result<Output, String> {
    let args = Args::new(arguments, &["path"])?;
    let path = existing_file(state, &args)?;
    let shown = state.show(&path);
    if is_project_file(&path) {
        let project = ProjectFile::from_path(&path).map_err(|err| format!("{shown}: {err}"))?;
        return Ok(Output {
            text: summary::project_to_markdown(&title(&path), &project, None),
            structured: Some(json!({ "path": shown, "project": to_value(&project)? })),
        });
    }
    // An archive without a directory is inspected as the resource file
    // it is.
    let listing = if archive::is_archive(&path) && path.is_file() {
        archive::list(&path).map_err(|err| format!("{shown}: {err}"))?
    } else {
        None
    };
    if let Some(listing) = listing {
        let members: Vec<String> = listing
            .opened()
            .into_iter()
            .map(|name| state.show(&archive::member_path(&path, name)))
            .collect();
        return Ok(Output {
            text: summary::archive_to_markdown(&title(&path), &listing),
            structured: Some(json!({
                "path": shown,
                "archive": to_value(&listing)?,
                "member_paths": members,
            })),
        });
    }
    let inspection = Inspection::from_path(&path).map_err(|err| format!("{shown}: {err}"))?;
    Ok(Output {
        text: summary::to_markdown(&title(&path), &inspection.metadata, None),
        structured: Some(json!({
            "path": shown,
            "metadata": to_value(&inspection.metadata)?,
            "has_block_diagram": inspection.diagram.is_some(),
        })),
    })
}

fn diagram(state: &mut State, arguments: &Map<String, Value>) -> Result<Output, String> {
    const FORMATS: [&str; 5] = ["markdown", "json", "mermaid", "dot", "svg"];
    let args = Args::new(arguments, &["path", "format"])?;
    let format = match arguments.get("format") {
        None => "markdown",
        Some(_) => args.text("format")?,
    };
    if !FORMATS.contains(&format) {
        return Err(format!(
            "argument `format` must be one of: {}",
            FORMATS.join(", ")
        ));
    }
    let path = existing_file(state, &args)?;
    let shown = state.show(&path);
    if is_project_file(&path) {
        return Err(format!("{shown}: a project file has no block diagram"));
    }
    if archive::is_archive(&path) && path.is_file() {
        return Err(format!(
            "{shown}: an archive has no block diagram; pass one of its members as \
             `{shown}/<member name>` (buzzard_inspect lists them)"
        ));
    }
    let inspection = Inspection::from_path(&path).map_err(|err| format!("{shown}: {err}"))?;
    let Some(diagram) = &inspection.diagram else {
        return Err(format!(
            "{shown}: no block diagram could be read (the file has none, or it is stored in a \
             form this tool cannot decode); buzzard_inspect still works on it"
        ));
    };
    Ok(match format {
        "json" => {
            let value = to_value(diagram)?;
            Output {
                text: pretty(&value),
                structured: value.is_object().then_some(value),
            }
        }
        "mermaid" => plain(render::to_mermaid(diagram)),
        "dot" => plain(render::to_dot(diagram)),
        "svg" => plain(render::to_svg(diagram)),
        _ => plain(summary::diagram_to_markdown(diagram)),
    })
}

fn plain(text: String) -> Output {
    Output {
        text,
        structured: None,
    }
}

/// Resolve the `root` argument to an existing directory.
fn root_dir(state: &State, args: &Args<'_>) -> Result<PathBuf, String> {
    let root = state.resolve(args.path("root")?);
    if !root.is_dir() {
        return Err(format!(
            "root is not an existing directory: {}",
            state.show(&root)
        ));
    }
    Ok(root)
}

#[derive(Clone, Copy)]
enum Direction {
    Callers,
    Callees,
}

fn neighbors(
    state: &mut State,
    arguments: &Map<String, Value>,
    direction: Direction,
) -> Result<Output, String> {
    let args = Args::new(arguments, &["path", "root", "refresh"])?;
    let path = state.resolve(args.path("path")?);
    let root = root_dir(state, &args)?;
    let refresh = args.flag("refresh")?;
    let (state, scan) = state.scan(vec![root.clone()], refresh);
    // A member is found under any letter case of its name; the answer
    // names it as its archive does.
    let Some(path) = scan.workspace.stored_path(&path) else {
        return Err(format!(
            "{} is not one of the {} analyzable file(s) under root {}",
            state.show(&path),
            scan.workspace.files().len(),
            state.show(&root)
        ));
    };
    let found = match direction {
        Direction::Callers => scan.graph.callers(&path),
        Direction::Callees => scan.graph.callees(&path),
    };
    let files: Vec<String> = found.into_iter().map(|p| state.show(p)).collect();
    let (key, heading) = match direction {
        Direction::Callers => ("callers", "use"),
        Direction::Callees => ("callees", "are used by"),
    };
    let mut text = format!(
        "{} file(s) under {} {heading} {}",
        files.len(),
        state.show(&root),
        state.show(&path)
    );
    text.push_str(if files.is_empty() { ".\n" } else { ":\n" });
    for file in &files {
        let _ = writeln!(text, "- {file}");
    }
    let mut structured = json!({
        "path": state.show(&path),
        "root": state.show(&root),
        "scanned_files": scan.workspace.files().len(),
    });
    structured[key] = json!(files);
    Ok(Output {
        text,
        structured: Some(structured),
    })
}

fn symbol_kind(path: &Path, state: &FileState) -> &'static str {
    // A project file that failed to parse is still a project file.
    let project = state
        .project()
        .map(|project| project.kind)
        .or_else(|| ProjectKind::from_path(path));
    match (project, state.metadata().map(|meta| meta.kind)) {
        (Some(ProjectKind::Class), _) => "class",
        (Some(ProjectKind::Library), _) => "library",
        (Some(ProjectKind::Project), _) => "project",
        _ if state.archive().is_some() => "archive",
        (None, Some(FileKind::Control)) => "control",
        (None, _) => "vi",
    }
}

fn symbols(state: &mut State, arguments: &Map<String, Value>) -> Result<Output, String> {
    let args = Args::new(arguments, &["root", "query", "limit", "refresh"])?;
    let root = root_dir(state, &args)?;
    let query = args.text("query")?;
    let limit = args.limit("limit", DEFAULT_SYMBOL_LIMIT, MAX_SYMBOL_LIMIT)?;
    let refresh = args.flag("refresh")?;
    let (state, scan) = state.scan(vec![root.clone()], refresh);

    let mut scored: Vec<(i32, String, &Path, &'static str)> = scan
        .workspace
        .files()
        .iter()
        .filter_map(|(path, file)| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let score = buzzard_ls::fuzzy::score(query, &name)?;
            Some((score, name, path.as_path(), symbol_kind(path, file)))
        })
        .collect();
    // Best match first; ties broken by name, then path.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(b.2))
    });
    let matches = scored.len();
    scored.truncate(limit);

    let mut text = format!(
        "{matches} of {} file(s) under {} match `{query}`",
        scan.workspace.files().len(),
        state.show(&root)
    );
    if scored.len() < matches {
        let _ = write!(text, "; showing the best {}", scored.len());
    }
    text.push_str(if scored.is_empty() { ".\n" } else { ":\n" });
    let mut list = Vec::with_capacity(scored.len());
    for (_, name, path, kind) in &scored {
        let shown = state.show(path);
        let _ = writeln!(text, "- {shown} ({kind})");
        list.push(json!({ "name": name, "kind": kind, "path": shown }));
    }
    Ok(Output {
        text,
        structured: Some(json!({
            "root": state.show(&root),
            "query": query,
            "matches": matches,
            "symbols": list,
        })),
    })
}

fn rules(arguments: &Map<String, Value>) -> Result<Output, String> {
    Args::new(arguments, &[])?;
    let mut text =
        String::from("| Code | Name | Severity | Meaning |\n| --- | --- | --- | --- |\n");
    let mut list = Vec::with_capacity(RULES.len());
    for rule in RULES {
        let _ = writeln!(
            text,
            "| `{}` | {} | {} | {} |",
            rule.code,
            rule.name,
            rule.severity.as_str(),
            rule.summary
        );
        list.push(json!({
            "code": rule.code,
            "name": rule.name,
            "severity": rule.severity.as_str(),
            "summary": rule.summary,
        }));
    }
    Ok(Output {
        text,
        structured: Some(json!({ "rules": list })),
    })
}

/// The `tools/list` payload, in the order of [`NAMES`].
#[expect(
    clippy::too_many_lines,
    reason = "the schemas of all nine tools, kept together as one table"
)]
pub(crate) fn definitions() -> Value {
    let path = |what: &str| {
        json!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_TEXT_BYTES,
            "description": format!(
                "{what} Relative paths are resolved against the server's working directory."
            ),
        })
    };
    let refresh = json!({
        "type": "boolean",
        "default": false,
        "description": "Force every file to be re-read and re-parsed instead of reusing the cached scan.",
    });
    let root = path("Directory to scan recursively (hidden directories are skipped).");
    let tool =
        |name: &str, title: &str, description: String, properties: Value, required: &[&str]| {
            json!({
                "name": name,
                "title": title,
                "description": description,
                "inputSchema": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                    "additionalProperties": false,
                },
                "annotations": {
                    "title": title,
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false,
                },
            })
        };
    json!([
        tool(
            "buzzard_check",
            "Lint LabVIEW files",
            format!(
                "Run buzzard's static analysis over LabVIEW files (.vi, .ctl, .lvproj, .lvlib, \
                 .lvclass) and report problems. Each diagnostic has a rule code (BZxxx), a \
                 severity (error, warning or info), the file path and a message; the summary \
                 counts files and problems per severity and per code. Cross-file rules \
                 (circular dependencies, version mismatch) only see the files under `paths`, \
                 so pass the project directory rather than a single file when you can. \
                 Diagnostics are sorted by path, then code. Call buzzard_rules to learn what \
                 a code means. Read-only. {CACHE_NOTE}"
            ),
            json!({
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_LIST_ITEMS,
                    "items": path("A file or directory to analyze; directories are scanned recursively."),
                    "description": "Files and directories to analyze together as one workspace.",
                },
                "ignore": {
                    "type": "array",
                    "maxItems": MAX_LIST_ITEMS,
                    "items": { "type": "string", "pattern": "^BZ[0-9]{3}$" },
                    "description": "Rule codes to skip, for example [\"BZ001\"]. Unknown codes are an error.",
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_DIAGNOSTIC_LIMIT,
                    "default": DEFAULT_DIAGNOSTIC_LIMIT,
                    "description": "Most diagnostics to return. The summary always counts all of them and says whether the list was truncated.",
                },
                "refresh": refresh,
            }),
            &["paths"],
        ),
        tool(
            "buzzard_verify",
            "Verify state machines",
            "Find the state machines on the block diagrams of .vi files and check them. A \
             state machine is a while loop around a case structure selected by a shift \
             register. For each one the result gives its location, its states (the cases), \
             the transitions between them, the initial and default state, whether the model \
             is exact, and the findings: states that can never be reached, states from which \
             the loop's condition terminal can never stop the loop, a loop whose condition \
             terminal can never stop it, and constants no case handles that can reach the \
             state register. Each finding is labelled PROVEN, STRUCTURAL or HEURISTIC with \
             one sentence of justification. PROVEN findings hold on a model that \
             over-approximates the diagram as saved in the file: wherever a next state or \
             stop condition cannot be followed (a subVI, arithmetic, a control), every \
             possibility is assumed, so a state reported as unreachable really is, while a \
             state not reported may still be dead. STRUCTURAL findings are facts about the \
             wiring that say nothing about what runs: a constant without a case, and a loop \
             that its condition terminal cannot stop while the diagram holds subVI calls or \
             other nodes (named in the finding) that could end the program some other way. \
             A block diagram that did not decode completely is listed and not searched. \
             String/queue state machines in the style of the JKI State Machine are \
             recognised but not modelled; their only check, enqueued names without a case, \
             is HEURISTIC. Files without a state machine are only counted. Formats: `text` \
             (default), `json` (the same data, also returned as structuredContent) and `dot` \
             (the transition graphs as Graphviz source). Files are read from disk on every \
             call (never cached). Read-only."
                .to_owned(),
            json!({
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_LIST_ITEMS,
                    "items": path("A file or directory to analyze; directories are scanned recursively."),
                    "description": "Files and directories whose block diagrams to check.",
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "json", "dot"],
                    "default": "text",
                    "description": "Output format.",
                },
            }),
            &["paths"],
        ),
        tool(
            "buzzard_deps",
            "List external dependencies",
            format!(
                "List what LabVIEW files (.vi, .ctl, .lvproj, .lvlib, .lvclass) under `paths` \
                 need from outside, as `buzzard deps` prints it. Three lists. (1) Files linked \
                 under a symbolic root such as <vilib> (libraries shipped with LabVIEW and \
                 installed add-ons), <instrlib> or <userlib>, grouped by root and by package, \
                 each with the files that use it. A package is a directory, not a product \
                 name: the first two directories below the root (three below `addons`), cut \
                 short at a library such as `error.llb`. (2) Machine-absolute links, which \
                 only work on a machine laid out like the one that saved the file. (3) \
                 Relative links with no file at their location. Each file under a root has a \
                 status: `unchecked` unless `roots` maps that root to a directory, then \
                 `found` or `missing` by a look at the disk; a file inside an .llb or another \
                 file counts as found when that file exists, and letter case is ignored. Files \
                 that cannot be parsed are listed under `unparsed` and are not an error. By \
                 default (`detail` \"packages\") the answer holds counts: per package the \
                 number of files, their statuses and the number of files that use them, and \
                 per link the number of files that hold it. `detail` \"files\" returns what \
                 `buzzard deps` prints, every file with every file that uses it; that is \
                 megabytes for a workspace of a thousand files, so narrow `paths` first. \
                 Formats: `text` (default), `json` and `markdown`; the JSON of the chosen \
                 detail is always returned as structuredContent. Read-only. {CACHE_NOTE}"
            ),
            json!({
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_LIST_ITEMS,
                    "items": path("A file or directory to analyze; directories are scanned recursively."),
                    "description": "Files and directories to analyze together as one workspace.",
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "json", "markdown"],
                    "default": "text",
                    "description": "Output format.",
                },
                "detail": {
                    "type": "string",
                    "enum": ["packages", "files"],
                    "default": "packages",
                    "description": "`packages`: counts per package and per link. `files`: every file and the files that use it, as `buzzard deps` prints.",
                },
                "roots": {
                    "description": "Directories of symbolic roots, to look their files up on disk: an object such as {\"vilib\": \"/opt/labview/vi.lib\"}, or an array of NAME=DIR strings. Names are written without angle brackets and matched without regard to case. Each directory must exist.",
                    "oneOf": [
                        {
                            "type": "object",
                            "maxProperties": MAX_LIST_ITEMS,
                            "additionalProperties": { "type": "string", "minLength": 1, "maxLength": MAX_TEXT_BYTES },
                        },
                        {
                            "type": "array",
                            "maxItems": MAX_LIST_ITEMS,
                            "items": { "type": "string", "pattern": "^[^=]+=.+$", "maxLength": MAX_TEXT_BYTES },
                        },
                    ],
                },
                "refresh": refresh,
            }),
            &["paths"],
        ),
        tool(
            "buzzard_inspect",
            "Inspect one LabVIEW file",
            "Describe one LabVIEW file without opening the IDE. For a .vi or .ctl: kind, saved \
             LabVIEW version, execution flags (reentrancy, debugging, breakpoints, separated \
             compiled code, password protection), owning libraries and the list of files it \
             depends on, as stored in the file. For a .lvproj, .lvlib or .lvclass: kind, \
             version, members and parent class. Returns Markdown as text and the same data as \
             structuredContent. The file is read from disk on every call (never cached). \
             Read-only. Use buzzard_diagram for the block diagram and buzzard_callers for who \
             uses the file."
                .to_owned(),
            json!({ "path": path("The file to inspect.") }),
            &["path"],
        ),
        tool(
            "buzzard_diagram",
            "Export a VI's block diagram",
            "Export the block diagram (the code) of one .vi or .ctl file. Formats: `markdown` \
             (default; a node and wire listing, best for reading the logic), `json` (the full \
             decoded graph of nodes, terminals and wires), `mermaid` (flowchart source), `dot` \
             (Graphviz source) and `svg` (a picture laid out as in the editor; large). Fails \
             with a clear message when the file has no decodable diagram, for example a \
             password-protected VI or a project file. Decoding is reverse-engineered: node \
             kinds it does not know are labelled generically rather than guessed. The file is \
             read from disk on every call (never cached). Read-only."
                .to_owned(),
            json!({
                "path": path("The .vi or .ctl file whose diagram to export."),
                "format": {
                    "type": "string",
                    "enum": ["markdown", "json", "mermaid", "dot", "svg"],
                    "default": "markdown",
                    "description": "Output format.",
                },
            }),
            &["path"],
        ),
        tool(
            "buzzard_callers",
            "Find files that use a file",
            format!(
                "List the files under `root` that use `path` (for example the VIs that call a \
                 subVI or use a type definition), sorted by path. Only files under `root` are \
                 searched, so pass the project's top directory; `path` itself must be under \
                 `root`. Membership in a project, library or class is not a use. Read-only. \
                 {CACHE_NOTE}"
            ),
            json!({
                "path": path("The file whose users to find."),
                "root": root,
                "refresh": refresh,
            }),
            &["path", "root"],
        ),
        tool(
            "buzzard_callees",
            "Find files a file uses",
            format!(
                "List the files under `root` that `path` uses (its subVIs, type definitions \
                 and other dependencies that exist under `root`), sorted by path. \
                 Dependencies outside `root` or missing from disk, such as LabVIEW's own \
                 vi.lib, are not listed; buzzard_inspect shows every stored dependency. \
                 Read-only. {CACHE_NOTE}"
            ),
            json!({
                "path": path("The file whose dependencies to list."),
                "root": root,
                "refresh": refresh,
            }),
            &["path", "root"],
        ),
        tool(
            "buzzard_symbols",
            "Search LabVIEW files by name",
            format!(
                "Fuzzy-search the LabVIEW files under `root` by file name. The query matches \
                 as a case-insensitive subsequence (`rdcfg` finds `Read Config.vi`); an empty \
                 query lists files. Each result has the file name, its kind (vi, control, \
                 class, library or project) and its path. Best matches come first; ties are \
                 ordered by name, then path. Read-only. {CACHE_NOTE}"
            ),
            json!({
                "root": root,
                "query": {
                    "type": "string",
                    "maxLength": MAX_TEXT_BYTES,
                    "description": "Characters to find in the file name, in order.",
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SYMBOL_LIMIT,
                    "default": DEFAULT_SYMBOL_LIMIT,
                    "description": "Most results to return.",
                },
                "refresh": refresh,
            }),
            &["root", "query"],
        ),
        tool(
            "buzzard_rules",
            "List buzzard's lint rules",
            "List every rule buzzard_check can report: code, name, default severity and \
             meaning. Takes no arguments and reads no files."
                .to_owned(),
            json!({}),
            &[],
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_cover_every_tool_in_order() {
        let definitions = definitions();
        let names: Vec<&str> = definitions
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, NAMES);
        for name in NAMES {
            assert!(name.starts_with("buzzard_"), "{name}");
        }
    }

    #[test]
    fn every_required_argument_is_declared() {
        for tool in definitions().as_array().unwrap() {
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false);
            for required in schema["required"].as_array().unwrap() {
                let key = required.as_str().unwrap();
                assert!(schema["properties"][key].is_object(), "{tool}");
            }
            assert!(tool["description"].as_str().unwrap().len() > 40);
        }
    }

    #[test]
    fn shows_paths_relative_to_the_base() {
        let state = State::new(Path::new("/work/./project"));
        assert_eq!(state.show(Path::new("/work/project/a/b.vi")), "a/b.vi");
        assert_eq!(state.show(Path::new("/work/project")), ".");
        assert_eq!(state.show(Path::new("/elsewhere/b.vi")), "/elsewhere/b.vi");
        assert_eq!(state.resolve("a/../b.vi"), Path::new("/work/project/b.vi"));
        assert_eq!(state.resolve("../b.vi"), Path::new("/work/b.vi"));
        assert_eq!(state.resolve("/abs/b.vi"), Path::new("/abs/b.vi"));
    }

    #[test]
    fn reuses_the_scan_until_the_root_changes_or_refresh_is_set() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzzard-rsrc/tests/fixtures");
        let mut state = State::new(&fixtures);
        let args = |value: Value| value.as_object().unwrap().clone();
        let mut search = |arguments: Value, expected_parses: u64| {
            let output = symbols(&mut state, &args(arguments)).unwrap();
            assert_eq!(state.parses, expected_parses);
            output.text
        };
        let first = search(json!({ "root": "pylavi", "query": "" }), 1);
        let second = search(json!({ "root": "pylavi", "query": "" }), 1);
        assert_eq!(first, second);
        // The same directory spelled differently is the same root.
        search(json!({ "root": "./pylavi/../pylavi", "query": "add" }), 1);
        search(json!({ "root": "pylavi", "query": "", "refresh": true }), 2);
        search(
            json!({ "root": "pylavi", "query": "", "refresh": false }),
            2,
        );
        search(json!({ "root": "pylabview", "query": "" }), 3);
        search(json!({ "root": "pylavi", "query": "" }), 4);
        // Other tools share the cache when they ask for the same root.
        let callers = json!({ "root": "pylavi", "path": "pylavi/empty.vi" });
        neighbors(&mut state, &args(callers), Direction::Callers).unwrap();
        check(&mut state, &args(json!({ "paths": ["pylavi"] }))).unwrap();
        assert_eq!(state.parses, 4);
    }

    #[test]
    fn limits_are_checked_and_clamped() {
        let map = |value: Value| value.as_object().unwrap().clone();
        let limit = |value: Value| {
            Args::new(&map(value), &["limit"])
                .unwrap()
                .limit("limit", 5, 9)
        };
        assert_eq!(limit(json!({})), Ok(5));
        assert_eq!(limit(json!({ "limit": 3 })), Ok(3));
        assert_eq!(limit(json!({ "limit": u64::MAX })), Ok(9));
        assert!(limit(json!({ "limit": 0 })).is_err());
        assert!(limit(json!({ "limit": -1 })).is_err());
        assert!(limit(json!({ "limit": 1.5 })).is_err());
        assert!(limit(json!({ "limit": "3" })).is_err());
    }
}
