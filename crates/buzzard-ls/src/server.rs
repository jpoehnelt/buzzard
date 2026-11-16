//! The `tower-lsp` front end over [`Engine`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use buzzard_lint::project::is_project_file;
use buzzard_lint::{
    summary, Diagnostic as Finding, FileState, LintConfig, RsrcInspector, Severity, ViInspector,
};
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};
use tower_lsp::jsonrpc::{Error, Request, Result};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tower_service::Service;

use crate::engine::{self, Engine};

/// How long to wait for a burst of file events to settle before re-analyzing.
const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_SYMBOLS: usize = 500;

type Watcher = Debouncer<notify::RecommendedWatcher>;

struct Backend {
    client: Client,
    engine: Arc<Mutex<Engine>>,
    inspector: Arc<dyn ViInspector>,
    watcher: Mutex<Option<Watcher>>,
}

fn lock(engine: &Mutex<Engine>) -> MutexGuard<'_, Engine> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn to_url(path: &Path) -> Option<Url> {
    let absolute = std::path::absolute(path).ok()?;
    Url::from_file_path(absolute).ok()
}

fn to_path(uri: &Url) -> Result<PathBuf> {
    uri.to_file_path()
        .map_err(|_| Error::invalid_params(format!("not a file URI: {uri}")))
}

fn file_location(path: &Path) -> Option<Location> {
    Some(Location {
        uri: to_url(path)?,
        range: Range::default(),
    })
}

fn to_lsp_diagnostic(finding: &Finding) -> Diagnostic {
    let related: Vec<DiagnosticRelatedInformation> = finding
        .related
        .iter()
        .filter_map(|path| {
            Some(DiagnosticRelatedInformation {
                location: file_location(path)?,
                message: "also involved".to_owned(),
            })
        })
        .collect();
    Diagnostic {
        range: Range::default(),
        severity: Some(match finding.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Info => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(finding.code.to_owned())),
        source: Some("buzzard".to_owned()),
        message: finding.message.clone(),
        related_information: (!related.is_empty()).then_some(related),
        ..Diagnostic::default()
    }
}

async fn publish(client: &Client, updates: Vec<engine::Update>) {
    for (path, findings) in updates {
        if let Some(uri) = to_url(&path) {
            let diagnostics = findings.iter().map(to_lsp_diagnostic).collect();
            client.publish_diagnostics(uri, diagnostics, None).await;
        }
    }
}

/// Re-inspect the given files and publish whatever diagnostics changed.
async fn refresh(
    client: &Client,
    engine: &Arc<Mutex<Engine>>,
    inspector: &Arc<dyn ViInspector>,
    paths: Vec<PathBuf>,
) {
    let mut changes = Vec::new();
    for path in paths {
        if !Engine::is_relevant(&path) {
            continue;
        }
        let state = if !path.is_file() {
            None
        } else if is_project_file(&path) {
            // Project-level XML is always read from disk; the inspector
            // backend only knows binary files.
            Some(FileState::load(&path))
        } else {
            Some(FileState::from_inspection(inspector.inspect(&path).await))
        };
        changes.push((path, state));
    }
    if changes.is_empty() {
        return;
    }
    let engine = engine.clone();
    let updates = tokio::task::spawn_blocking(move || lock(&engine).apply(changes)).await;
    publish(client, updates.unwrap_or_default()).await;
}

impl Backend {
    fn start_watcher(&self) {
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();
        let debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
            if let Ok(events) = result {
                let _ = tx.send(events.into_iter().map(|e| e.path).collect());
            }
        });
        let Ok(mut debouncer) = debouncer else { return };
        for root in lock(&self.engine).roots() {
            let _ = debouncer.watcher().watch(root, RecursiveMode::Recursive);
        }
        *self.watcher.lock().unwrap_or_else(|p| p.into_inner()) = Some(debouncer);

        let client = self.client.clone();
        let engine = self.engine.clone();
        let inspector = self.inspector.clone();
        tokio::spawn(async move {
            while let Some(paths) = rx.recv().await {
                refresh(&client, &engine, &inspector, paths).await;
            }
        });
    }

    /// `buzzard/inspect`: metadata and graph neighbours of one file as JSON.
    /// A project, library or class answers with `project` instead of
    /// `metadata`.
    async fn inspect(&self, params: InspectParams) -> Result<Value> {
        let path = to_path(&params.uri)?;
        let engine = lock(&self.engine);
        let uris =
            |paths: Vec<PathBuf>| -> Vec<Url> { paths.iter().filter_map(|p| to_url(p)).collect() };
        if let Some(project) = engine.project(&path) {
            return Ok(json!({
                "project": project,
                "members": uris(engine.members(&path)),
                "owners": uris(engine.owners(&path)),
                "parentClass": engine.parent_class(&path).and_then(|p| to_url(&p)),
                "childClasses": uris(engine.child_classes(&path)),
                "callers": uris(engine.callers(&path)),
            }));
        }
        Ok(match engine.metadata(&path) {
            Some(metadata) => json!({
                "metadata": metadata,
                "callers": uris(engine.callers(&path)),
                "callees": uris(engine.callees(&path)),
                "owners": uris(engine.owners(&path)),
            }),
            None => Value::Null,
        })
    }
}

impl Backend {
    /// `buzzard/exportAst`: the block diagram of one file as a graph of
    /// nodes, terminals and wires, for tools and AI agents. The diagram is
    /// decoded from disk on request; it is not kept in memory.
    async fn export_ast(&self, params: ExportAstParams) -> Result<Value> {
        let path = to_path(&params.uri)?;
        let markdown = match params.format.as_deref() {
            None | Some("json") => false,
            Some("markdown") => true,
            Some(other) => {
                return Err(Error::invalid_params(format!(
                    "unknown format `{other}`; use `json` or `markdown`"
                )))
            }
        };
        let inspection = self
            .inspector
            .inspect(&path)
            .await
            .map_err(|reason| Error::invalid_params(format!("cannot read file: {reason}")))?;
        Ok(match inspection.diagram {
            None => Value::Null,
            Some(diagram) if markdown => {
                json!({ "markdown": summary::diagram_to_markdown(&diagram) })
            }
            Some(diagram) => json!({ "diagram": diagram }),
        })
    }
}

#[derive(Debug, Deserialize)]
struct InspectParams {
    uri: Url,
}

#[derive(Debug, Deserialize)]
struct ExportAstParams {
    uri: Url,
    /// `"json"` (default) or `"markdown"`.
    #[serde(default)]
    format: Option<String>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .unwrap_or_default()
            .iter()
            .filter_map(|folder| folder.uri.to_file_path().ok())
            .collect();
        if roots.is_empty() {
            #[allow(deprecated)]
            roots.extend(params.root_uri.and_then(|uri| uri.to_file_path().ok()));
        }
        *lock(&self.engine) = Engine::new(roots, LintConfig::default());

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "buzzard".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Watch before scanning so that a save during the scan is not missed.
        self.start_watcher();
        let engine = self.engine.clone();
        let updates = tokio::task::spawn_blocking(move || lock(&engine).scan()).await;
        publish(&self.client, updates.unwrap_or_default()).await;
    }

    async fn shutdown(&self) -> Result<()> {
        self.watcher
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        Ok(())
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let paths = params
            .changes
            .iter()
            .filter_map(|c| c.uri.to_file_path().ok())
            .collect();
        refresh(&self.client, &self.engine, &self.inspector, paths).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let path = to_path(&params.text_document_position_params.text_document.uri)?;
        Ok(lock(&self.engine).hover(&path).map(|value| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: None,
        }))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let path = to_path(&params.text_document_position.text_document.uri)?;
        let callers = lock(&self.engine).callers(&path);
        Ok(Some(
            callers.iter().filter_map(|p| file_location(p)).collect(),
        ))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let symbols = lock(&self.engine).symbols(&params.query, MAX_SYMBOLS);
        let out = symbols
            .into_iter()
            .filter_map(|symbol| {
                #[allow(deprecated)]
                Some(SymbolInformation {
                    kind: match symbol.kind {
                        engine::SymbolKind::Vi => SymbolKind::FUNCTION,
                        engine::SymbolKind::Control => SymbolKind::STRUCT,
                        engine::SymbolKind::Class => SymbolKind::CLASS,
                        engine::SymbolKind::Library => SymbolKind::MODULE,
                        engine::SymbolKind::Project => SymbolKind::PACKAGE,
                    },
                    location: file_location(&symbol.path)?,
                    name: symbol.name,
                    tags: None,
                    deprecated: None,
                    container_name: None,
                })
            })
            .collect();
        Ok(Some(out))
    }
}

/// Wraps the LSP service to signal when the client sends `exit`. The
/// underlying server otherwise keeps running until stdin is closed.
struct ExitAware<S> {
    inner: S,
    exit: Arc<Notify>,
}

impl<S> Service<Request> for ExitAware<S>
where
    S: Service<Request>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = std::result::Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::result::Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let is_exit = request.method() == "exit";
        let response = self.inner.call(request);
        let exit = self.exit.clone();
        Box::pin(async move {
            let response = response.await;
            if is_exit {
                exit.notify_one();
            }
            response
        })
    }
}

/// Serve the language server over stdin/stdout until the client sends
/// `exit` or closes the connection.
pub async fn run_stdio() {
    let (service, socket) = LspService::build(|client| Backend {
        client,
        engine: Arc::new(Mutex::new(Engine::default())),
        inspector: Arc::new(RsrcInspector),
        watcher: Mutex::new(None),
    })
    .custom_method("buzzard/inspect", Backend::inspect)
    .custom_method("buzzard/exportAst", Backend::export_ast)
    .finish();
    let exit = Arc::new(Notify::new());
    let service = ExitAware {
        inner: service,
        exit: exit.clone(),
    };
    let server = Server::new(tokio::io::stdin(), tokio::io::stdout(), socket);
    tokio::select! {
        () = server.serve(service) => {}
        () = exit.notified() => {}
    }
}
