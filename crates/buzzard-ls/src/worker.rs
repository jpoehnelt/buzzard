//! The one place where the analyzed state changes.
//!
//! Every mutation (the initial scan, file-watcher batches and
//! `workspace/didChangeWatchedFiles`) is queued as a [`Job`] and handled by
//! a single task, one job after the other: read the files, apply the
//! result, publish the diagnostics. Nothing else writes the state or
//! publishes, so both are totally ordered and the last thing a client sees
//! for a file is the newest.
//!
//! Requests never wait for that task. They read an immutable [`Snapshot`]
//! that is swapped in when a job has finished computing the next one.

use std::collections::BTreeMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

use buzzard_lint::archive::is_archive;
use buzzard_lint::project::is_project_file;
use buzzard_lint::workspace::discover;
use buzzard_lint::{FileState, LintConfig, ViInspector};
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::engine::{Engine, Snapshot, Update};
use crate::paths::{Presence, Roots};

/// How long to wait for a burst of file events to settle before re-analyzing.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// A file that fails to parse right after it was written is probably still
/// being written. It is read again after each of these delays before the
/// failure is reported.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(60),
    Duration::from_millis(120),
    Duration::from_millis(240),
];

/// A file modified longer ago than this is at rest: a failure is final.
const SETTLED_AFTER: Duration = Duration::from_secs(2);

/// Below this many files, reading them on one thread is quicker.
const PARALLEL_THRESHOLD: usize = 16;

type Watcher = Debouncer<notify::RecommendedWatcher>;

/// Changed paths and how often each has been read without success so far.
pub(crate) type Batch = BTreeMap<PathBuf, usize>;

/// Modification time and size of a file when it was last read.
type Signature = Option<(SystemTime, u64)>;

#[derive(Debug)]
pub(crate) enum Job {
    /// The workspace folders named by `initialize`.
    Roots(Vec<PathBuf>),
    /// The client is ready: read every root, unless that was done already.
    Scan,
    /// Read every root from scratch again.
    Rescan,
    /// Files or directories that changed on disk, in any spelling.
    Paths(Vec<PathBuf>),
    /// Files to read again after a failure that looked transient.
    Retry(Batch),
    /// End the worker, which also stops the file watcher.
    Shutdown,
}

/// What handling one batch produced.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Diagnostics to publish, sorted by path.
    pub updates: Vec<Update>,
    /// Files to read again shortly, with their new attempt count.
    pub retry: Batch,
}

struct Core {
    engine: Engine,
    roots: Roots,
    signatures: BTreeMap<PathBuf, Signature>,
}

/// The analyzed state: written by the worker, read by request handlers.
pub(crate) struct State {
    core: Mutex<Core>,
    view: RwLock<Arc<Snapshot>>,
    inspector: Arc<dyn ViInspector>,
}

fn signature(path: &Path) -> Signature {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// True if the file may still be in the middle of being written.
fn is_unsettled(signature: &Signature) -> bool {
    match signature {
        // Gone or unreadable: the next look decides what it is.
        None => true,
        // A time in the future (clock skew) counts as recent.
        Some((modified, _)) => SystemTime::now()
            .duration_since(*modified)
            .map_or(true, |age| age < SETTLED_AFTER),
    }
}

impl State {
    pub(crate) fn new(inspector: Arc<dyn ViInspector>) -> Self {
        Self {
            core: Mutex::new(Core {
                engine: Engine::default(),
                roots: Roots::default(),
                signatures: BTreeMap::new(),
            }),
            view: RwLock::new(Arc::default()),
            inspector,
        }
    }

    /// Only the worker takes this lock; a panic while it was held must not
    /// take the server down with it.
    fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current state. Never waits for a scan: the lock is only held
    /// while a pointer is copied or swapped.
    pub(crate) fn view(&self) -> Arc<Snapshot> {
        self.view
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn show(&self, snapshot: Arc<Snapshot>) {
        *self.view.write().unwrap_or_else(PoisonError::into_inner) = snapshot;
    }

    pub(crate) fn set_roots(&self, roots: Vec<PathBuf>) {
        let mut core = self.core();
        core.engine = Engine::new(roots, LintConfig::default());
        core.roots = Roots::default();
        core.signatures.clear();
    }

    pub(crate) fn roots(&self) -> Vec<PathBuf> {
        self.core().engine.roots().to_vec()
    }

    /// Read every root from scratch. Blocking.
    pub(crate) fn scan(&self) -> Vec<Update> {
        let mut core = self.core();
        core.roots = Roots::new(core.engine.roots());
        let updates = core.engine.scan();
        let snapshot = core.engine.snapshot();
        core.signatures = snapshot
            .paths()
            .map(|path| (path.to_path_buf(), signature(path)))
            .collect();
        self.show(snapshot);
        updates
    }

    /// Bring the state in line with the disk for every path in `batch`.
    /// Blocking; `runtime` drives the inspector backend.
    ///
    /// A path is taken for what a fresh scan would make of it now, whatever
    /// the event said: a directory stands for everything below it, and a
    /// path that discovery would skip (hidden, behind a symbolic link,
    /// outside the roots) is not in the workspace.
    pub(crate) fn process(&self, runtime: &Handle, batch: Batch) -> Outcome {
        let mut core = self.core();
        let before = core.engine.snapshot();

        // Every path whose state may have changed. `Some(attempt)` forces a
        // read; `None` reads only if the file is new or looks different.
        let mut candidates: BTreeMap<PathBuf, Option<usize>> = BTreeMap::new();
        for (raw, attempt) in batch {
            let Some(path) = core.roots.localize(&raw) else {
                continue;
            };
            for below in before.paths_under(&path) {
                // A member of an archive is not on disk: it comes and goes
                // with its archive, which is a candidate itself.
                if !before.is_member(&below) {
                    candidates.entry(below).or_insert(None);
                }
            }
            if core.roots.presence(&path) == Presence::Dir {
                for found in discover(std::slice::from_ref(&path)) {
                    candidates.entry(found).or_insert(None);
                }
            }
            let forced = candidates.entry(path).or_insert(Some(attempt));
            *forced = Some(forced.map_or(attempt, |earlier| earlier.min(attempt)));
        }

        let mut changes: Vec<(PathBuf, Option<FileState>)> = Vec::new();
        let mut to_read: Vec<(PathBuf, usize)> = Vec::new();
        for (path, forced) in candidates {
            if core.roots.presence(&path) != Presence::File {
                core.signatures.remove(&path);
                if before.contains(&path) {
                    changes.push((path, None));
                }
            } else if let Some(attempt) = forced {
                to_read.push((path, attempt));
            } else if !before.contains(&path)
                || core.signatures.get(&path) != Some(&signature(&path))
            {
                to_read.push((path, 0));
            }
        }

        let mut retry = Batch::new();
        for (path, attempt, signature, states) in read_all(runtime, &self.inspector, to_read) {
            let failed = matches!(states.first(), Some((_, FileState::Failed(_))));
            if failed && attempt < RETRY_DELAYS.len() && is_unsettled(&signature) {
                // Keep what was known about the file until it settles.
                retry.insert(path, attempt + 1);
            } else {
                core.signatures.insert(path, signature);
                // An archive comes with its members, the archive first.
                changes.extend(states.into_iter().map(|(path, state)| (path, Some(state))));
            }
        }

        let updates = if changes.is_empty() {
            Vec::new()
        } else {
            core.engine.apply(changes)
        };
        self.show(core.engine.snapshot());
        Outcome { updates, retry }
    }
}

/// A file that was read: its path, the attempt, its signature, and its
/// state under its path, followed by the members of an archive.
type Read = (PathBuf, usize, Signature, Vec<(PathBuf, FileState)>);

fn read_one(runtime: &Handle, inspector: &dyn ViInspector, path: PathBuf, attempt: usize) -> Read {
    // Taken before reading: if the file changes meanwhile, the signature is
    // stale and the file is read again rather than trusted.
    let signature = signature(&path);
    let states = if is_project_file(&path) || is_archive(&path) {
        // Project-level XML and archives are always read from disk; the
        // inspector backend only knows binary files.
        FileState::load_all(&path)
    } else {
        let state = FileState::from_inspection(runtime.block_on(inspector.inspect(&path)));
        vec![(path.clone(), state)]
    };
    (path, attempt, signature, states)
}

/// Read the given files, in the given order, on as many threads as help.
fn read_all(
    runtime: &Handle,
    inspector: &Arc<dyn ViInspector>,
    files: Vec<(PathBuf, usize)>,
) -> Vec<Read> {
    let threads = std::thread::available_parallelism().map_or(1, NonZero::get);
    if files.len() < PARALLEL_THRESHOLD || threads < 2 {
        return files
            .into_iter()
            .map(|(path, attempt)| read_one(runtime, inspector.as_ref(), path, attempt))
            .collect();
    }
    let chunk = files.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|(path, attempt)| {
                            read_one(runtime, inspector.as_ref(), path.clone(), *attempt)
                        })
                        .collect::<Vec<Read>>()
                })
            })
            .collect();
        // A reader that panicked contributes nothing; its files keep their
        // previous state.
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// Where the worker sends diagnostics. The server publishes them to the
/// client; tests record them.
#[tower_lsp::async_trait]
pub(crate) trait Publisher: Send + Sync {
    async fn publish(&self, updates: Vec<Update>);
}

fn start_watcher(roots: &[PathBuf], jobs: &UnboundedSender<Job>) -> Option<Watcher> {
    let jobs = jobs.clone();
    let mut debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
        let _ = jobs.send(match result {
            Ok(events) => Job::Paths(events.into_iter().map(|e| e.path).collect()),
            // Events were lost (for example a queue overflow): start over.
            Err(_) => Job::Rescan,
        });
    })
    .ok()?;
    for root in roots {
        let _ = debouncer.watcher().watch(root, RecursiveMode::Recursive);
    }
    Some(debouncer)
}

/// Everything queued at one moment, merged: one pass over the disk serves
/// any number of events.
#[derive(Default)]
struct Pending {
    roots: Option<Vec<PathBuf>>,
    scan: bool,
    rescan: bool,
    paths: Batch,
    shutdown: bool,
}

impl Pending {
    fn add(&mut self, job: Job) {
        match job {
            Job::Roots(roots) => self.roots = Some(roots),
            Job::Scan => self.scan = true,
            Job::Rescan => self.rescan = true,
            Job::Paths(paths) => {
                for path in paths {
                    self.paths.insert(path, 0);
                }
            }
            Job::Retry(batch) => {
                for (path, attempt) in batch {
                    let known = self.paths.entry(path).or_insert(attempt);
                    *known = (*known).min(attempt);
                }
            }
            Job::Shutdown => self.shutdown = true,
        }
    }
}

/// Handle jobs one at a time until `Shutdown` arrives or every sender is
/// gone. `jobs` is used to schedule retries and to feed watcher events.
pub(crate) async fn run(
    state: Arc<State>,
    publisher: impl Publisher,
    mut queue: UnboundedReceiver<Job>,
    jobs: UnboundedSender<Job>,
) {
    let mut watcher: Option<Watcher> = None;
    let mut have_roots = false;
    let mut want_scan = false;
    let mut scanned = false;
    while let Some(job) = queue.recv().await {
        let mut pending = Pending::default();
        pending.add(job);
        while let Ok(job) = queue.try_recv() {
            pending.add(job);
        }
        if pending.shutdown {
            break;
        }
        if let Some(roots) = pending.roots {
            state.set_roots(roots);
            have_roots = true;
            scanned = false;
            watcher = None;
        }
        // `initialized` may be seen before `initialize` has delivered the
        // roots; the scan then waits for them. It is asked for more than
        // once (see `server::Handshake`) and done once.
        want_scan |= (pending.scan && !scanned) || (pending.rescan && scanned);
        if want_scan && have_roots {
            want_scan = false;
            scanned = true;
            if watcher.is_none() {
                // Watch before scanning so a save during the scan is seen.
                watcher = start_watcher(&state.roots(), &jobs);
            }
            let state = Arc::clone(&state);
            let updates = tokio::task::spawn_blocking(move || state.scan()).await;
            publisher.publish(updates.unwrap_or_default()).await;
        }
        if !scanned || pending.paths.is_empty() {
            continue;
        }
        let outcome = {
            let state = Arc::clone(&state);
            let runtime = Handle::current();
            let batch = pending.paths;
            tokio::task::spawn_blocking(move || state.process(&runtime, batch)).await
        }
        .unwrap_or_default();
        publisher.publish(outcome.updates).await;

        let mut by_attempt: BTreeMap<usize, Batch> = BTreeMap::new();
        for (path, attempt) in outcome.retry {
            by_attempt.entry(attempt).or_default().insert(path, attempt);
        }
        for (attempt, batch) in by_attempt {
            let delay = RETRY_DELAYS[(attempt - 1).min(RETRY_DELAYS.len() - 1)];
            let jobs = jobs.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = jobs.send(Job::Retry(batch));
            });
        }
    }
}

#[cfg(test)]
mod tests;
