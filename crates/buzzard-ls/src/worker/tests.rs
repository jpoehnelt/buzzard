use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::time::Instant;

use buzzard_lint::Inspection;
use buzzard_rsrc::builder::RsrcBuilder;
use buzzard_rsrc::LvPath;
use tokio::sync::mpsc;

use super::*;

const V23: u32 = 0x2300_8000;
const CLEAN: [u32; 6] = [0, 0x400, 0, 0, 0, 0];
const INLINE_CODE: [u32; 6] = [0; 6];

type Simple = Vec<(String, Vec<&'static str>)>;

/// Parses files from disk like the real backend, but counts the calls,
/// can fail the first few of them, and can be held up by a test.
#[derive(Default)]
struct TestInspector {
    calls: AtomicUsize,
    failures_left: AtomicUsize,
    gate: Mutex<Option<std_mpsc::Receiver<()>>>,
}

/// Decrement a counter unless it is zero; true if it was decremented.
///
/// Written as a compare-and-swap loop because the standard helper for this
/// was renamed between the oldest and newest toolchains this crate builds
/// on.
fn take_one(counter: &AtomicUsize) -> bool {
    let mut current = counter.load(Ordering::SeqCst);
    while current > 0 {
        match counter.compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(seen) => current = seen,
        }
    }
    false
}

#[tower_lsp::async_trait]
impl ViInspector for TestInspector {
    async fn inspect(&self, path: &Path) -> Result<Inspection, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
        let fail = take_one(&self.failures_left);
        if fail {
            return Err("file is truncated".to_owned());
        }
        Inspection::from_path(path)
    }
}

struct Fixture {
    dir: PathBuf,
    inspector: Arc<TestInspector>,
    state: Arc<State>,
    runtime: tokio::runtime::Runtime,
}

fn write_vi(path: &Path, words: &[u32]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = RsrcBuilder::default().save_record(V23, words).build();
    fs::write(path, bytes).unwrap();
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("buzzard-ls-{}-worker-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ws")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let inspector = Arc::new(TestInspector::default());
        let state = Arc::new(State::new(inspector.clone()));
        state.set_roots(vec![dir.join("ws")]);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        Self {
            dir,
            inspector,
            state,
            runtime,
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.join("ws")
    }

    fn vi(&self, name: &str, words: &[u32]) {
        write_vi(&self.root().join(name), words);
    }

    fn reads(&self) -> usize {
        self.inspector.calls.load(Ordering::SeqCst)
    }

    /// Report these paths (relative to the root) as changed.
    fn changed(&self, names: &[&str]) -> Outcome {
        let batch = names.iter().map(|n| (self.root().join(n), 0)).collect();
        self.state.process(self.runtime.handle(), batch)
    }

    fn relative(&self, path: &Path) -> String {
        let name = path.strip_prefix(self.root()).unwrap().to_string_lossy();
        name.replace('\\', "/")
    }

    fn simplify(&self, updates: &[Update]) -> Simple {
        updates
            .iter()
            .map(|(path, diagnostics)| {
                (
                    self.relative(path),
                    diagnostics.iter().map(|d| d.code).collect(),
                )
            })
            .collect()
    }

    fn files(&self) -> Vec<String> {
        let view = self.state.view();
        let names = view.paths().map(|path| self.relative(path));
        names.collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn update(name: &str, codes: &[&'static str]) -> (String, Vec<&'static str>) {
    (name.to_owned(), codes.to_vec())
}

fn nothing() -> Simple {
    Vec::new()
}

/// A directory rename is reported for the directory only; the files below
/// it must follow.
#[test]
fn directory_rename_moves_every_file_below_it() {
    let f = Fixture::new("rename");
    f.vi("sub/a.vi", &INLINE_CODE);
    f.vi("sub/deep/b.vi", &INLINE_CODE);
    f.vi("keep.vi", &CLEAN);
    let scanned = f.state.scan();
    assert_eq!(
        f.simplify(&scanned),
        [
            update("sub/a.vi", &["BZ001"]),
            update("sub/deep/b.vi", &["BZ001"])
        ]
    );

    fs::rename(f.root().join("sub"), f.root().join("sub2")).unwrap();
    let outcome = f.changed(&["sub", "sub2"]);
    assert_eq!(
        f.simplify(&outcome.updates),
        [
            update("sub/a.vi", &[]),
            update("sub/deep/b.vi", &[]),
            update("sub2/a.vi", &["BZ001"]),
            update("sub2/deep/b.vi", &["BZ001"]),
        ]
    );
    assert!(outcome.retry.is_empty());
    assert_eq!(f.files(), ["keep.vi", "sub2/a.vi", "sub2/deep/b.vi"]);
    assert!(f.state.view().hover(&f.root().join("sub2/a.vi")).is_some());
    assert!(f.state.view().hover(&f.root().join("sub/a.vi")).is_none());
}

#[test]
fn directory_moved_in_and_directory_deleted() {
    let f = Fixture::new("move-in");
    f.vi("keep.vi", &CLEAN);
    f.state.scan();

    // Prepared outside the workspace, then moved in as a whole.
    write_vi(&f.dir.join("staging/x.vi"), &INLINE_CODE);
    write_vi(&f.dir.join("staging/inner/y.ctl"), &INLINE_CODE);
    write_vi(&f.dir.join("staging/.hidden/z.vi"), &INLINE_CODE);
    fs::rename(f.dir.join("staging"), f.root().join("lib")).unwrap();
    let outcome = f.changed(&["lib"]);
    assert_eq!(
        f.simplify(&outcome.updates),
        [
            update("lib/inner/y.ctl", &["BZ001"]),
            update("lib/x.vi", &["BZ001"])
        ]
    );
    assert_eq!(f.files(), ["keep.vi", "lib/inner/y.ctl", "lib/x.vi"]);

    fs::remove_dir_all(f.root().join("lib")).unwrap();
    let outcome = f.changed(&["lib"]);
    assert_eq!(
        f.simplify(&outcome.updates),
        [update("lib/inner/y.ctl", &[]), update("lib/x.vi", &[])]
    );
    assert_eq!(f.files(), ["keep.vi"]);
}

/// A directory event must not cost a re-read of everything below it: some
/// platforms report the parent directory on every save.
#[test]
fn directory_event_reads_only_new_or_modified_files() {
    let f = Fixture::new("dir-cost");
    f.vi("sub/a.vi", &INLINE_CODE);
    f.vi("sub/b.vi", &INLINE_CODE);
    f.state.scan();
    assert_eq!(f.reads(), 0);

    let outcome = f.changed(&["sub", ""]);
    assert_eq!(f.simplify(&outcome.updates), nothing());
    assert_eq!(f.reads(), 0);

    // Same name, different content and size: noticed without an event for
    // the file itself.
    let bytes = RsrcBuilder::default()
        .save_record(V23, &CLEAN)
        .links(&[("a.vi", LvPath::sibling("a.vi"))])
        .build();
    fs::write(f.root().join("sub/b.vi"), bytes).unwrap();
    f.vi("sub/c.vi", &CLEAN);
    let outcome = f.changed(&["sub"]);
    assert_eq!(f.simplify(&outcome.updates), [update("sub/b.vi", &[])]);
    assert_eq!(f.reads(), 2);
    assert_eq!(f.files(), ["sub/a.vi", "sub/b.vi", "sub/c.vi"]);
}

/// Incremental updates admit exactly what the initial scan admits.
#[test]
fn events_for_files_that_discovery_skips_are_ignored() {
    let f = Fixture::new("skipped");
    f.vi("a.vi", &CLEAN);
    f.vi(".hidden/pre.vi", &INLINE_CODE);
    f.state.scan();
    assert_eq!(f.files(), ["a.vi"]);

    f.vi(".hidden/h.vi", &INLINE_CODE);
    f.vi(".dot.vi", &INLINE_CODE);
    f.vi("sub/.cache/g.vi", &INLINE_CODE);
    fs::write(f.root().join(".hidden/p.lvproj"), "<Project/>").unwrap();
    write_vi(&f.dir.join("outside/o.vi"), &INLINE_CODE);
    let mut names = vec![
        ".hidden/h.vi",
        ".hidden/pre.vi",
        ".hidden/p.lvproj",
        ".hidden",
        ".dot.vi",
        "sub/.cache/g.vi",
        "sub",
        "../outside/o.vi",
        "../outside",
    ];
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(f.dir.join("outside/o.vi"), f.root().join("link.vi")).unwrap();
        symlink(f.dir.join("outside"), f.root().join("linked")).unwrap();
        names.extend(["link.vi", "linked", "linked/o.vi"]);
    }
    let outcome = f.changed(&names);
    assert_eq!(f.simplify(&outcome.updates), nothing());
    assert!(outcome.retry.is_empty());
    assert_eq!(f.files(), ["a.vi"]);
    assert_eq!(f.reads(), 0);

    // The answer is the same as starting over.
    assert_eq!(f.simplify(&f.state.scan()), nothing());
    assert_eq!(f.files(), ["a.vi"]);
}

/// A known file replaced by something discovery skips leaves the workspace.
#[cfg(unix)]
#[test]
fn file_replaced_by_a_symlink_is_dropped() {
    let f = Fixture::new("to-link");
    f.vi("a.vi", &INLINE_CODE);
    f.vi("b.vi", &CLEAN);
    f.state.scan();
    fs::remove_file(f.root().join("a.vi")).unwrap();
    std::os::unix::fs::symlink(f.root().join("b.vi"), f.root().join("a.vi")).unwrap();
    let outcome = f.changed(&["a.vi"]);
    assert_eq!(f.simplify(&outcome.updates), [update("a.vi", &[])]);
    assert_eq!(f.files(), ["b.vi"]);
}

/// A watcher may report resolved paths (FSEvents does); the file keeps the
/// name it has under the root as the client spelled it.
#[cfg(unix)]
#[test]
fn canonical_event_paths_keep_the_given_root_spelling() {
    let f = Fixture::new("canonical");
    fs::create_dir_all(f.dir.join("actual/proj")).unwrap();
    std::os::unix::fs::symlink(f.dir.join("actual"), f.dir.join("link")).unwrap();
    let given = f.dir.join("link/proj");
    let canonical = f.dir.join("actual/proj");
    write_vi(&canonical.join("util.vi"), &INLINE_CODE);
    f.state.set_roots(vec![given.clone()]);
    let scanned = f.state.scan();
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0].0, given.join("util.vi"));

    write_vi(&canonical.join("util.vi"), &CLEAN);
    write_vi(&canonical.join("new.vi"), &INLINE_CODE);
    let batch = [canonical.join("util.vi"), canonical.join("new.vi")]
        .into_iter()
        .map(|path| (path, 0))
        .collect();
    let outcome = f.state.process(f.runtime.handle(), batch);
    let updates: Vec<(PathBuf, usize)> = outcome
        .updates
        .iter()
        .map(|(path, diagnostics)| (path.clone(), diagnostics.len()))
        .collect();
    assert_eq!(
        updates,
        [(given.join("new.vi"), 1), (given.join("util.vi"), 0)]
    );
    let view = f.state.view();
    let known: Vec<&Path> = view.paths().collect();
    assert_eq!(known, [given.join("new.vi"), given.join("util.vi")]);

    fs::remove_file(canonical.join("util.vi")).unwrap();
    let batch = Batch::from([(canonical.join("util.vi"), 0)]);
    f.state.process(f.runtime.handle(), batch);
    let view = f.state.view();
    assert_eq!(view.paths().collect::<Vec<_>>(), [given.join("new.vi")]);
}

/// A file caught in the middle of a save fails to parse. That must not be
/// reported while the file is still fresh; it is read again.
#[test]
fn a_fresh_file_that_fails_to_parse_is_retried_not_reported() {
    let f = Fixture::new("retry");
    f.vi("a.vi", &INLINE_CODE);
    f.state.scan();

    f.vi("a.vi", &CLEAN);
    f.inspector.failures_left.store(2, Ordering::SeqCst);
    let first = f.changed(&["a.vi"]);
    assert_eq!(f.simplify(&first.updates), nothing());
    let path = f.root().join("a.vi");
    assert_eq!(first.retry, Batch::from([(path.clone(), 1)]));
    // Meanwhile the previous state stands.
    assert!(f.state.view().metadata(&path).is_some());

    let second = f.state.process(f.runtime.handle(), first.retry);
    assert_eq!(f.simplify(&second.updates), nothing());
    assert_eq!(second.retry, Batch::from([(path.clone(), 2)]));

    let third = f.state.process(f.runtime.handle(), second.retry);
    assert_eq!(f.simplify(&third.updates), [update("a.vi", &[])]);
    assert!(third.retry.is_empty());
}

#[test]
fn retries_are_bounded_and_skipped_for_files_at_rest() {
    let f = Fixture::new("retry-bound");
    f.vi("a.vi", &CLEAN);
    f.vi("old.vi", &CLEAN);
    f.state.scan();

    // Really broken: reported after the last retry.
    fs::write(f.root().join("a.vi"), b"RSRC").unwrap();
    let mut outcome = f.changed(&["a.vi"]);
    let mut reads = 1;
    while !outcome.retry.is_empty() {
        assert_eq!(f.simplify(&outcome.updates), nothing());
        outcome = f.state.process(f.runtime.handle(), outcome.retry);
        reads += 1;
    }
    assert_eq!(reads, 1 + RETRY_DELAYS.len());
    assert_eq!(f.simplify(&outcome.updates), [update("a.vi", &["BZ000"])]);

    // Broken and not written recently: nothing to wait for.
    let old = f.root().join("old.vi");
    fs::write(&old, b"RSRC").unwrap();
    let long_ago = SystemTime::now() - Duration::from_secs(3600);
    let file = fs::File::options().write(true).open(&old).unwrap();
    file.set_modified(long_ago).unwrap();
    drop(file);
    let outcome = f.changed(&["old.vi"]);
    assert!(outcome.retry.is_empty());
    assert_eq!(f.simplify(&outcome.updates), [update("old.vi", &["BZ000"])]);
}

/// Requests read a snapshot; they must be answered while a slow batch is
/// still being read.
#[test]
fn readers_are_not_blocked_by_a_running_batch() {
    let f = Fixture::new("readers");
    f.vi("a.vi", &INLINE_CODE);
    f.state.scan();
    let path = f.root().join("a.vi");

    let (open_gate, gate) = std_mpsc::channel();
    *f.inspector.gate.lock().unwrap() = Some(gate);
    f.vi("a.vi", &CLEAN);
    let state = f.state.clone();
    let runtime = f.runtime.handle().clone();
    let batch = Batch::from([(path.clone(), 0)]);
    let worker = std::thread::spawn(move || state.process(&runtime, batch));
    while f.reads() == 0 {
        std::thread::yield_now();
    }

    // The batch is now stuck inside the inspector. A reader on another
    // thread must still get an answer, and it is the previous state.
    let (answered, answer) = std_mpsc::channel();
    let state = f.state.clone();
    let asked = path.clone();
    std::thread::spawn(move || {
        let view = state.view();
        let _ = answered.send((view.hover(&asked), view.symbols("", 10).len()));
    });
    let (hover, symbols) = answer
        .recv_timeout(Duration::from_secs(10))
        .expect("a reader waited for the running batch");
    assert!(hover.is_some());
    assert_eq!(symbols, 1);

    open_gate.send(()).unwrap();
    let outcome = worker.join().unwrap();
    assert_eq!(f.simplify(&outcome.updates), [update("a.vi", &[])]);
}

struct Recorder {
    seen: Mutex<Vec<Update>>,
    delay: Duration,
}

#[tower_lsp::async_trait]
impl Publisher for Arc<Recorder> {
    async fn publish(&self, updates: Vec<Update>) {
        for update in updates {
            // A client that is slower than the server.
            tokio::time::sleep(self.delay).await;
            self.seen.lock().unwrap().push(update);
        }
    }
}

/// The scan and a change that arrives while the scan is still being
/// published go out in order: the last word on a file is the newest. Also:
/// `Scan` queued before `Roots` (a client that pipelines `initialize` and
/// `initialized`) still scans.
#[test]
fn publishing_is_ordered_and_scan_waits_for_roots() {
    const FILES: usize = 40;
    let f = Fixture::new("ordered");
    for i in 0..FILES {
        f.vi(&format!("f{i:02}.vi"), &INLINE_CODE);
    }
    let last = f.root().join(format!("f{:02}.vi", FILES - 1));
    let state = Arc::new(State::new(Arc::new(TestInspector::default())));
    let recorder = Arc::new(Recorder {
        seen: Mutex::new(Vec::new()),
        delay: Duration::from_millis(3),
    });
    let (jobs, queue) = mpsc::unbounded_channel();
    let worker = f
        .runtime
        .spawn(run(state.clone(), recorder.clone(), queue, jobs.clone()));
    jobs.send(Job::Scan).unwrap();
    jobs.send(Job::Roots(vec![f.root()])).unwrap();

    // Wait until the scan is being published, then fix the last file and
    // report it twice, as the watcher and the client both may.
    let deadline = Instant::now() + Duration::from_secs(30);
    while recorder.seen.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the scan was never published");
        std::thread::sleep(Duration::from_millis(1));
    }
    write_vi(&last, &CLEAN);
    jobs.send(Job::Paths(vec![last.clone()])).unwrap();
    jobs.send(Job::Paths(vec![last.clone()])).unwrap();
    loop {
        let seen = recorder.seen.lock().unwrap().clone();
        let for_last: Vec<usize> = seen
            .iter()
            .filter(|(path, _)| *path == last)
            .map(|(_, diagnostics)| diagnostics.len())
            .collect();
        if for_last.last() == Some(&0) {
            // Every file once from the scan, in sorted order, then the fix.
            assert_eq!(seen.len(), FILES + 1);
            let scanned: Vec<&PathBuf> = seen[..FILES].iter().map(|(path, _)| path).collect();
            let mut sorted = scanned.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(scanned, sorted);
            assert_eq!(for_last, [1, 0]);
            break;
        }
        assert!(Instant::now() < deadline, "saw {for_last:?}");
        std::thread::sleep(Duration::from_millis(5));
    }

    jobs.send(Job::Shutdown).unwrap();
    f.runtime.block_on(worker).unwrap();
    // Nothing is published after shutdown.
    write_vi(&last, &INLINE_CODE);
    let _ = jobs.send(Job::Paths(vec![last.clone()]));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(recorder.seen.lock().unwrap().len(), FILES + 1);
}
