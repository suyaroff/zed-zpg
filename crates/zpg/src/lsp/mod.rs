mod activity;
mod backup;
mod catalog;
mod complete;
mod diagnostics;
mod edits;
mod export;
mod page;
mod query;
mod row;
mod running;
mod sqlinfo;
mod tokens;
mod uri;
mod values;

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Notification as _,
    PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{
    ApplyWorkspaceEdit, CodeActionRequest, CodeLensRequest, Completion, ExecuteCommand, Formatting, GotoDefinition,
    HoverRequest, Request as _, SemanticTokensFullRequest, ShowDocument,
};
use lsp_types::{
    ApplyWorkspaceEditParams, CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionProviderCapability,
    CodeLensOptions, Command, CompletionOptions, CompletionResponse, DocumentChanges, ExecuteCommandOptions,
    GotoDefinitionResponse, Hover, HoverContents, HoverProviderCapability, InitializeParams, Location, MarkupContent,
    MarkupKind, MessageType, OneOf, OptionalVersionedTextDocumentIdentifier, Position, PublishDiagnosticsParams, Range,
    SaveOptions, SemanticTokenType, SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities,
    ShowDocumentParams, ShowMessageParams, TextDocumentEdit, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri, WorkspaceEdit,
};
use serde_json::Value;
use zpg_core::header::encode_name;
use zpg_core::sqlctx::Schema;
use zpg_core::table;
use zpg_core::{Issue, Snapshot};

use self::edits::minimal_edits;
use self::page::Reload;
use self::tokens::TOKEN_TYPES;
use self::uri::{fnv, path_to_uri, uri_to_path};
use crate::config::{self, Settings};
use crate::db::Db;
use crate::state;

const CMD_REFRESH: &str = "zpg.refresh";
const CMD_PAGE: &str = "zpg.page";
const CMD_APPLY: &str = "zpg.apply";
const CMD_PREVIEW: &str = "zpg.previewSql";
const CMD_OPEN: &str = "zpg.open";
const CMD_OPEN_ROW: &str = "zpg.openRow";
const CMD_RUN: &str = "zpg.run";
const CMD_RERUN: &str = "zpg.rerun";
const CMD_EDIT_RESULT: &str = "zpg.editResult";
const CMD_DROP: &str = "zpg.drop";
const CMD_SORT: &str = "zpg.sort";
const CMD_CONFIG: &str = "zpg.openConfig";
const CMD_DDL: &str = "zpg.ddl";
const CMD_BACKREF: &str = "zpg.referencing";
const CMD_EXPLAIN: &str = "zpg.explain";
const CMD_EXPORT: &str = "zpg.export";
const CMD_FILTER: &str = "zpg.filter";
const CMD_ACTIVITY: &str = "zpg.activity";
const CMD_SIGNAL: &str = "zpg.signal";
const CMD_BACKUP: &str = "zpg.backup";

/// Bigger exports and backups are written but not opened in the editor.
const MAX_OPEN: u64 = 20 << 20;

type Res<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Catalog,
    Table,
    Row,
    Sql,
    /// `.pgres`: rows returned by a statement run from a `.sql` file.
    Result,
    Other,
}

struct Doc {
    uri: Uri,
    text: String,
    version: i32,
}

impl Doc {
    fn path(&self) -> Option<PathBuf> {
        uri_to_path(&self.uri)
    }

    fn kind(&self) -> Kind {
        match self.path().as_deref().and_then(Path::extension).and_then(|e| e.to_str()) {
            Some("pgdb") => Kind::Catalog,
            Some("pgt") => Kind::Table,
            Some("pgr") => Kind::Row,
            Some("sql" | "pgsql") => Kind::Sql,
            Some("pgres") => Kind::Result,
            _ => Kind::Other,
        }
    }

    /// True when the buffer has no unsaved edits.
    fn is_clean(&self) -> bool {
        self.path().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|t| t == self.text)
    }
}

#[derive(Default)]
struct ClientCaps {
    apply_edit: bool,
    document_changes: bool,
    show_document: bool,
    tokens_refresh: bool,
    code_lens_refresh: bool,
    /// `window/showMessageRequest` with buttons.
    message_actions: bool,
    /// Status-bar progress (`window/workDoneProgress/create`).
    progress: bool,
}

#[derive(Default)]
struct Server {
    docs: HashMap<String, Doc>,
    snaps: HashMap<PathBuf, Snapshot>,
    results: HashMap<PathBuf, query::ResultState>,
    /// First workspace folder, where `.zpg` goes when nothing closer marks a project.
    root: Option<PathBuf>,
    /// Why the last write of a buffer to the database failed.
    failed: HashMap<String, Issue>,
    settings: Settings,
    /// When the settings file was last read; `None` when it didn't exist.
    settings_mtime: Option<SystemTime>,
    db: Db,
    caps: ClientCaps,
    next_id: i32,
    /// Files to write once a connection to the client is at hand.
    pending_writes: Vec<(PathBuf, String)>,
    /// Tables and columns per connection for `.sql` files, with when they
    /// were read; `None` when reading failed.
    schemas: HashMap<String, (Instant, Option<Schema>)>,
    /// Runs waiting for the user's yes, by request id.
    confirms: HashMap<i32, query::Pending>,
    /// Errors Postgres found in `.sql` buffers on open/save; dropped on edit.
    checked: HashMap<String, Vec<Issue>>,
    /// The statement running now, for the thread that reads messages meanwhile.
    running: running::Shared,
}

pub fn run() -> Res<()> {
    let (conn, io_threads) = Connection::stdio();
    let caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions { include_text: Some(false) })),
            ..Default::default()
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["\"".into(), ".".into()]),
            ..Default::default()
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: TOKEN_TYPES.iter().map(|t| SemanticTokenType::new(t)).collect(),
                    token_modifiers: vec![],
                },
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                ..Default::default()
            },
        )),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        code_lens_provider: Some(CodeLensOptions { resolve_provider: Some(false) }),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: [
                CMD_REFRESH,
                CMD_PAGE,
                CMD_APPLY,
                CMD_PREVIEW,
                CMD_OPEN,
                CMD_OPEN_ROW,
                CMD_RUN,
                CMD_RERUN,
                CMD_EDIT_RESULT,
                CMD_DROP,
                CMD_SORT,
                CMD_CONFIG,
                CMD_DDL,
                CMD_BACKREF,
                CMD_EXPLAIN,
                CMD_EXPORT,
                CMD_FILTER,
                running::CMD_CANCEL,
                CMD_ACTIVITY,
                CMD_SIGNAL,
                CMD_BACKUP,
            ]
            .map(String::from)
            .to_vec(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let init: InitializeParams = serde_json::from_value(conn.initialize(serde_json::to_value(caps)?)?)?;
    let ws = init.capabilities.workspace.unwrap_or_default();
    #[allow(deprecated)]
    let root = init
        .workspace_folders
        .as_ref()
        .and_then(|f| f.first())
        .map(|f| f.uri.clone())
        .or(init.root_uri.clone())
        .and_then(|u| uri_to_path(&u));

    let mut server = Server {
        root,
        caps: ClientCaps {
            apply_edit: ws.apply_edit.unwrap_or(false),
            document_changes: ws.workspace_edit.and_then(|w| w.document_changes).unwrap_or(false),
            tokens_refresh: ws.semantic_tokens.and_then(|s| s.refresh_support).unwrap_or(false),
            code_lens_refresh: ws.code_lens.and_then(|c| c.refresh_support).unwrap_or(false),
            message_actions: init.capabilities.window.as_ref().is_some_and(|w| w.show_message.is_some()),
            progress: init.capabilities.window.as_ref().and_then(|w| w.work_done_progress).unwrap_or(false),
            show_document: init.capabilities.window.and_then(|w| w.show_document).is_some_and(|s| s.support),
        },
        ..Default::default()
    };
    if let Err(e) = config::ensure_config_file() {
        show(&conn, MessageType::WARNING, format!("zpg: {e}"))?;
    }
    server.settings_mtime = settings_mtime();
    match config::load() {
        Ok(s) => server.settings = s,
        Err(e) => show(&conn, MessageType::ERROR, format!("zpg: {e}"))?,
    }

    let messages = running::forward(conn.receiver.clone(), conn.sender.clone(), server.running.clone());
    for msg in &messages {
        match msg {
            Message::Request(req) => {
                // `exit` follows, and the reader thread stops on it.
                if req.method == lsp_types::request::Shutdown::METHOD {
                    conn.sender.send(Response::new_ok(req.id, ()).into())?;
                    break;
                }
                let id = req.id.clone();
                let resp = match server.request(&conn, req) {
                    Ok(v) => Response::new_ok(id, v),
                    Err(e) => Response::new_err(id, lsp_server::ErrorCode::RequestFailed as i32, e.to_string()),
                };
                conn.sender.send(resp.into())?;
            }
            Message::Notification(n) => server.notification(&conn, n)?,
            Message::Response(r) => {
                // Our requests have numeric ids.
                if let Ok(id) = r.id.to_string().parse::<i32>() {
                    server.answer(&conn, id, r.response_result.ok())?;
                }
            }
        }
    }
    drop(conn);
    io_threads.join()?;
    Ok(())
}

fn settings_mtime() -> Option<SystemTime> {
    std::fs::metadata(config::config_path()?).and_then(|m| m.modified()).ok()
}

fn markdown_hover(md: String) -> Hover {
    Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: md }), range: None }
}

fn notify<N: lsp_types::notification::Notification>(conn: &Connection, params: N::Params) -> Res<()> {
    conn.sender.send(Notification::new(N::METHOD.into(), params).into())?;
    Ok(())
}

fn show(conn: &Connection, typ: MessageType, message: String) -> Res<()> {
    notify::<ShowMessage>(conn, ShowMessageParams { typ, message })
}

fn line_range(text: &str, line: u32) -> Range {
    let l = text.split('\n').nth(line as usize).unwrap_or("");
    Range::new(Position::new(line, 0), Position::new(line, zpg_core::utf16_len(l)))
}

fn command(title: String, cmd: &str, args: Vec<Value>) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.clone(),
        kind: Some(CodeActionKind::SOURCE),
        command: Some(Command::new(title, cmd.into(), Some(args))),
        ..Default::default()
    })
}

fn edit_action(title: String, kind: CodeActionKind, uri: &Uri, edits: Vec<TextEdit>) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(kind),
        edit: Some(WorkspaceEdit { changes: Some(HashMap::from([(uri.clone(), edits)])), ..Default::default() }),
        ..Default::default()
    })
}

fn snapshot<'a>(snaps: &'a mut HashMap<PathBuf, Snapshot>, path: &Path) -> Option<&'a Snapshot> {
    if !snaps.contains_key(path) {
        snaps.insert(path.to_path_buf(), state::load(path)?);
    }
    snaps.get(path)
}

impl Server {
    /// Returns the request's id, to match the response.
    fn send_request<R: lsp_types::request::Request>(&mut self, conn: &Connection, params: R::Params) -> Res<i32> {
        self.next_id += 1;
        let req = Request::new(RequestId::from(self.next_id), R::METHOD.into(), params);
        conn.sender.send(req.into())?;
        Ok(self.next_id)
    }

    /// Re-reads the settings file when it changed since it was last read.
    fn reload_settings(&mut self, conn: &Connection) -> Res<()> {
        let mtime = settings_mtime();
        if mtime == self.settings_mtime {
            return Ok(());
        }
        self.settings_mtime = mtime;
        match config::load() {
            Ok(s) => {
                self.settings = s;
                self.db.clear();
                self.schemas.clear();
                self.refresh_catalogs(conn)?;
            }
            Err(e) => show(conn, MessageType::ERROR, format!("zpg: {e}"))?,
        }
        Ok(())
    }

    fn notification(&mut self, conn: &Connection, n: Notification) -> Res<()> {
        self.reload_settings(conn)?;
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let p: lsp_types::DidOpenTextDocumentParams = serde_json::from_value(n.params)?;
                let key = p.text_document.uri.as_str().to_string();
                let doc =
                    Doc { uri: p.text_document.uri, text: p.text_document.text, version: p.text_document.version };
                let kind = doc.kind();
                self.docs.insert(key.clone(), doc);
                match kind {
                    Kind::Catalog => self.refresh_catalog(conn, &key)?,
                    Kind::Sql => self.check_sql(&key),
                    _ => {}
                }
                self.publish(conn, &key)?;
            }
            DidChangeTextDocument::METHOD => {
                let p: lsp_types::DidChangeTextDocumentParams = serde_json::from_value(n.params)?;
                let key = p.text_document.uri.as_str().to_string();
                let Some(doc) = self.docs.get_mut(&key) else {
                    return Ok(());
                };
                let Some(change) = p.content_changes.into_iter().last() else {
                    return Ok(());
                };
                doc.text = change.text;
                doc.version = p.text_document.version;
                let sql = doc.kind() == Kind::Sql;
                self.failed.remove(&key);
                self.checked.remove(&key);
                self.publish(conn, &key)?;
                // ▶ lenses carry statement offsets; redraw them for the new text.
                if sql && self.caps.code_lens_refresh {
                    self.send_request::<lsp_types::request::CodeLensRefresh>(conn, ())?;
                }
            }
            DidSaveTextDocument::METHOD => {
                let p: lsp_types::DidSaveTextDocumentParams = serde_json::from_value(n.params)?;
                let key = p.text_document.uri.as_str();
                match self.docs.get(key).map(Doc::kind) {
                    Some(Kind::Table) => self.save_table(conn, key, false)?,
                    Some(Kind::Row) => self.save_row(conn, key, false)?,
                    Some(Kind::Sql) => {
                        self.check_sql(key);
                        self.publish(conn, key)?;
                    }
                    _ => {}
                }
            }
            DidCloseTextDocument::METHOD => {
                let p: lsp_types::DidCloseTextDocumentParams = serde_json::from_value(n.params)?;
                let key = p.text_document.uri.as_str();
                self.docs.remove(key);
                self.failed.remove(key);
                notify::<PublishDiagnostics>(conn, PublishDiagnosticsParams::new(p.text_document.uri, vec![], None))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn request(&mut self, conn: &Connection, req: Request) -> Res<Value> {
        self.reload_settings(conn)?;
        match req.method.as_str() {
            GotoDefinition::METHOD => {
                let p: lsp_types::GotoDefinitionParams = serde_json::from_value(req.params)?;
                let pos = p.text_document_position_params;
                let target = self.definition(pos.text_document.uri.as_str(), pos.position);
                self.flush_writes(conn)?;
                match target {
                    Ok(Some(path)) => Ok(serde_json::to_value(GotoDefinitionResponse::Scalar(Location {
                        uri: path_to_uri(&path)?,
                        range: Range::default(),
                    }))?),
                    Ok(None) => Ok(Value::Null),
                    Err(e) => {
                        show(conn, MessageType::ERROR, format!("zpg: {e}"))?;
                        Ok(Value::Null)
                    }
                }
            }
            HoverRequest::METHOD => {
                let p: lsp_types::HoverParams = serde_json::from_value(req.params)?;
                let pos = p.text_document_position_params;
                Ok(serde_json::to_value(self.hover(pos.text_document.uri.as_str(), pos.position))?)
            }
            Completion::METHOD => {
                let p: lsp_types::CompletionParams = serde_json::from_value(req.params)?;
                let pos = p.text_document_position;
                let items = self.completion(pos.text_document.uri.as_str(), pos.position);
                Ok(serde_json::to_value(CompletionResponse::Array(items))?)
            }
            Formatting::METHOD => {
                let p: lsp_types::DocumentFormattingParams = serde_json::from_value(req.params)?;
                let edits = self
                    .docs
                    .get(p.text_document.uri.as_str())
                    .filter(|d| matches!(d.kind(), Kind::Table | Kind::Result))
                    .and_then(|d| Some(minimal_edits(&d.text, &table::realign(&d.text)?)))
                    .unwrap_or_default();
                Ok(serde_json::to_value(edits)?)
            }
            SemanticTokensFullRequest::METHOD => {
                let p: lsp_types::SemanticTokensParams = serde_json::from_value(req.params)?;
                let data = self.semantic_tokens(p.text_document.uri.as_str());
                Ok(serde_json::to_value(SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data }))?)
            }
            CodeLensRequest::METHOD => {
                let p: lsp_types::CodeLensParams = serde_json::from_value(req.params)?;
                Ok(serde_json::to_value(self.code_lens(p.text_document.uri.as_str()))?)
            }
            CodeActionRequest::METHOD => {
                let p: lsp_types::CodeActionParams = serde_json::from_value(req.params)?;
                Ok(serde_json::to_value(self.code_actions(&p.text_document.uri, p.range))?)
            }
            ExecuteCommand::METHOD => {
                let p: lsp_types::ExecuteCommandParams = serde_json::from_value(req.params)?;
                let Some(Value::String(key)) = p.arguments.first() else {
                    return Ok(Value::Null);
                };
                let key = key.clone();
                let num = |i: usize| p.arguments.get(i).and_then(Value::as_u64).map(|n| n as u32);
                match p.command.as_str() {
                    CMD_REFRESH => self.refresh(conn, &key, Reload::Same)?,
                    CMD_ACTIVITY => {
                        if let Some(line) = num(1) {
                            self.activity(conn, &key, line)?;
                        }
                    }
                    CMD_SIGNAL => {
                        let pid = p.arguments.get(1).and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok());
                        if let Some(pid) = pid {
                            let terminate = p.arguments.get(2).and_then(Value::as_str) == Some("terminate");
                            self.signal(conn, &key, pid, terminate, false)?;
                        }
                    }
                    CMD_FILTER => {
                        if let Some(filter) = p.arguments.get(1).and_then(Value::as_str) {
                            self.refresh(conn, &key, Reload::Filter(filter.to_string()))?;
                        }
                    }
                    CMD_SORT => {
                        if let Some(order) = p.arguments.get(1).and_then(Value::as_str) {
                            self.refresh(conn, &key, Reload::Order(order.to_string()))?;
                        }
                    }
                    CMD_PAGE => {
                        if let Some(page) = num(1) {
                            self.refresh(conn, &key, Reload::Page(page))?;
                        }
                    }
                    CMD_APPLY => {
                        let doc = self.docs.get(&key);
                        if doc.is_some_and(Doc::is_clean) {
                            match doc.map(Doc::kind) {
                                Some(Kind::Row) => self.save_row(conn, &key, true)?,
                                _ => self.save_table(conn, &key, true)?,
                            }
                        } else {
                            show(conn, MessageType::WARNING, "zpg: save the file first".into())?;
                        }
                    }
                    CMD_PREVIEW => self.preview(conn, &key)?,
                    CMD_DDL => self.show_ddl(conn, &key, num(1))?,
                    CMD_BACKUP => self.backup(conn, &key, num(1))?,
                    CMD_RUN => {
                        if let Some(target) = query::RunTarget::from_args(&p.arguments) {
                            self.run_sql(conn, &key, target)?;
                        }
                    }
                    CMD_EXPLAIN => {
                        if let Some(target) = query::RunTarget::from_args(&p.arguments) {
                            let analyze = p.arguments.get(6).and_then(Value::as_bool).unwrap_or(false);
                            self.explain(conn, &key, target, analyze)?;
                        }
                    }
                    CMD_RERUN => self.rerun(conn, &key)?,
                    CMD_EXPORT => {
                        if let Some(format) = p.arguments.get(1).and_then(Value::as_str) {
                            self.export(conn, &key, format)?;
                        }
                    }
                    CMD_EDIT_RESULT => self.edit_result(conn, &key)?,
                    CMD_DROP => {
                        if let Some(line) = num(1) {
                            self.prepare_drop(conn, &key, line)?;
                        }
                    }
                    CMD_CONFIG => match config::ensure_config_file() {
                        Ok(path) => self.show_document(conn, &path)?,
                        Err(e) => show(conn, MessageType::ERROR, format!("zpg: {e}"))?,
                    },
                    CMD_OPEN | CMD_OPEN_ROW | CMD_BACKREF => {
                        if let Some(line) = num(1) {
                            let target = match (p.command.as_str(), num(2)) {
                                (CMD_BACKREF, Some(k)) => self.referencing(&key, line, k as usize),
                                _ => self.definition(&key, Position::new(line, 0)),
                            };
                            self.flush_writes(conn)?;
                            match target {
                                Ok(Some(path)) => self.show_document(conn, &path)?,
                                Ok(None) => {}
                                Err(e) => show(conn, MessageType::ERROR, format!("zpg: {e}"))?,
                            }
                        }
                    }
                    _ => {}
                }
                Ok(Value::Null)
            }
            _ => Err(format!("unsupported request {}", req.method).into()),
        }
    }

    fn show_document(&mut self, conn: &Connection, path: &Path) -> Res<()> {
        let uri = path_to_uri(path)?;
        self.send_request::<ShowDocument>(
            conn,
            ShowDocumentParams { uri, external: None, take_focus: Some(true), selection: None },
        )?;
        Ok(())
    }

    fn doc_key(&self, path: &Path) -> Option<String> {
        self.docs.iter().find(|(_, d)| d.path().as_deref() == Some(path)).map(|(k, _)| k.clone())
    }

    /// Puts `text` into the buffer: on disk when it has no unsaved edits (Zed
    /// reloads it), as a workspace edit otherwise.
    fn replace_doc(&mut self, conn: &Connection, key: &str, text: String) -> Res<()> {
        let Some(doc) = self.docs.get_mut(key) else {
            return Ok(());
        };
        if doc.text == text {
            return Ok(());
        }
        if doc.is_clean()
            && let Some(path) = doc.path()
        {
            state::atomic_write(&path, text.as_bytes())?;
            doc.text = text;
            return Ok(());
        }
        if !self.caps.apply_edit {
            return show(conn, MessageType::WARNING, "zpg: save or revert the file, then retry".into());
        }
        let edits = minimal_edits(&doc.text, &text);
        let edit = if self.caps.document_changes {
            WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: doc.uri.clone(),
                        version: Some(doc.version),
                    },
                    edits: edits.into_iter().map(OneOf::Left).collect(),
                }])),
                ..Default::default()
            }
        } else {
            WorkspaceEdit { changes: Some(HashMap::from([(doc.uri.clone(), edits)])), ..Default::default() }
        };
        self.send_request::<ApplyWorkspaceEdit>(
            conn,
            ApplyWorkspaceEditParams { label: Some("zpg: reload".into()), edit },
        )?;
        Ok(())
    }

    /// Writes a file that may or may not be open in Zed.
    fn write_file(&mut self, conn: &Connection, path: &Path, text: String) -> Res<()> {
        match self.doc_key(path) {
            Some(key) => self.replace_doc(conn, &key, text),
            None => Ok(state::atomic_write(path, text.as_bytes())?),
        }
    }

    fn flush_writes(&mut self, conn: &Connection) -> Res<()> {
        for (path, text) in std::mem::take(&mut self.pending_writes) {
            self.write_file(conn, &path, text)?;
        }
        Ok(())
    }

    fn store(&mut self, path: &Path, snap: Snapshot) -> Result<(), String> {
        state::save(path, &snap)?;
        self.snaps.insert(path.to_path_buf(), snap);
        Ok(())
    }

    /// What F12 leads to: a table from the catalog, the row of a key cell, or
    /// the row a foreign key points at.
    fn definition(&mut self, key: &str, pos: Position) -> Result<Option<PathBuf>, String> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(None);
        };
        let Some(path) = doc.path() else {
            return Ok(None);
        };
        let text = doc.text.clone();
        match doc.kind() {
            Kind::Catalog => self.catalog_definition(&path, &text, pos.line),
            Kind::Table => self.table_definition(&path, &text, pos),
            Kind::Row => self.row_definition(&path, &text, pos.line),
            Kind::Sql => self.sql_definition(key, pos),
            Kind::Result | Kind::Other => Ok(None),
        }
    }

    fn hover(&mut self, key: &str, pos: Position) -> Option<Hover> {
        let doc = self.docs.get(key)?;
        let kind = doc.kind();
        let path = doc.path()?;
        let text = doc.text.clone();
        match kind {
            Kind::Result => self.result_hover(&text, &path, pos),
            Kind::Sql => self.sql_hover(key, pos),
            Kind::Catalog => self.catalog_hover(&text, pos),
            Kind::Table => self.table_hover(&path, &text, pos).map(markdown_hover),
            Kind::Row => self.row_hover(&path, &text, pos).map(markdown_hover),
            Kind::Other => None,
        }
    }

    fn completion(&mut self, key: &str, pos: Position) -> Vec<lsp_types::CompletionItem> {
        let Some(doc) = self.docs.get(key) else {
            return vec![];
        };
        let kind = doc.kind();
        if kind == Kind::Sql {
            return self.sql_completion(key, pos);
        }
        let Some(path) = doc.path() else {
            return vec![];
        };
        let text = doc.text.clone();
        match kind {
            Kind::Table => self.table_completion(&path, &text, pos),
            Kind::Row => self.row_completion(&path, &text, pos),
            _ => vec![],
        }
    }

    fn code_actions(&mut self, uri: &Uri, range: Range) -> Vec<CodeActionOrCommand> {
        let key = uri.as_str();
        let Some(doc) = self.docs.get(key) else {
            return vec![];
        };
        let kind = doc.kind();
        let text = doc.text.clone();
        let Some(path) = doc.path() else {
            return vec![];
        };
        match kind {
            Kind::Catalog => self.catalog_actions(key, &text, range.start.line),
            Kind::Table => self.table_actions(uri, &path, &text, range),
            Kind::Row => self.row_actions(key, &path, &text),
            Kind::Sql => {
                let doc = &self.docs[key];
                self.sql_actions(doc, key, range)
            }
            Kind::Result => {
                let mut out = self.pid_actions(key, &path, &text, range.start.line);
                out.extend(self.result_actions(key, &path));
                if self.caps.show_document {
                    out.extend(export::actions(key, Kind::Result));
                }
                out
            }
            Kind::Other => vec![],
        }
    }
}
