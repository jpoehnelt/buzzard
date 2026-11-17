//! The `tower-lsp` front end over the analyzed state.

use std::future::{ready, Future, Ready};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use buzzard_lint::{summary, Diagnostic as Finding, RsrcInspector, Severity, ViInspector};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::Notify;
use tower_lsp::jsonrpc::{Error, Request, Response, Result};
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DidChangeWatchedFilesParams,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MarkupContent, MarkupKind, NumberOrString, OneOf, Range,
    ReferenceParams, ServerCapabilities, ServerInfo, SymbolInformation, SymbolKind, Url,
    WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tower_service::Service;

use crate::engine;
use crate::worker::{self, Job, Publisher, State};

const MAX_SYMBOLS: usize = 500;

/// The protocol front end. It never changes the analyzed state itself:
/// handlers read the current snapshot, and anything that may change the
/// state is queued for the worker (see [`crate::worker`]).
struct Backend {
    state: Arc<State>,
    jobs: UnboundedSender<Job>,
    inspector: Arc<dyn ViInspector>,
}

fn to_url(path: &Path) -> Option<Url> {
    let absolute = std::path::absolute(path).ok()?;
    Url::from_file_path(absolute).ok()
}

fn to_path(uri: &Url) -> Result<PathBuf> {
    uri.to_file_path()
        .map_err(|()| Error::invalid_params(format!("not a file URI: {uri}")))
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

/// Publishes to the client, one file after the other, in the order given.
struct ClientPublisher(Client);

#[tower_lsp::async_trait]
impl Publisher for ClientPublisher {
    async fn publish(&self, updates: Vec<engine::Update>) {
        for (path, findings) in updates {
            if let Some(uri) = to_url(&path) {
                let diagnostics = findings.iter().map(to_lsp_diagnostic).collect();
                self.0.publish_diagnostics(uri, diagnostics, None).await;
            }
        }
    }
}

impl Backend {
    /// `buzzard/inspect`: metadata and graph neighbours of one file as JSON.
    /// A project, library or class answers with `project` instead of
    /// `metadata`.
    ///
    /// tower-lsp wants a future, but the answer comes straight from the
    /// current snapshot, so the future is ready at once.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "tower-lsp passes the deserialized params by value"
    )]
    fn inspect(&self, params: InspectParams) -> Ready<Result<Value>> {
        ready(self.inspect_now(&params.uri))
    }

    fn inspect_now(&self, uri: &Url) -> Result<Value> {
        let path = to_path(uri)?;
        let engine = self.state.view();
        // A member of an archive is shown as the archive: that is the
        // file a client can open.
        let uris = |paths: Vec<PathBuf>| -> Vec<Url> {
            let mut out: Vec<Url> = paths
                .iter()
                .filter_map(|p| to_url(&engine.document(p)))
                .collect();
            out.dedup();
            out
        };
        if let Some(archive) = engine.archive(&path) {
            return Ok(json!({
                "archive": archive,
                "opened": archive.opened(),
            }));
        }
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
        // Reading and decoding a file must not stall the other requests.
        let inspector = Arc::clone(&self.inspector);
        let runtime = tokio::runtime::Handle::current();
        let inspection =
            tokio::task::spawn_blocking(move || runtime.block_on(inspector.inspect(&path)))
                .await
                .map_err(|_| Error::internal_error())?
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
        // `rootUri` is superseded by workspace folders; it is the fallback
        // for clients that send none.
        if roots.is_empty() {
            roots.extend(params.root_uri.and_then(|uri| uri.to_file_path().ok()));
        }
        let _ = self.jobs.send(Job::Roots(roots));

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
        let _ = self.jobs.send(Job::Scan);
    }

    async fn shutdown(&self) -> Result<()> {
        // Stops the file watcher and ends the worker.
        let _ = self.jobs.send(Job::Shutdown);
        Ok(())
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let paths = params
            .changes
            .iter()
            .filter_map(|c| c.uri.to_file_path().ok())
            .collect();
        let _ = self.jobs.send(Job::Paths(paths));
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let path = to_path(&params.text_document_position_params.text_document.uri)?;
        Ok(self.state.view().hover(&path).map(|value| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: None,
        }))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let path = to_path(&params.text_document_position.text_document.uri)?;
        let view = self.state.view();
        let mut callers: Vec<PathBuf> = view
            .callers(&path)
            .iter()
            .map(|caller| view.document(caller))
            .collect();
        callers.dedup();
        Ok(Some(
            callers.iter().filter_map(|p| file_location(p)).collect(),
        ))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let view = self.state.view();
        let symbols = view.symbols(&params.query, MAX_SYMBOLS);
        let out = symbols
            .into_iter()
            .filter_map(|symbol| {
                #[expect(
                    deprecated,
                    reason = "`SymbolInformation::deprecated` is superseded by `tags`, but a struct literal must still set it"
                )]
                Some(SymbolInformation {
                    kind: match symbol.kind {
                        engine::SymbolKind::Vi => SymbolKind::FUNCTION,
                        engine::SymbolKind::Control => SymbolKind::STRUCT,
                        engine::SymbolKind::Class => SymbolKind::CLASS,
                        engine::SymbolKind::Library => SymbolKind::MODULE,
                        engine::SymbolKind::Project => SymbolKind::PACKAGE,
                    },
                    // A member of an archive is found in the archive.
                    location: file_location(&view.document(&symbol.path))?,
                    container_name: view.is_member(&symbol.path).then(|| {
                        let archive = view.document(&symbol.path);
                        archive
                            .file_name()
                            .unwrap_or(archive.as_os_str())
                            .to_string_lossy()
                            .into_owned()
                    }),
                    name: symbol.name,
                    tags: None,
                    deprecated: None,
                })
            })
            .collect();
        Ok(Some(out))
    }
}

/// What the client has said about ending the session.
#[derive(Default)]
struct Lifecycle {
    /// A `shutdown` request was answered successfully.
    shutdown: AtomicBool,
    exit: Notify,
}

impl Lifecycle {
    /// The process status the protocol asks for after `exit`: 0 if the
    /// client sent `shutdown` first, else 1.
    fn status_after_exit(&self) -> i32 {
        i32::from(!self.shutdown.load(Ordering::SeqCst))
    }
}

/// Wraps the LSP service to follow the handshakes at both ends of a
/// session, where the underlying server does not tell enough:
///
/// - `shutdown` / `exit`: it keeps running until stdin is closed, and does
///   not say whether `shutdown` came first.
/// - `initialize` / `initialized`: it drops an `initialized` that arrives
///   before `initialize` has been answered, which a client that sends both
///   in one write can cause. The workspace would never be scanned.
struct Handshake<S> {
    inner: S,
    lifecycle: Arc<Lifecycle>,
    jobs: UnboundedSender<Job>,
}

impl<S> Service<Request> for Handshake<S>
where
    S: Service<Request, Response = Option<Response>>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = std::result::Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::result::Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let is_exit = req.method() == "exit";
        let is_shutdown = req.method() == "shutdown";
        if req.method() == "initialized" {
            // The worker waits for the roots and scans once, however often
            // it is asked.
            let _ = self.jobs.send(Job::Scan);
        }
        let response = self.inner.call(req);
        let lifecycle = Arc::clone(&self.lifecycle);
        Box::pin(async move {
            let response = response.await;
            // A `shutdown` that was refused (sent before `initialize`, or
            // twice) does not count.
            if is_shutdown && matches!(&response, Ok(Some(answer)) if answer.is_ok()) {
                lifecycle.shutdown.store(true, Ordering::SeqCst);
            }
            if is_exit {
                lifecycle.exit.notify_one();
            }
            response
        })
    }
}

/// Serve the language server over stdin/stdout until the client sends
/// `exit` or closes the connection.
///
/// Returns the status the process should exit with: 1 if `exit` arrived
/// without a preceding `shutdown`, as the protocol requires, and 0
/// otherwise (including when the client just closes the connection).
pub async fn run_stdio() -> i32 {
    let inspector: Arc<dyn ViInspector> = Arc::new(RsrcInspector);
    let state = Arc::new(State::new(Arc::clone(&inspector)));
    let (jobs, queue) = mpsc::unbounded_channel();
    let (service, socket) = LspService::build(|client| {
        tokio::spawn(worker::run(
            Arc::clone(&state),
            ClientPublisher(client),
            queue,
            jobs.clone(),
        ));
        Backend {
            state,
            jobs: jobs.clone(),
            inspector,
        }
    })
    .custom_method("buzzard/inspect", Backend::inspect)
    .custom_method("buzzard/exportAst", Backend::export_ast)
    .finish();
    let lifecycle = Arc::new(Lifecycle::default());
    let service = Handshake {
        inner: service,
        lifecycle: Arc::clone(&lifecycle),
        jobs,
    };
    let server = Server::new(tokio::io::stdin(), tokio::io::stdout(), socket);
    tokio::select! {
        () = server.serve(service) => 0,
        () = lifecycle.exit.notified() => lifecycle.status_after_exit(),
    }
}
