mod utils;

use lsp_server::{Connection, Message, RequestId, Response};
use lsp_types::{
    DocumentSymbol, GotoDefinitionResponse, Hover, HoverContents,
    InitializeParams, Location, MarkupContent, MarkupKind, Position, Range,
    ServerCapabilities, SymbolInformation, TextDocumentSyncKind, TextDocumentSyncOptions, Url,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::Sender;
use ctxtag_core::DbStore;

/// Single shared channel to the LSP IO thread. Set once in `main`; used by
/// `send_response` so every handler writes over the same connection instead of
/// spawning a fresh `Connection::stdio()` (which would create competing IO
/// threads on stdin/stdout).
static SENDER: OnceLock<Sender<Message>> = OnceLock::new();

struct BackendState {
    store: Option<Arc<DbStore>>,
    open_documents: HashMap<String, String>,
}

impl BackendState {
    fn new() -> Self {
        Self {
            store: None,
            open_documents: HashMap::new(),
        }
    }
}

/// Ask the kernel to SIGTERM this process when its parent (the editor's LSP
/// client) exits. Without it, a window close/reload that abandons the server
/// while leaving its stdio socket open leaks the process forever — the reader
/// never sees EOF and no `shutdown` arrives. Linux-only; a no-op elsewhere.
#[cfg(target_os = "linux")]
fn install_parent_death_signal() {
    unsafe {
        let parent = libc::getppid();
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0);
        // Race guard: if the parent died between exec and the prctl above, we've
        // already been reparented, so PDEATHSIG will never fire — exit now
        // instead of running orphaned.
        if libc::getppid() != parent {
            std::process::exit(0);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn install_parent_death_signal() {}

fn main() -> anyhow::Result<()> {
    // Arm the parent-death guard first, before anything else can delay it.
    install_parent_death_signal();

    // Logs MUST go to stderr: stdout is the LSP JSON-RPC channel and any stray
    // bytes there corrupt the Content-Length framing.
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let (connection, io_threads) = Connection::stdio();

    // Share the single connection's sender with all handlers.
    SENDER.set(connection.sender.clone()).ok();

    let server_capabilities = ServerCapabilities {
        definition_provider: Some(lsp_types::OneOf::Left(true)),
        references_provider: Some(lsp_types::OneOf::Left(true)),
        document_highlight_provider: Some(lsp_types::OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(lsp_types::OneOf::Left(true)),
        workspace_symbol_provider: Some(lsp_types::OneOf::Left(true)),
        text_document_sync: Some(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL.into()),
                will_save: None,
                will_save_wait_until: None,
                save: Some(lsp_types::TextDocumentSyncSaveOptions::Supported(true)),
            }
            .into(),
        ),
        ..Default::default()
    };

    let init_result = serde_json::to_value(server_capabilities)?;

    // Standard LSP handshake: reads the client's `initialize` request, replies
    // with the matching id, waits for `initialized`, and returns the client's
    // initialize params. `initialize` wraps the value as `{ "capabilities": .. }`.
    let init_params = connection.initialize(init_result)?;

    let state = Arc::new(Mutex::new(BackendState::new()));

    // Load the database from the workspace root: {root}/.ctxtag.db
    if let Ok(params) = serde_json::from_value::<InitializeParams>(init_params) {
        if let Some(root_uri) = params.root_uri {
            if let Ok(root_path) = root_uri.to_file_path() {
                let root_path_str = root_path.to_string_lossy().to_string();
                let db_path = format!("{}/.ctxtag.db", root_path_str);
                match DbStore::new(&db_path) {
                    Ok(store) => {
                        tracing::info!("Loaded database: {}", db_path);
                        state.lock().unwrap().store = Some(Arc::new(store));
                    }
                    Err(e) => tracing::error!("Failed to open database {}: {}", db_path, e),
                }
            }
        }
    }

    loop {
        let msg = match connection.receiver.recv() {
            Ok(msg) => msg,
            Err(_) => break,
        };
        match msg {
            Message::Request(req) => {
                // handle_shutdown returns Ok(true) only for a real `shutdown`
                // request; Ok(false) for everything else (keep looping).
                if connection.handle_shutdown(&req)? {
                    break;
                }
                let method = req.method.clone();
                let id = req.id.clone();

                match method.as_str() {
                    "textDocument/definition" => handle_definition(&state, id, Some(req.params.clone()))?,
                    "textDocument/references" => handle_references(&state, id, Some(req.params.clone()))?,
                    "textDocument/documentSymbol" => {
                        handle_document_symbol(&state, id, Some(req.params.clone()))?
                    }
                    "workspace/symbol" => handle_workspace_symbol(&state, id, Some(req.params.clone()))?,
                    "textDocument/hover" => handle_hover(&state, id, Some(req.params.clone()))?,
                    "textDocument/documentHighlight" => {
                        handle_highlight(&state, id, Some(req.params.clone()))?
                    }
                    _ => tracing::debug!("Unhandled: {}", method),
                }
            }
            Message::Notification(not) => match not.method.as_str() {
                "textDocument/didOpen" => {
                    if let Ok(params) =
                        serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(not.params)
                    {
                        let mut state = state.lock().unwrap();
                        state
                            .open_documents
                            .insert(params.text_document.uri.to_string(), params.text_document.text);
                    }
                }
                "textDocument/didSave" => {
                    // Could trigger re-index here
                }
                _ => {}
            },
            Message::Response(_) => {}
        }
    }

    io_threads.join()?;
    Ok(())
}

fn send_response(
    id: RequestId,
    result: Result<serde_json::Value, String>,
) -> anyhow::Result<()> {
    let sender = match SENDER.get() {
        Some(s) => s,
        None => return Ok(()),
    };
    match result {
        Ok(val) => {
            let _ = sender.send(Message::Response(Response {
                id,
                error: None,
                result: Some(val),
            }));
        }
        Err(msg) => {
            let _ = sender.send(Message::Response(Response {
                id,
                error: Some(lsp_server::ResponseError {
                    code: -32603,
                    message: msg,
                    data: None,
                }),
                result: None,
            }));
        }
    }
    Ok(())
}

fn get_store(state: &Mutex<BackendState>) -> Option<Arc<DbStore>> {
    let s = state.lock().unwrap();
    s.store.clone()
}

/// Convert a u32 LSP symbol kind value to lsp_types::SymbolKind.
fn to_symbol_kind(kind: u32) -> lsp_types::SymbolKind {
    match kind {
        1  => lsp_types::SymbolKind::FILE,
        2  => lsp_types::SymbolKind::MODULE,
        3  => lsp_types::SymbolKind::NAMESPACE,
        4  => lsp_types::SymbolKind::PACKAGE,
        5  => lsp_types::SymbolKind::CLASS,
        6  => lsp_types::SymbolKind::METHOD,
        7  => lsp_types::SymbolKind::PROPERTY,
        8  => lsp_types::SymbolKind::FIELD,
        9  => lsp_types::SymbolKind::CONSTRUCTOR,
        10 => lsp_types::SymbolKind::ENUM,
        11 => lsp_types::SymbolKind::INTERFACE,
        12 => lsp_types::SymbolKind::FUNCTION,
        13 => lsp_types::SymbolKind::VARIABLE,
        14 => lsp_types::SymbolKind::CONSTANT,
        15 => lsp_types::SymbolKind::STRING,
        16 => lsp_types::SymbolKind::NUMBER,
        17 => lsp_types::SymbolKind::BOOLEAN,
        18 => lsp_types::SymbolKind::ARRAY,
        19 => lsp_types::SymbolKind::OBJECT,
        20 => lsp_types::SymbolKind::KEY,
        21 => lsp_types::SymbolKind::NULL,
        22 => lsp_types::SymbolKind::ENUM_MEMBER,
        23 => lsp_types::SymbolKind::STRUCT,
        24 => lsp_types::SymbolKind::EVENT,
        25 => lsp_types::SymbolKind::OPERATOR,
        26 => lsp_types::SymbolKind::TYPE_PARAMETER,
        _  => lsp_types::SymbolKind::VARIABLE,
    }
}

fn handle_definition(
    state: &Arc<Mutex<BackendState>>,
    id: RequestId,
    params: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let store = match get_store(state) {
        Some(s) => s,
        None => return send_response(id, Err("No database".into())),
    };

    let params: lsp_types::GotoDefinitionParams =
        serde_json::from_value(params.unwrap_or_default())?;
    let uri = params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let file_path = uri.to_file_path().unwrap_or_default().to_string_lossy().to_string();
    let word = utils::word_at_position(&uri, pos);

    let word = match word {
        Some(w) => w,
        None => return send_response(id, Err("No word at position".into())),
    };

    // Member access (`p->field` / `x.field`): when the indexer resolved the
    // receiver's struct, scope the jump to that struct's member instead of
    // matching every same-named symbol.
    if let Ok(Some((tag, member))) = store.member_ref_at(&file_path, pos.line, pos.character) {
        let mut results = store.find_member_definition(&tag, &member).unwrap_or_default();
        if results.is_empty() {
            results = store.find_definitions(&member).unwrap_or_default();
        }
        let locations = symbol_locations(&results);
        return send_response(
            id,
            Ok(serde_json::to_value(GotoDefinitionResponse::Array(locations))?),
        );
    }

    // A function-local variable or parameter resolves to its own declaration,
    // not to a same-named global symbol.
    if let Some(loc) = local_definition_location(&file_path, pos.line, pos.character, &word) {
        return send_response(
            id,
            Ok(serde_json::to_value(GotoDefinitionResponse::Array(vec![loc]))?),
        );
    }

    let results = store.find_definitions(&word).ok();
    if let Some(results) = results {
        let locations = symbol_locations(&results);
        return send_response(
            id,
            Ok(serde_json::to_value(GotoDefinitionResponse::Array(locations))?),
        );
    }

    send_response(id, Ok(serde_json::json!(null)))
}

/// Resolve a function-local variable or parameter to its declaration within the
/// enclosing function. Locals aren't stored as symbols, so a plain name lookup
/// would wrongly jump to an unrelated global that shares the name (e.g. a local
/// `attribute` matching `struct attribute`). Returns `None` when the identifier
/// isn't a function-local, letting the caller fall back to the symbol table.
fn local_definition_location(
    file_path: &str,
    line: u32,
    col: u32,
    word: &str,
) -> Option<Location> {
    use ctxtag_core::parser::{find_local_declaration, LanguageKind};
    let path = std::path::Path::new(file_path);
    let language = LanguageKind::from_path(path)?;
    let source = std::fs::read(path).ok()?;
    let decl = find_local_declaration(&source, language, line, col, word)?;
    Some(Location {
        uri: url_from_path(file_path),
        range: Range {
            start: Position {
                line: decl.line_start,
                character: decl.col_start,
            },
            end: Position {
                line: decl.line_end,
                character: decl.col_end,
            },
        },
    })
}

/// Map query results to LSP locations.
fn symbol_locations(results: &[ctxtag_core::QueryResult]) -> Vec<Location> {
    results
        .iter()
        .map(|r| Location {
            uri: url_from_path(&r.file_path),
            range: Range {
                start: Position {
                    line: r.symbol.line_start,
                    character: r.symbol.column_start,
                },
                end: Position {
                    line: r.symbol.line_end,
                    character: r.symbol.column_end,
                },
            },
        })
        .collect()
}

fn handle_references(
    state: &Arc<Mutex<BackendState>>,
    id: RequestId,
    params: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let store = match get_store(state) {
        Some(s) => s,
        None => return send_response(id, Err("No database".into())),
    };

    let params: lsp_types::ReferenceParams =
        serde_json::from_value(params.unwrap_or_default())?;
    let uri = params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let file_path = uri.to_file_path().unwrap_or_default().to_string_lossy().to_string();
    let word = utils::word_at_position(&uri, pos);
    let word = match word {
        Some(w) => w,
        None => return send_response(id, Err("No word".into())),
    };

    // Member access: scope references to the receiver's struct when resolved,
    // otherwise every same-named member across all structs would match.
    if let Ok(Some((tag, member))) = store.member_ref_at(&file_path, pos.line, pos.character) {
        if let Ok(mrefs) = store.find_member_references(&tag, &member) {
            let locations: Vec<Location> = mrefs
                .iter()
                .map(|(file, line, col, _)| ref_location(file, *line, *col))
                .collect();
            return send_response(id, Ok(serde_json::to_value(locations)?));
        }
    }

    // Cursor on a member *definition* (e.g. `int (*set)(...)` inside a struct):
    // scope references to that struct's member instead of every identifier that
    // happens to share the name (locals, params, other structs' members).
    if let Ok(Some(qr)) = store.find_definition_at(&file_path, pos.line, pos.character) {
        if qr.symbol.kind == ctxtag_core::SymbolKind::Member {
            if let Some(container) = qr.symbol.container_name.as_deref() {
                if let Ok(mrefs) = store.find_member_references(container, &qr.symbol.name) {
                    if !mrefs.is_empty() {
                        let locations: Vec<Location> = mrefs
                            .iter()
                            .map(|(file, line, col, _)| ref_location(file, *line, *col))
                            .collect();
                        return send_response(id, Ok(serde_json::to_value(locations)?));
                    }
                }
            }
        }
    }

    let refs = store.find_references(&word).ok();
    if let Some(refs) = refs {
        let locations: Vec<Location> = refs
            .iter()
            .map(|(_, file, line, col, _, _)| ref_location(file, *line, *col))
            .collect();
        return send_response(id, Ok(serde_json::to_value(locations)?));
    }

    send_response(id, Ok(serde_json::json!([])))
}

fn ref_location(file: &str, line: u32, col: u32) -> Location {
    Location {
        uri: url_from_path(file),
        range: Range {
            start: Position { line, character: col },
            end: Position { line, character: col + 1 },
        },
    }
}

fn handle_document_symbol(
    state: &Arc<Mutex<BackendState>>,
    id: RequestId,
    params: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let store = match get_store(state) {
        Some(s) => s,
        None => return send_response(id, Err("No database".into())),
    };

    let params: lsp_types::DocumentSymbolParams =
        serde_json::from_value(params.unwrap_or_default())?;
    let file_path = params
        .text_document
        .uri
        .to_file_path()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let symbols = store.document_symbols(&file_path).ok();
    if let Some(symbols) = symbols {
        let doc_symbols: Vec<DocumentSymbol> = symbols
            .iter()
            .map(|r| DocumentSymbol {
                name: r.symbol.name.clone(),
                detail: r.symbol.signature.clone(),
                kind: to_symbol_kind(r.symbol.kind.lsp_kind()),
                range: Range {
                    start: Position {
                        line: r.symbol.line_start,
                        character: r.symbol.column_start,
                    },
                    end: Position {
                        line: r.symbol.line_end,
                        character: r.symbol.column_end,
                    },
                },
                selection_range: Range {
                    start: Position {
                        line: r.symbol.selection_line.unwrap_or(r.symbol.line_start),
                        character: r.symbol.selection_column.unwrap_or(r.symbol.column_start),
                    },
                    end: Position {
                        line: r.symbol.line_end,
                        character: r.symbol.column_end,
                    },
                },
                tags: None,
                deprecated: None,
                children: None,
            })
            .collect();
        return send_response(id, Ok(serde_json::to_value(doc_symbols)?));
    }

    send_response(id, Ok(serde_json::json!([])))
}

fn handle_workspace_symbol(
    state: &Arc<Mutex<BackendState>>,
    id: RequestId,
    params: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let store = match get_store(state) {
        Some(s) => s,
        None => return send_response(id, Err("No database".into())),
    };

    let params: lsp_types::WorkspaceSymbolParams =
        serde_json::from_value(params.unwrap_or_default())?;
    let query = params.query;
    if query.is_empty() {
        return send_response(id, Ok(serde_json::json!([])));
    }

    let results = store.workspace_symbols(&query, 100).ok();
    if let Some(results) = results {
        let symbols: Vec<SymbolInformation> = results
            .iter()
            .map(|r| SymbolInformation {
                name: r.symbol.name.clone(),
                kind: to_symbol_kind(r.symbol.kind.lsp_kind()),
                tags: None,
                deprecated: None,
                location: Location {
                    uri: url_from_path(&r.file_path),
                    range: Range {
                        start: Position {
                            line: r.symbol.line_start,
                            character: r.symbol.column_start,
                        },
                        end: Position {
                            line: r.symbol.line_end,
                            character: r.symbol.column_end,
                        },
                    },
                },
                container_name: r.symbol.container_name.clone(),
            })
            .collect();
        return send_response(id, Ok(serde_json::to_value(symbols)?));
    }

    send_response(id, Ok(serde_json::json!([])))
}

fn handle_hover(
    state: &Arc<Mutex<BackendState>>,
    id: RequestId,
    params: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let store = match get_store(state) {
        Some(s) => s,
        None => return send_response(id, Err("No database".into())),
    };

    let params: lsp_types::HoverParams =
        serde_json::from_value(params.unwrap_or_default())?;
    let uri = params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let file_path = uri.to_file_path().unwrap_or_default().to_string_lossy().to_string();
    let word = utils::word_at_position(&uri, pos);
    let word = match word {
        Some(w) => w,
        None => return send_response(id, Ok(serde_json::json!(null))),
    };

    let results = if let Ok(Some((tag, member))) =
        store.member_ref_at(&file_path, pos.line, pos.character)
    {
        let scoped = store.find_member_definition(&tag, &member).unwrap_or_default();
        if scoped.is_empty() {
            store.find_definitions(&word).ok()
        } else {
            Some(scoped)
        }
    } else {
        store.find_definitions(&word).ok()
    };
    if let Some(results) = results {
        if let Some(r) = results.first() {
            let mut md = format!(
                "#### {}\n\n**Kind**: {}\n\n",
                r.symbol.name, r.symbol.kind.as_str()
            );
            if let Some(sig) = &r.symbol.signature {
                md.push_str(&format!("```c\n{}\n```\n\n", sig));
            }
            if let Some(doc) = &r.symbol.documentation {
                md.push_str(&format!("{}\n\n", doc));
            }
            md.push_str(&format!("`{}:{}`", r.file_path, r.symbol.line_start + 1));

            let hover = Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: md,
                }),
                range: None,
            };
            return send_response(id, Ok(serde_json::to_value(hover)?));
        }
    }

    send_response(id, Ok(serde_json::json!(null)))
}

fn handle_highlight(
    _state: &Arc<Mutex<BackendState>>,
    id: RequestId,
    _params: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    send_response(id, Ok(serde_json::json!([])))
}

/// Helper to create a file:// URL from a path, falling back to a dummy URL
fn url_from_path(path: &str) -> Url {
    Url::from_file_path(path).unwrap_or_else(|_| {
        // Fallback: construct a minimal valid file URL
        Url::parse(&format!("file://{}", path)).unwrap_or_else(|_| Url::parse("file:///").unwrap())
    })
}
