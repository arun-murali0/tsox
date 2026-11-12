//! The language server: the tower service that speaks LSP and drives the worker.
//!
//! Ordering is the property everything else rests on. The transport may run up to four
//! handlers at once, so the notification handlers here (`didOpen`, `didChange`, `didClose`)
//! do all their work before their first await: they are plain synchronous code in an async
//! signature, and a future that never yields finishes on its first poll, in arrival order.
//! Nothing is ever sent to the client from a handler directly. Every message that goes out
//! (diagnostics, logs) is pushed onto one queue, under the lock that guards the documents,
//! and a single task sends them. The order of the queue is then the order in which the
//! document state changed, so a late diagnostic can never overtake the clearing of a
//! closed document.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, FileSystemWatcher, GlobPattern, InitializeParams, InitializeResult,
    InitializedParams, MessageType, Registration, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, Uri,
};
use tower_lsp_server::ls_types::{
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, Location, MarkupContent,
    MarkupKind, OneOf, ReferenceParams,
};
use tower_lsp_server::{Client, LanguageServer, LspService};

use crate::completion::{self, Kind as CompletionKind};
use crate::hover as hover_text;
use crate::navigate::{self, Access, Hit, Host, RenameError, Snapshot};
use crate::paths::{path_to_uri, uri_to_path};
use crate::signature;
use crate::structure::{self, FoldKind};
use crate::symbols;
use crate::text::{Document, LineMap};
use crate::worker::{Command, Event, FileUpdate, Queries, Worker, find_tsconfig};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionOptions, CompletionParams,
    CompletionResponse, Documentation, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureHelpOptions, SignatureHelpParams, SignatureInformation,
};
use tower_lsp_server::ls_types::{
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, FoldingRange,
    FoldingRangeKind, FoldingRangeParams, FoldingRangeProviderCapability, PrepareRenameResponse,
    RenameOptions, RenameParams, SelectionRange, SelectionRangeParams,
    SelectionRangeProviderCapability, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};

/// What goes to the client, in the order it goes.
enum Outgoing {
    Diagnostics {
        uri: Uri,
        diagnostics: Vec<tower_lsp_server::ls_types::Diagnostic>,
        version: Option<i32>,
    },
    Log {
        kind: MessageType,
        message: String,
    },
}

struct Open {
    uri: Uri,
    document: Document,
}

struct Inner {
    /// The open documents, by the path the project knows them by.
    documents: Mutex<HashMap<PathBuf, Open>>,
    worker: Mutex<Option<Worker>>,
    outbox: UnboundedSender<Outgoing>,
    /// The workspace folder, as the project finds its `tsconfig.json`.
    root: Mutex<Option<PathBuf>>,
    /// Resolvers of import specifiers, one per `tsconfig.json` (or none). They cache what
    /// they read from disk, so they are kept until a watched file changes.
    resolvers: Mutex<HashMap<Option<PathBuf>, Arc<tsox::ModuleResolver>>>,
}

impl Inner {
    fn documents(&self) -> MutexGuard<'_, HashMap<PathBuf, Open>> {
        self.documents.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, command: Command) {
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            worker.send(command);
        }
    }

    /// The resolver for files under the `tsconfig.json` the project uses for `from`: the
    /// workspace's, else the nearest one above it.
    fn resolver_for(&self, from: &Path) -> Arc<tsox::ModuleResolver> {
        let root = self.root.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let config = root
            .map(|root| root.join("tsconfig.json"))
            .filter(|candidate| candidate.is_file())
            .or_else(|| from.parent().and_then(find_tsconfig));
        let mut resolvers = self.resolvers.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(resolvers.entry(config.clone()).or_insert_with(|| {
            Arc::new(match config {
                Some(config) => tsox::ModuleResolver::with_tsconfig(config),
                None => tsox::ModuleResolver::new(),
            })
        }))
    }

    /// The URI to give a client for a file: the one the document was opened with, else one
    /// made from the path.
    fn uri_for(&self, path: &Path) -> Option<Uri> {
        match self.documents().get(path) {
            Some(open) => Some(open.uri.clone()),
            None => path_to_uri(path),
        }
    }

    /// The project's files and every open document: the places a search looks in.
    fn search_files(&self) -> Vec<PathBuf> {
        let mut files = self
            .queries()
            .and_then(|queries| queries.project_files())
            .unwrap_or_default();
        files.extend(self.documents().keys().cloned());
        files.sort();
        files.dedup();
        files
    }

    fn queries(&self) -> Option<Queries> {
        self.worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(Worker::queries)
    }

    fn log(&self, kind: MessageType, message: impl Into<String>) {
        let _ = self.outbox.send(Outgoing::Log {
            kind,
            message: message.into(),
        });
    }

    /// Turns what a check found into what the client is told. An update for a document that
    /// was closed is dropped. One for a document that has changed since is dropped too, and
    /// the worker is told to report that document afresh, because it has recorded the update
    /// as sent.
    fn apply_event(&self, event: Event) {
        match event {
            Event::Error(message) => self.log(MessageType::ERROR, message),
            Event::Diagnostics { files, stats } => {
                tracing::debug!(
                    checked = stats.units_checked,
                    reused = stats.units_reused,
                    reported = files.len(),
                    "check finished"
                );
                let documents = self.documents();
                let mut stale = Vec::new();
                for FileUpdate {
                    path,
                    version,
                    diagnostics,
                } in files
                {
                    let Some(open) = documents.get(&path) else {
                        continue;
                    };
                    if open.document.version != version {
                        stale.push(path);
                        continue;
                    }
                    let _ = self.outbox.send(Outgoing::Diagnostics {
                        uri: open.uri.clone(),
                        diagnostics,
                        version: Some(version),
                    });
                }
                drop(documents);
                for path in stale {
                    self.send(Command::Forget { path });
                }
            }
        }
    }
}

/// Files as navigation reads them: open documents as they are now, the rest from disk, and
/// specifiers resolved as the project resolves them.
struct ServerHost(Arc<Inner>);

impl Host for ServerHost {
    fn text(&self, path: &Path) -> Option<Arc<str>> {
        if let Some(open) = self.0.documents().get(path) {
            return Some(Arc::from(open.document.text.as_str()));
        }
        std::fs::read_to_string(path).ok().map(Arc::from)
    }

    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        let resolver = self.0.resolver_for(from);
        // Declaration files first: a jump into a package should land on its types.
        resolver
            .resolve_dts_from_file(from, specifier)
            .or_else(|_| resolver.resolve_from_file(from, specifier))
            .ok()
    }
}

/// Stops a search when the request that asked for it is dropped (the client cancelled).
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The places a set of hits are, as the client reads them.
fn locations(inner: &Inner, host: &dyn Host, hits: Vec<Hit>) -> Vec<Location> {
    let mut maps: HashMap<PathBuf, Option<(Arc<str>, LineMap)>> = HashMap::new();
    let mut out = Vec::new();
    for hit in hits {
        let entry = maps.entry(hit.path.clone()).or_insert_with(|| {
            let text = host.text(&hit.path)?;
            let map = LineMap::new(&text);
            Some((text, map))
        });
        let Some((text, map)) = entry else {
            continue;
        };
        let Some(uri) = inner.uri_for(&hit.path) else {
            continue;
        };
        out.push(Location {
            uri,
            range: map.range(text, hit.span.0 as usize, hit.span.1 as usize),
        });
    }
    out
}

pub struct Backend {
    inner: Arc<Inner>,
    client: Client,
    // Taken by `initialize`, which starts the tasks that use it.
    outbox_rx: Mutex<Option<UnboundedReceiver<Outgoing>>>,
    // Whether the client said it takes file watchers registered after startup. A server
    // must not ask for a registration the client did not declare support for.
    dynamic_watchers: AtomicBool,
    // Whether the client reads the nested form of an outline; if not, the flat one is sent.
    hierarchical_symbols: AtomicBool,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        let (outbox, outbox_rx) = unbounded_channel();
        Self {
            inner: Arc::new(Inner {
                documents: Mutex::new(HashMap::new()),
                worker: Mutex::new(None),
                outbox,
                root: Mutex::new(None),
                resolvers: Mutex::new(HashMap::new()),
            }),
            client,
            outbox_rx: Mutex::new(Some(outbox_rx)),
            dynamic_watchers: AtomicBool::new(false),
            hierarchical_symbols: AtomicBool::new(false),
        }
    }
}

/// The service and the socket to serve it on.
pub fn service() -> (LspService<Backend>, tower_lsp_server::ClientSocket) {
    LspService::new(Backend::new)
}

/// The workspace folder: the first folder the client gave, else its root URI.
#[allow(deprecated)]
fn workspace_root(params: &InitializeParams) -> Option<PathBuf> {
    params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| &folder.uri)
        .or(params.root_uri.as_ref())
        .and_then(uri_to_path)
}

/// The files whose changes on disk can change a check.
const WATCHED: &[&str] = &[
    "**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}",
    "**/tsconfig*.json",
    "**/jsconfig*.json",
    "**/package.json",
];

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let root = workspace_root(&params);
        *self.inner.root.lock().unwrap_or_else(|e| e.into_inner()) = root.clone();
        let dynamic = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|watched| watched.dynamic_registration)
            .unwrap_or(false);
        self.dynamic_watchers.store(dynamic, Ordering::Relaxed);
        let hierarchical = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|document| document.document_symbol.as_ref())
            .and_then(|symbols| symbols.hierarchical_document_symbol_support)
            .unwrap_or(false);
        self.hierarchical_symbols
            .store(hierarchical, Ordering::Relaxed);

        let (events_tx, mut events_rx) = unbounded_channel();
        let worker = Worker::spawn(root, events_tx).map_err(|error| {
            let mut failure = tower_lsp_server::jsonrpc::Error::internal_error();
            failure.message = format!("could not start the checker thread: {error}").into();
            failure
        })?;
        *self.inner.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(worker);

        // Worker events become outgoing messages.
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            while let Some(event) = events_rx.recv().await {
                inner.apply_event(event);
            }
        });

        // The one task that talks to the client.
        let outbox_rx = self
            .outbox_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mut outbox_rx) = outbox_rx {
            let client = self.client.clone();
            tokio::spawn(async move {
                while let Some(message) = outbox_rx.recv().await {
                    match message {
                        Outgoing::Diagnostics {
                            uri,
                            diagnostics,
                            version,
                        } => client.publish_diagnostics(uri, diagnostics, version).await,
                        Outgoing::Log { kind, message } => client.log_message(kind, message).await,
                    }
                }
            });
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        ..Default::default()
                    },
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(
                        [".", "\"", "'", "/"]
                            .iter()
                            .map(|c| (*c).to_owned())
                            .collect(),
                    ),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                document_highlight_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "tsox-lsp".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Files that change on disk are the client's to watch. A client that cannot register
        // watchers still works; a file added outside the editor is found at the next change
        // of a document.
        if !self.dynamic_watchers.load(Ordering::Relaxed) {
            return;
        }
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: WATCHED
                .iter()
                .map(|pattern| FileSystemWatcher {
                    glob_pattern: GlobPattern::String((*pattern).to_owned()),
                    kind: None,
                })
                .collect(),
        };
        let registration = Registration {
            id: "tsox-watched-files".to_owned(),
            method: "workspace/didChangeWatchedFiles".to_owned(),
            register_options: serde_json::to_value(options).ok(),
        };
        // Not awaited here: the client answers when it likes, and a handler that waits on
        // that answer would hold up `exit` for as long as the client takes.
        let client = self.client.clone();
        tokio::spawn(async move {
            if let Err(error) = client.register_capability(vec![registration]).await {
                tracing::debug!(%error, "the client did not take the file watchers");
            }
        });
    }

    async fn shutdown(&self) -> Result<()> {
        let worker = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(worker) = worker {
            // Stopping joins the thread, which may be inside a check.
            let _ = tokio::task::spawn_blocking(move || worker.shutdown()).await;
        }
        self.inner.documents().clear();
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let item = params.text_document;
        let Some(path) = uri_to_path(&item.uri) else {
            tracing::debug!(uri = item.uri.as_str(), "not a file document; ignored");
            return;
        };
        let text: Arc<str> = Arc::from(item.text.as_str());
        let document = Document::new(item.version, item.text);
        self.inner.documents().insert(
            path.clone(),
            Open {
                uri: item.uri,
                document,
            },
        );
        self.inner.send(Command::Buffer {
            path,
            version: item.version,
            text,
        });
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let identifier = params.text_document;
        let Some(path) = uri_to_path(&identifier.uri) else {
            return;
        };
        let (version, text) = {
            let mut documents = self.inner.documents();
            let Some(open) = documents.get_mut(&path) else {
                drop(documents);
                self.inner.log(
                    MessageType::WARNING,
                    format!(
                        "change for a document that is not open: {}",
                        identifier.uri.as_str()
                    ),
                );
                return;
            };
            open.document
                .apply_changes(identifier.version, params.content_changes);
            (
                open.document.version,
                Arc::<str>::from(open.document.text.as_str()),
            )
        };
        self.inner.send(Command::Buffer {
            path,
            version,
            text,
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let Some(path) = uri_to_path(&params.text_document.uri) else {
            return;
        };
        // The client keeps showing the last diagnostics of a closed document until told
        // otherwise. Queued under the lock, so no report of the document can follow it.
        let mut documents = self.inner.documents();
        let Some(open) = documents.remove(&path) else {
            return;
        };
        let _ = self.inner.outbox.send(Outgoing::Diagnostics {
            uri: open.uri,
            diagnostics: Vec::new(),
            version: None,
        });
        drop(documents);
        self.inner.send(Command::Close { path });
    }

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {
        // A changed `tsconfig.json`, `package.json` or added file changes what a specifier
        // names; the resolvers' caches are not to be trusted past it.
        self.inner
            .resolvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.inner.send(Command::FilesChanged);
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let Some(path) = uri_to_path(&uri) else {
            return Ok(None);
        };
        // The text as it is now; the outline is read from this copy, off the lock and off
        // the async threads. The file name picks the dialect (`.tsx`, `.js`, ...).
        let Some(text) = self
            .inner
            .documents()
            .get(&path)
            .map(|open| open.document.text.clone())
        else {
            return Ok(None);
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let hierarchical = self.hierarchical_symbols.load(Ordering::Relaxed);
        let answer = tokio::task::spawn_blocking(move || {
            // The parser recurses; the checker's deep stack is the one that is safe for it.
            tsox::with_deep_stack(move || {
                let found = symbols::outline(&text, &name)?;
                let map = LineMap::new(&text);
                Some(if hierarchical {
                    DocumentSymbolResponse::Nested(symbols::to_nested(&found, &text, &map))
                } else {
                    DocumentSymbolResponse::Flat(symbols::to_flat(&found, &text, &map, &uri))
                })
            })
        })
        .await;
        match answer {
            Ok(response) => Ok(response),
            Err(error) => {
                tracing::error!(%error, "the outline failed");
                Err(tower_lsp_server::jsonrpc::Error::internal_error())
            }
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        let Some((path, text, offset, _version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let inner = Arc::clone(&self.inner);
        let answer = tokio::task::spawn_blocking(move || {
            tsox::with_deep_stack(move || {
                let host = ServerHost(Arc::clone(&inner));
                let snapshot = Snapshot::new(&host);
                let hits = navigate::definition(&snapshot, &path, &text, offset);
                locations(&inner, &snapshot, hits)
            })
        })
        .await;
        match answer {
            Ok(found) if found.is_empty() => Ok(None),
            Ok(found) => Ok(Some(GotoDefinitionResponse::Array(found))),
            Err(error) => {
                tracing::error!(%error, "definition failed");
                Err(tower_lsp_server::jsonrpc::Error::internal_error())
            }
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let position = params.text_document_position_params;
        let Some((path, text, offset, version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let inner = Arc::clone(&self.inner);
        let answer = tokio::task::spawn_blocking(move || {
            tsox::with_deep_stack(move || {
                let host = ServerHost(Arc::clone(&inner));
                let snapshot = Snapshot::new(&host);
                // The checker's type at a place, from the project as the worker last
                // finished checking it. Nothing is waited for beyond that check.
                let queries = inner.queries();
                let type_at = |at: u32| {
                    queries
                        .as_ref()
                        .and_then(|q| q.type_at(&path, at, Some(version)))
                };
                let found = hover_text::hover(&snapshot, &path, &text, offset, &type_at)?;
                let map = LineMap::new(&text);
                Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: found.markdown,
                    }),
                    range: Some(map.range(&text, found.range.0 as usize, found.range.1 as usize)),
                })
            })
        })
        .await;
        match answer {
            Ok(found) => Ok(found),
            Err(error) => {
                tracing::error!(%error, "hover failed");
                Err(tower_lsp_server::jsonrpc::Error::internal_error())
            }
        }
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let position = params.text_document_position;
        let include_declaration = params.context.include_declaration;
        let Some((path, text, offset, _version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let inner = Arc::clone(&self.inner);
        // The search reads other files; a client that cancels the request stops it.
        let cancel = Arc::new(AtomicBool::new(false));
        let _stop = CancelOnDrop(Arc::clone(&cancel));
        let answer = tokio::task::spawn_blocking(move || {
            tsox::with_deep_stack(move || {
                // The project's files, and every open document: one may not be part of the
                // project yet.
                let mut files = inner
                    .queries()
                    .and_then(|queries| queries.project_files())
                    .unwrap_or_default();
                files.extend(inner.documents().keys().cloned());
                files.sort();
                files.dedup();
                let host = ServerHost(Arc::clone(&inner));
                let snapshot = Snapshot::new(&host);
                let hits = navigate::references(
                    &snapshot,
                    &files,
                    &path,
                    &text,
                    offset,
                    include_declaration,
                    &cancel,
                );
                locations(&inner, &snapshot, hits)
            })
        })
        .await;
        match answer {
            Ok(found) if found.is_empty() => Ok(None),
            Ok(found) => Ok(Some(found)),
            Err(error) => {
                tracing::error!(%error, "references failed");
                Err(tower_lsp_server::jsonrpc::Error::internal_error())
            }
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let position = params.text_document_position;
        let Some((path, text, offset, _version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let items = self
            .blocking(move |inner| {
                let host = ServerHost(inner);
                let snapshot = Snapshot::new(&host);
                completion::completion(&snapshot, &path, &text, offset)
                    .into_iter()
                    .map(|item| CompletionItem {
                        label: item.label,
                        kind: Some(match item.kind {
                            CompletionKind::Function => CompletionItemKind::FUNCTION,
                            CompletionKind::Class => CompletionItemKind::CLASS,
                            CompletionKind::Interface => CompletionItemKind::INTERFACE,
                            CompletionKind::TypeAlias => CompletionItemKind::TYPE_PARAMETER,
                            CompletionKind::Enum => CompletionItemKind::ENUM,
                            CompletionKind::EnumMember => CompletionItemKind::ENUM_MEMBER,
                            CompletionKind::Variable => CompletionItemKind::VARIABLE,
                            CompletionKind::Constant => CompletionItemKind::CONSTANT,
                            CompletionKind::Namespace | CompletionKind::Module => {
                                CompletionItemKind::MODULE
                            }
                            CompletionKind::Property => CompletionItemKind::PROPERTY,
                            CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                            CompletionKind::File => CompletionItemKind::FILE,
                            CompletionKind::Folder => CompletionItemKind::FOLDER,
                        }),
                        detail: item.detail,
                        sort_text: Some(item.sort),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>()
            })
            .await?;
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: false,
            items,
        })))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let position = params.text_document_position_params;
        let Some((path, text, offset, version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let found = self
            .blocking(move |inner| {
                let host = ServerHost(Arc::clone(&inner));
                let snapshot = Snapshot::new(&host);
                let queries = inner.queries();
                let type_at = |at: u32| {
                    queries
                        .as_ref()
                        .and_then(|q| q.type_at(&path, at, Some(version)))
                };
                signature::signature_help(&snapshot, &path, &text, offset, &type_at)
            })
            .await?;
        Ok(found.map(|signature| {
            let label = signature.label;
            let parameters: Vec<ParameterInformation> = signature
                .parameters
                .iter()
                .map(|(start, end)| ParameterInformation {
                    label: ParameterLabel::LabelOffsets([*start, *end]),
                    documentation: None,
                })
                .collect();
            SignatureHelp {
                signatures: vec![SignatureInformation {
                    label,
                    documentation: signature.documentation.map(|value| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value,
                        })
                    }),
                    parameters: Some(parameters),
                    active_parameter: Some(signature.active_parameter),
                }],
                active_signature: Some(0),
                active_parameter: Some(signature.active_parameter),
            }
        }))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let position = params.text_document_position_params;
        let Some((path, text, offset, _version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let found = self
            .blocking(move |_| {
                let map = LineMap::new(&text);
                navigate::highlights(&path, &text, offset)
                    .into_iter()
                    .map(|((start, end), access)| DocumentHighlight {
                        range: map.range(&text, start as usize, end as usize),
                        kind: Some(match access {
                            Access::Read => DocumentHighlightKind::READ,
                            Access::Write => DocumentHighlightKind::WRITE,
                        }),
                    })
                    .collect::<Vec<_>>()
            })
            .await?;
        Ok((!found.is_empty()).then_some(found))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let Some((path, text)) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        let name = file_name_of(&path);
        let folds = self
            .blocking(move |_| structure::folding(&text, &name))
            .await?;
        Ok(Some(
            folds
                .into_iter()
                .map(|fold| FoldingRange {
                    start_line: fold.start_line,
                    start_character: None,
                    end_line: fold.end_line,
                    end_character: None,
                    kind: match fold.kind {
                        FoldKind::Comment => Some(FoldingRangeKind::Comment),
                        FoldKind::Imports => Some(FoldingRangeKind::Imports),
                        FoldKind::Region => Some(FoldingRangeKind::Region),
                        FoldKind::Code => None,
                    },
                    collapsed_text: None,
                })
                .collect(),
        ))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let Some((path, text)) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        let name = file_name_of(&path);
        let positions = params.positions;
        let ranges = self
            .blocking(move |_| {
                let map = LineMap::new(&text);
                positions
                    .iter()
                    .map(|position| {
                        let offset = map.offset(&text, *position);
                        let chain = structure::selection_chain(&text, &name, offset);
                        // The whole file is the outermost; each smaller range's parent is
                        // the next larger.
                        let mut parent: Option<Box<SelectionRange>> = None;
                        for (start, end) in chain.into_iter().rev() {
                            parent = Some(Box::new(SelectionRange {
                                range: map.range(&text, start as usize, end as usize),
                                parent,
                            }));
                        }
                        *parent.expect("a chain always ends with the whole file")
                    })
                    .collect::<Vec<_>>()
            })
            .await?;
        Ok(Some(ranges))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let cancel = Arc::new(AtomicBool::new(false));
        let _stop = CancelOnDrop(Arc::clone(&cancel));
        let query = params.query;
        let found = self
            .blocking(move |inner| {
                let files = inner.search_files();
                let host = ServerHost(Arc::clone(&inner));
                let snapshot = Snapshot::new(&host);
                let found = symbols::workspace_symbols(&snapshot, &files, &query, 500, &cancel);
                let mut maps: HashMap<PathBuf, Option<(Arc<str>, LineMap)>> = HashMap::new();
                let mut out = Vec::new();
                for item in found {
                    let entry = maps.entry(item.path.clone()).or_insert_with(|| {
                        let text = snapshot.text(&item.path)?;
                        let map = LineMap::new(&text);
                        Some((text, map))
                    });
                    let (Some((text, map)), Some(uri)) = (entry, inner.uri_for(&item.path)) else {
                        continue;
                    };
                    let start = item.symbol.range.0.min(item.symbol.selection.0);
                    let end = item.symbol.range.1.max(item.symbol.selection.1);
                    #[allow(deprecated)]
                    out.push(tower_lsp_server::ls_types::SymbolInformation {
                        name: item.symbol.name.clone(),
                        kind: item.symbol.kind,
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri,
                            range: map.range(text, start as usize, end as usize),
                        },
                        container_name: item.container.clone(),
                    });
                }
                out
            })
            .await?;
        Ok(Some(WorkspaceSymbolResponse::Flat(found)))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let Some((path, text, offset, _version)) =
            self.cursor(&params.text_document.uri, params.position)
        else {
            return Ok(None);
        };
        let outcome = self
            .blocking(move |inner| {
                let host = ServerHost(inner);
                let snapshot = Snapshot::new(&host);
                navigate::prepare_rename(&snapshot, &path, &text, offset).map(|prepared| {
                    let map = LineMap::new(&text);
                    PrepareRenameResponse::RangeWithPlaceholder {
                        range: map.range(&text, prepared.span.0 as usize, prepared.span.1 as usize),
                        placeholder: prepared.name,
                    }
                })
            })
            .await?;
        match outcome {
            Ok(response) => Ok(Some(response)),
            // Nothing there to rename is an empty answer, not a failure.
            Err(RenameError::Nothing) => Ok(None),
            Err(error) => Err(tower_lsp_server::jsonrpc::Error::invalid_params(
                error.to_string(),
            )),
        }
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let position = params.text_document_position;
        let Some((path, text, offset, _version)) =
            self.cursor(&position.text_document.uri, position.position)
        else {
            return Ok(None);
        };
        let new_name = params.new_name;
        let cancel = Arc::new(AtomicBool::new(false));
        let _stop = CancelOnDrop(Arc::clone(&cancel));
        let outcome = self
            .blocking(move |inner| {
                let files = inner.search_files();
                let host = ServerHost(Arc::clone(&inner));
                let snapshot = Snapshot::new(&host);
                let edits =
                    navigate::rename(&snapshot, &files, &path, &text, offset, &new_name, &cancel)?;
                let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
                let mut maps: HashMap<PathBuf, Option<(Arc<str>, LineMap)>> = HashMap::new();
                for edit in edits {
                    let entry = maps.entry(edit.path.clone()).or_insert_with(|| {
                        let text = snapshot.text(&edit.path)?;
                        let map = LineMap::new(&text);
                        Some((text, map))
                    });
                    let (Some((file_text, map)), Some(uri)) = (entry, inner.uri_for(&edit.path))
                    else {
                        continue;
                    };
                    changes.entry(uri).or_default().push(TextEdit {
                        range: map.range(file_text, edit.span.0 as usize, edit.span.1 as usize),
                        new_text: edit.new_text,
                    });
                }
                Ok::<_, RenameError>(changes)
            })
            .await?;
        match outcome {
            // Nothing there to rename is an empty answer, as it is for `prepareRename`.
            Err(RenameError::Nothing) => Ok(None),
            Ok(changes) if changes.is_empty() => Ok(None),
            Ok(changes) => Ok(Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            })),
            Err(error) => Err(tower_lsp_server::jsonrpc::Error::invalid_params(
                error.to_string(),
            )),
        }
    }
}

/// The file name of a path, which decides the dialect.
fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Backend {
    /// Runs `work` off the async threads, on the stack the parser and checker need. A panic in
    /// it is an error answer for this request and nothing else.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(Arc<Inner>) -> T + Send + 'static,
    ) -> Result<T> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || tsox::with_deep_stack(move || work(inner)))
            .await
            .map_err(|error| {
                tracing::error!(%error, "a request failed");
                tower_lsp_server::jsonrpc::Error::internal_error()
            })
    }

    /// The open document a request names: its path and its text as it is now.
    fn document(&self, uri: &Uri) -> Option<(PathBuf, String)> {
        let path = uri_to_path(uri)?;
        let text = self
            .inner
            .documents()
            .get(&path)
            .map(|open| open.document.text.clone())?;
        Some((path, text))
    }

    /// The open document under a request's position: its path, its text as it is now, and
    /// the byte offset of the position in it.
    fn cursor(
        &self,
        uri: &Uri,
        position: tower_lsp_server::ls_types::Position,
    ) -> Option<(PathBuf, String, usize, i32)> {
        let path = uri_to_path(uri)?;
        let (text, version) = self
            .inner
            .documents()
            .get(&path)
            .map(|open| (open.document.text.clone(), open.document.version))?;
        let offset = LineMap::new(&text).offset(&text, position);
        Some((path, text, offset, version))
    }
}
