mod complete;
mod edits;

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as _, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{
    ApplyWorkspaceEdit, CodeActionRequest, Completion, ExecuteCommand, Formatting, GotoDefinition, HoverRequest,
    Request as _, SemanticTokensFullRequest, SemanticTokensRefresh, ShowDocument,
};
use lsp_types::{
    ApplyWorkspaceEditParams, CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionProviderCapability,
    Command, CompletionOptions, CompletionResponse, Diagnostic, DiagnosticSeverity, DocumentChanges,
    ExecuteCommandOptions, GotoDefinitionResponse, Hover, HoverContents, HoverProviderCapability, InitializeParams,
    Location, MarkupContent, MarkupKind, MessageType, OneOf, OptionalVersionedTextDocumentIdentifier, Position,
    PublishDiagnosticsParams, Range, SaveOptions, SemanticToken, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, ShowDocumentParams, ShowMessageParams,
    TextDocumentEdit, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, TextEdit, Uri, WorkspaceEdit,
};
use serde_json::Value;
use zpg_core::catalog::{self, Section};
use zpg_core::cell::{self, Value as CellValue};
use zpg_core::diff::{self, Diff, Source};
use zpg_core::header::display_ident;
use zpg_core::table::{self, Parsed};
use zpg_core::{Apply, ForeignKey, Issue, QualName, Query, Severity, Snapshot, record, sql};

use self::edits::minimal_edits;
use crate::config::Settings;
use crate::db::{self, Db, Defaults};
use crate::state;

const CMD_REFRESH: &str = "zpg.refresh";
const CMD_PAGE: &str = "zpg.page";
const CMD_APPLY: &str = "zpg.apply";
const CMD_PREVIEW: &str = "zpg.previewSql";
const CMD_OPEN: &str = "zpg.open";
const CMD_OPEN_ROW: &str = "zpg.openRow";

type Res<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Semantic token types, styled by `semantic_token_rules.json` of each language.
const TOKEN_MODIFIED: u32 = 0;
const TOKEN_INSERTED: u32 = 1;
const TOKEN_COLUMN: u32 = 2;
const TOKEN_TYPES: [&str; 3] = ["zpgModified", "zpgInserted", "zpgColumn"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Catalog,
    Table,
    Row,
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
            _ => Kind::Other,
        }
    }

    /// True when the buffer has no unsaved edits.
    fn is_clean(&self) -> bool {
        self.path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|t| t == self.text)
    }
}

#[derive(Default)]
struct ClientCaps {
    apply_edit: bool,
    document_changes: bool,
    show_document: bool,
    tokens_refresh: bool,
}

#[derive(Default)]
struct Server {
    docs: HashMap<String, Doc>,
    snaps: HashMap<PathBuf, Snapshot>,
    /// Why the last write of a buffer to the database failed.
    failed: HashMap<String, Issue>,
    settings: Settings,
    db: Db,
    caps: ClientCaps,
    next_id: i32,
    /// Files to write once a connection to the client is at hand.
    pending_writes: Vec<(PathBuf, String)>,
}

pub fn run() -> Res<()> {
    let (conn, io_threads) = Connection::stdio();
    let caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                include_text: Some(false),
            })),
            ..Default::default()
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["\"".into()]),
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
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: [CMD_REFRESH, CMD_PAGE, CMD_APPLY, CMD_PREVIEW, CMD_OPEN, CMD_OPEN_ROW]
                .map(String::from)
                .to_vec(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let init: InitializeParams = serde_json::from_value(conn.initialize(serde_json::to_value(caps)?)?)?;
    let ws = init.capabilities.workspace.unwrap_or_default();

    let mut server = Server {
        caps: ClientCaps {
            apply_edit: ws.apply_edit.unwrap_or(false),
            document_changes: ws.workspace_edit.and_then(|w| w.document_changes).unwrap_or(false),
            tokens_refresh: ws.semantic_tokens.and_then(|s| s.refresh_support).unwrap_or(false),
            show_document: init
                .capabilities
                .window
                .and_then(|w| w.show_document)
                .is_some_and(|s| s.support),
        },
        ..Default::default()
    };
    match Settings::from_value(init.initialization_options) {
        Ok(s) => server.settings = s,
        Err(e) => show(&conn, MessageType::ERROR, format!("zpg: {e}"))?,
    }

    for msg in &conn.receiver {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req)? {
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
            Message::Response(_) => {}
        }
    }
    drop(conn);
    io_threads.join()?;
    Ok(())
}

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str().strip_prefix("file://")?;
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    // file:///C:/dir -> C:/dir
    if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') {
        return Some(PathBuf::from(&path[1..]));
    }
    Some(PathBuf::from(path))
}

fn path_to_uri(path: &Path) -> Res<Uri> {
    let mut s = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        s = format!("/{}", s.replace('\\', "/"));
    }
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    Ok(out.parse()?)
}

/// A file name for arbitrary text.
fn encode_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c.to_string()
            } else {
                c.to_string().bytes().map(|b| format!("%{b:02X}")).collect()
            }
        })
        .collect()
}

/// File name of a row's `.pgr`: its key, shortened with a hash when long.
fn row_file(key: &[Option<String>]) -> String {
    let text = key.iter().map(|v| v.as_deref().unwrap_or("NULL")).collect::<Vec<_>>().join(",");
    let enc = encode_name(&text);
    if enc.chars().count() <= 120 {
        return format!("{enc}.pgr");
    }
    // FNV-1a: stable across builds, unlike std's hasher.
    let hash = text
        .bytes()
        .fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
    format!("{}-{hash:016x}.pgr", enc.chars().take(80).collect::<String>())
}

/// `.zpg/<conn>/schema.table.pgt` -> `.zpg/<conn>/schema.table.rows/`
fn rows_dir(table_path: &Path) -> PathBuf {
    table_path.with_extension("rows")
}

/// `.zpg/<conn>/schema.table.rows/<key>.pgr` -> `.zpg/<conn>/schema.table.pgt`
fn table_of_row(row_path: &Path) -> Option<PathBuf> {
    Some(row_path.parent()?.with_extension("pgt"))
}

fn sql_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
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
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn snapshot<'a>(snaps: &'a mut HashMap<PathBuf, Snapshot>, path: &Path) -> Option<&'a Snapshot> {
    if !snaps.contains_key(path) {
        snaps.insert(path.to_path_buf(), state::load(path)?);
    }
    snaps.get(path)
}

/// Index of the cell under UTF-16 column `ch`.
fn cell_at(line: &str, ch: u32) -> Option<usize> {
    table::split(line).iter().rposition(|c| c.start <= ch + 1)
}

/// The snapshot row a buffer line stands for, matched by its key cells.
fn row_of_line(snap: &Snapshot, line: &str) -> Option<usize> {
    let cells = table::split(line);
    if cells.len() != snap.columns.len() || snap.pk.is_empty() {
        return None;
    }
    snap.rows.iter().position(|r| snap.pk.iter().all(|&k| snap.cell(k, &r.values[k]).0 == cells[k].text))
}

/// Text values of the FK columns, if all are plain values.
fn fk_values(fk: &ForeignKey, value_of: impl Fn(usize) -> Option<CellValue>) -> Option<Vec<String>> {
    fk.columns
        .iter()
        .map(|&c| match value_of(c)? {
            CellValue::Text(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn column_md(snap: &Snapshot, i: usize) -> String {
    let col = &snap.columns[i];
    let mut md = format!("**{}** `{}`", col.name, col.type_name);
    if snap.pk.contains(&i) {
        md += " · primary key";
    }
    if col.not_null {
        md += " · not null";
    }
    if col.generated {
        md += " · generated";
    } else if col.has_default {
        md += " · has default";
    }
    for fk in snap.foreign_keys.iter().filter(|f| f.columns.contains(&i)) {
        md += &format!(" · → {}({})", fk.table, fk.ref_columns.join(", "));
    }
    md
}

impl Server {
    fn send_request<R: lsp_types::request::Request>(&mut self, conn: &Connection, params: R::Params) -> Res<()> {
        self.next_id += 1;
        let req = Request::new(RequestId::from(self.next_id), R::METHOD.into(), params);
        conn.sender.send(req.into())?;
        Ok(())
    }

    fn notification(&mut self, conn: &Connection, n: Notification) -> Res<()> {
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let p: lsp_types::DidOpenTextDocumentParams = serde_json::from_value(n.params)?;
                let key = p.text_document.uri.as_str().to_string();
                let doc = Doc {
                    uri: p.text_document.uri,
                    text: p.text_document.text,
                    version: p.text_document.version,
                };
                let catalog = doc.kind() == Kind::Catalog;
                self.docs.insert(key.clone(), doc);
                if catalog {
                    self.refresh_catalog(conn, &key)?;
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
                self.failed.remove(&key);
                self.publish(conn, &key)?;
            }
            DidSaveTextDocument::METHOD => {
                let p: lsp_types::DidSaveTextDocumentParams = serde_json::from_value(n.params)?;
                let key = p.text_document.uri.as_str();
                match self.docs.get(key).map(Doc::kind) {
                    Some(Kind::Table) => self.save_table(conn, key, false)?,
                    Some(Kind::Row) => self.save_row(conn, key, false)?,
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
            DidChangeConfiguration::METHOD => {
                let p: lsp_types::DidChangeConfigurationParams = serde_json::from_value(n.params)?;
                match Settings::from_value(Some(p.settings)) {
                    Ok(s) => {
                        self.settings = s;
                        self.db.clear();
                        let catalogs: Vec<String> = self
                            .docs
                            .iter()
                            .filter(|(_, d)| d.kind() == Kind::Catalog)
                            .map(|(k, _)| k.clone())
                            .collect();
                        for key in catalogs {
                            self.refresh_catalog(conn, &key)?;
                        }
                    }
                    Err(e) => show(conn, MessageType::ERROR, format!("zpg: {e}"))?,
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn request(&mut self, conn: &Connection, req: Request) -> Res<Value> {
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
                    .filter(|d| d.kind() == Kind::Table)
                    .and_then(|d| Some(minimal_edits(&d.text, &table::realign(&d.text)?)))
                    .unwrap_or_default();
                Ok(serde_json::to_value(edits)?)
            }
            SemanticTokensFullRequest::METHOD => {
                let p: lsp_types::SemanticTokensParams = serde_json::from_value(req.params)?;
                let data = self.semantic_tokens(p.text_document.uri.as_str());
                Ok(serde_json::to_value(SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data,
                }))?)
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
                    CMD_REFRESH => self.refresh(conn, &key, None)?,
                    CMD_PAGE => {
                        if let Some(page) = num(1) {
                            self.refresh(conn, &key, Some(page))?;
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
                    CMD_OPEN | CMD_OPEN_ROW => {
                        if let Some(line) = num(1) {
                            let target = self.definition(&key, Position::new(line, 0));
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
            ShowDocumentParams {
                uri,
                external: None,
                take_focus: Some(true),
                selection: None,
            },
        )
    }

    fn doc_key(&self, path: &Path) -> Option<String> {
        self.docs
            .iter()
            .find(|(_, d)| d.path().as_deref() == Some(path))
            .map(|(k, _)| k.clone())
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
            WorkspaceEdit {
                changes: Some(HashMap::from([(doc.uri.clone(), edits)])),
                ..Default::default()
            }
        };
        self.send_request::<ApplyWorkspaceEdit>(
            conn,
            ApplyWorkspaceEditParams {
                label: Some("zpg: reload".into()),
                edit,
            },
        )
    }

    /// Writes a file that may or may not be open in Zed.
    fn write_file(&mut self, conn: &Connection, path: &Path, text: String) -> Res<()> {
        match self.doc_key(path) {
            Some(key) => self.replace_doc(conn, &key, text),
            None => Ok(state::atomic_write(path, text.as_bytes())?),
        }
    }

    /// True when the file is open with edits that aren't in the database yet.
    fn has_local_edits(&mut self, path: &Path) -> bool {
        let Some(key) = self.doc_key(path) else {
            return false;
        };
        let doc = &self.docs[&key];
        if !doc.is_clean() {
            return true;
        }
        let parsed = table::parse(&doc.text);
        match snapshot(&mut self.snaps, path) {
            Some(snap) => parsed.has_errors() || !diff::diff(&parsed, snap).ops.is_empty(),
            None => false,
        }
    }

    fn flush_writes(&mut self, conn: &Connection) -> Res<()> {
        for (path, text) in std::mem::take(&mut self.pending_writes) {
            self.write_file(conn, &path, text)?;
        }
        Ok(())
    }

    fn refresh_catalog(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let names: Vec<String> = self.settings.connections.keys().cloned().collect();
        let sections: Vec<Section> = names
            .into_iter()
            .map(|name| {
                let relations = self.db.client(&self.settings, &name).and_then(db::catalog);
                Section { conn: name, relations }
            })
            .collect();
        self.replace_doc(conn, key, catalog::render(&sections))
    }

    fn defaults(&self, conn: &str, query: Query, width: u32, apply: Apply) -> Defaults {
        Defaults {
            query,
            width,
            apply,
            read_only_conn: self.settings.connections.get(conn).is_some_and(|c| c.read_only()),
        }
    }

    /// Loads the table's shape and a page into a snapshot stored for `path`.
    fn load_table(&mut self, path: &Path, conn: &str, table: &QualName, d: &Defaults) -> Result<Snapshot, String> {
        let client = self.db.client(&self.settings, conn)?;
        let snap = db::load(client, conn, table, d)?;
        self.store(path, snap.clone())?;
        Ok(snap)
    }

    fn store(&mut self, path: &Path, snap: Snapshot) -> Result<(), String> {
        state::save(path, &snap)?;
        self.snaps.insert(path.to_path_buf(), snap);
        Ok(())
    }

    /// Path of the table's `.pgt` under the `.zpg` dir `root`, loading it unless it's already there.
    fn open_table(&mut self, root: &Path, conn: &str, table: &QualName) -> Result<PathBuf, String> {
        let path = root.join(encode_name(conn)).join(format!("{}.pgt", table.file_stem()));
        if path.exists() && snapshot(&mut self.snaps, &path).is_some() {
            return Ok(path);
        }
        let query = Query {
            page: 1,
            limit: self.settings.page_size,
            filter: String::new(),
            order: String::new(),
        };
        let d = self.defaults(conn, query, self.settings.max_cell_width, self.settings.apply.into());
        let snap = self.load_table(&path, conn, table, &d)?;
        state::atomic_write(&path, snap.render().as_bytes())?;
        Ok(path)
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
        let line = text.split('\n').nth(pos.line as usize).unwrap_or("").to_string();
        match doc.kind() {
            Kind::Catalog => {
                let Some((conn, table)) = catalog::entry_at(&text, pos.line) else {
                    return Ok(None);
                };
                let root = path.parent().ok_or("catalog has no directory")?.to_path_buf();
                self.open_table(&root, &conn, &table).map(Some)
            }
            Kind::Table => {
                let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
                    return Ok(None);
                };
                if pos.line == 0 {
                    return Ok(None);
                }
                let cells = table::split(&line);
                let Some(i) = cell_at(&line, pos.character) else {
                    return Ok(None);
                };
                if let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&i)) {
                    let values = fk_values(fk, |c| cells.get(c).and_then(|c| cell::parse(&c.text).ok()));
                    return match values {
                        Some(v) => self.follow_fk(&path, &snap, fk, &v).map(Some),
                        None => Ok(None),
                    };
                }
                match row_of_line(&snap, &line) {
                    Some(r) => self.open_row(&path, &snap, r).map(Some),
                    None => Ok(None),
                }
            }
            Kind::Row => {
                let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
                    return Ok(None);
                };
                let parsed = record::parse(&text, &snap);
                let Some(field) = parsed.field_at(pos.line) else {
                    return Ok(None);
                };
                let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&field.col)) else {
                    return Ok(None);
                };
                let value_of = |c: usize| parsed.fields.iter().find(|f| f.col == c).and_then(|f| f.value.clone().ok());
                let Some(values) = fk_values(fk, value_of) else {
                    return Ok(None);
                };
                let Some(table_path) = table_of_row(&path) else {
                    return Ok(None);
                };
                self.follow_fk(&table_path, &snap, fk, &values).map(Some)
            }
            Kind::Other => Ok(None),
        }
    }

    /// Opens the referenced table filtered to the referenced row.
    fn follow_fk(&mut self, from: &Path, snap: &Snapshot, fk: &ForeignKey, values: &[String]) -> Result<PathBuf, String> {
        let root = from
            .parent()
            .and_then(Path::parent)
            .ok_or("table file is not inside .zpg/<conn>/")?
            .to_path_buf();
        let path = self.open_table(&root, &snap.conn, &fk.table)?;
        if self.has_local_edits(&path) {
            return Ok(path);
        }
        let Some(target) = snapshot(&mut self.snaps, &path).cloned() else {
            return Ok(path);
        };
        let filter = fk
            .ref_columns
            .iter()
            .zip(values)
            .map(|(c, v)| format!("{} = {}", display_ident(c), sql_literal(v)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let query = Query {
            page: 1,
            filter,
            ..target.query.clone()
        };
        let d = self.defaults(&target.conn, query, target.width, target.apply);
        let loaded = self.load_table(&path, &target.conn, &target.table, &d)?;
        self.pending_writes.push((path.clone(), loaded.render()));
        Ok(path)
    }

    /// Writes the row's `.pgr` (unless it has unsaved edits) and returns its path.
    fn open_row(&mut self, table_path: &Path, snap: &Snapshot, row: usize) -> Result<PathBuf, String> {
        let key = snap.pk.iter().map(|&k| snap.rows[row].values[k].clone()).collect::<Vec<_>>();
        let path = rows_dir(table_path).join(row_file(&key));
        if self.doc_key(&path).is_some_and(|k| !self.docs[&k].is_clean()) {
            return Ok(path);
        }
        let client = self.db.client(&self.settings, &snap.conn)?;
        let fetched = db::fetch_row(client, snap, &key, true)?.ok_or("the row no longer exists; refresh the table")?;
        let row_snap = Snapshot {
            rows: vec![fetched],
            width: 0,
            ..snap.clone()
        };
        let label = snap
            .pk
            .iter()
            .zip(&key)
            .map(|(&k, v)| format!("{}={}", snap.columns[k].name, v.as_deref().unwrap_or("NULL")))
            .collect::<Vec<_>>()
            .join(",");
        self.pending_writes.push((path.clone(), record::render(&row_snap, &label)));
        self.store(&path, row_snap)?;
        Ok(path)
    }

    /// Reloads a catalog, table page (another page when `page` is set) or row.
    fn refresh(&mut self, conn: &Connection, key: &str, page: Option<u32>) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let kind = doc.kind();
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let text = doc.text.clone();
        match kind {
            Kind::Catalog => return self.refresh_catalog(conn, key),
            Kind::Row => return self.refresh_row(conn, key, &path),
            Kind::Table => {}
            Kind::Other => return Ok(()),
        }
        let parsed = table::parse(&text);
        let old = snapshot(&mut self.snaps, &path).cloned();
        if page.is_some()
            && let Some(old) = &old
            && !parsed.has_errors()
            && !diff::diff(&parsed, old).ops.is_empty()
        {
            return show(conn, MessageType::WARNING, "zpg: save or discard changes before switching pages".into());
        }
        // The header wins, so a refresh also works when the saved state is gone.
        let (conn_name, table, mut query, width, apply) = match (&parsed.header, &old) {
            (Some(h), _) => (h.conn.clone(), h.table.clone(), h.query.clone(), h.width, h.apply),
            (None, Some(o)) => (o.conn.clone(), o.table.clone(), o.query.clone(), o.width, o.apply),
            (None, None) => return show(conn, MessageType::ERROR, "zpg: fix the header line first".into()),
        };
        if let Some(p) = page {
            query.page = p.max(1);
        }
        let d = self.defaults(&conn_name, query, width, apply);
        let snap = match self.load_table(&path, &conn_name, &table, &d) {
            Ok(s) => s,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        if page.is_some_and(|p| p > 1) && snap.rows.is_empty() {
            if let Some(o) = old {
                self.store(&path, o)?;
            }
            return show(conn, MessageType::INFO, "zpg: no more rows".into());
        }
        self.failed.remove(key);
        self.replace_doc(conn, key, snap.render())?;
        self.publish(conn, key)
    }

    fn refresh_row(&mut self, conn: &Connection, key: &str, path: &Path) -> Res<()> {
        let Some(snap) = snapshot(&mut self.snaps, path).cloned() else {
            return show(conn, MessageType::ERROR, "zpg: no saved state; open the row from its table".into());
        };
        let Some(row) = snap.rows.first() else {
            return Ok(());
        };
        let key_values: Vec<Option<String>> = snap.pk.iter().map(|&k| row.values[k].clone()).collect();
        let label = record::RowHeader::parse(self.docs[key].text.lines().next().unwrap_or(""))
            .map(|h| h.key)
            .unwrap_or_default();
        let fetched = match self
            .db
            .client(&self.settings, &snap.conn)
            .and_then(|c| db::fetch_row(c, &snap, &key_values, true))
        {
            Ok(Some(r)) => r,
            Ok(None) => return show(conn, MessageType::WARNING, "zpg: the row no longer exists".into()),
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let new = Snapshot {
            rows: vec![fetched],
            ..snap
        };
        let text = record::render(&new, &label);
        self.store(path, new)?;
        self.failed.remove(key);
        self.replace_doc(conn, key, text)?;
        self.publish(conn, key)
    }

    /// Records a failed write and reports it.
    fn fail(&mut self, conn: &Connection, key: &str, line: u32, message: &str) -> Res<()> {
        let r = line_range(&self.docs[key].text, line);
        self.failed.insert(
            key.to_string(),
            Issue::error(line, 0, r.end.character.max(1), format!("not saved: {message}")),
        );
        show(conn, MessageType::ERROR, format!("zpg: nothing written, transaction rolled back: {message}"))?;
        self.publish(conn, key)
    }

    /// Runs `d` against the database; `None` after reporting a failure.
    fn run_diff(&mut self, conn: &Connection, key: &str, snap: &Snapshot, d: &Diff) -> Res<Option<Vec<Option<zpg_core::SnapRow>>>> {
        let client = match self.db.client(&self.settings, &snap.conn) {
            Ok(c) => c,
            Err(e) => {
                show(conn, MessageType::ERROR, format!("zpg: {e}"))?;
                return Ok(None);
            }
        };
        match db::apply(client, snap, d) {
            Ok(rows) => Ok(Some(rows)),
            Err(e) => {
                let line = d.ops[e.op].line().unwrap_or(0);
                self.fail(conn, key, line, &e.message)?;
                Ok(None)
            }
        }
    }

    /// Writes the buffer's changes to the database. In `apply=manual` mode only
    /// when `force` (the Apply command).
    fn save_table(&mut self, conn: &Connection, key: &str, force: bool) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let parsed = table::parse(&doc.text);
        let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
            return self.publish(conn, key);
        };
        let not_saved = |conn: &Connection| {
            show(conn, MessageType::WARNING, "zpg: not written to the database, fix the errors first".into())
        };
        let Some(header) = parsed.header.clone().filter(|_| !parsed.has_errors()) else {
            self.publish(conn, key)?;
            return not_saved(conn);
        };
        if header.conn != snap.conn || header.table != snap.table {
            self.publish(conn, key)?;
            return not_saved(conn);
        }
        let d = diff::diff(&parsed, &snap);
        if d.issues.iter().any(|i| i.severity == Severity::Error) {
            self.publish(conn, key)?;
            return not_saved(conn);
        }

        let mut new = snap.clone();
        new.apply = header.apply;
        let mut summary = None;
        if !d.ops.is_empty() {
            if header.apply == Apply::Manual && !force {
                if new.apply != snap.apply {
                    self.store(&path, new)?;
                }
                return self.publish(conn, key);
            }
            let Some(rows) = self.run_diff(conn, key, &snap, &d)? else {
                return Ok(());
            };
            new.rows = d
                .order
                .iter()
                .filter_map(|s| match *s {
                    Source::Snapshot(i) => Some(snap.rows[i].clone()),
                    Source::Op(k) => rows[k].clone(),
                })
                .collect();
            summary = Some(d.summary());
        }

        if header.query != snap.query || header.width != snap.width {
            new.width = header.width;
            let fetched = self
                .db
                .client(&self.settings, &snap.conn)
                .and_then(|c| db::fetch(c, &new, &header.query));
            match fetched {
                Ok(rows) => {
                    new.query = header.query.clone();
                    new.rows = rows;
                }
                Err(e) => show(conn, MessageType::ERROR, format!("zpg: {e}"))?,
            }
        }

        let text = new.render();
        self.store(&path, new)?;
        self.failed.remove(key);
        self.replace_doc(conn, key, text)?;
        if let Some(s) = summary {
            show(conn, MessageType::INFO, format!("zpg: saved {s}"))?;
        }
        self.refresh_tokens(conn)?;
        self.publish(conn, key)
    }

    /// Like `save_table`; a row view follows the `apply` mode of its table.
    fn save_row(&mut self, conn: &Connection, key: &str, force: bool) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let text = doc.text.clone();
        let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
            return self.publish(conn, key);
        };
        let parsed = record::parse(&text, &snap);
        let d = record::diff(&parsed, &snap);
        if !parsed.issues.is_empty() || d.issues.iter().any(|i| i.severity == Severity::Error) {
            self.publish(conn, key)?;
            return show(conn, MessageType::WARNING, "zpg: not written to the database, fix the errors first".into());
        }
        if d.ops.is_empty() || (snap.apply == Apply::Manual && !force) {
            return self.publish(conn, key);
        }
        if self.run_diff(conn, key, &snap, &d)?.is_none() {
            return Ok(());
        }
        show(conn, MessageType::INFO, "zpg: saved 1 update".into())?;
        self.refresh_row(conn, key, &path)?;
        self.refresh_tokens(conn)?;
        self.sync_table_row(conn, &path, &snap)
    }

    /// After a row view was saved: update that row in its table page, unless
    /// the page has edits of its own.
    fn sync_table_row(&mut self, conn: &Connection, row_path: &Path, row_snap: &Snapshot) -> Res<()> {
        let Some(table_path) = table_of_row(row_path) else {
            return Ok(());
        };
        if self.has_local_edits(&table_path) {
            return Ok(());
        }
        let Some(mut table) = snapshot(&mut self.snaps, &table_path).cloned() else {
            return Ok(());
        };
        let Some(row) = row_snap.rows.first() else {
            return Ok(());
        };
        let key: Vec<Option<String>> = row_snap.pk.iter().map(|&k| row.values[k].clone()).collect();
        let Some(i) = table
            .rows
            .iter()
            .position(|r| table.pk.iter().zip(&key).all(|(&k, v)| &r.values[k] == v))
        else {
            return Ok(());
        };
        let fetched = self
            .db
            .client(&self.settings, &table.conn)
            .and_then(|c| db::fetch_row(c, &table, &key, false));
        if let Ok(Some(r)) = fetched {
            table.rows[i] = r;
            let text = table.render();
            self.store(&table_path, table)?;
            self.write_file(conn, &table_path, text)?;
        }
        Ok(())
    }

    fn preview(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let parsed = table::parse(&doc.text);
        let Some(snap) = snapshot(&mut self.snaps, &path) else {
            return Ok(());
        };
        let d = diff::diff(&parsed, snap);
        let mut out = String::from("-- What saving would run, in one transaction.\nBEGIN;\n");
        for op in &d.ops {
            out += &sql::inline(&sql::for_op(snap, op));
            out += ";\n";
        }
        out += "COMMIT;\n";
        let target = path.with_extension("preview.sql");
        state::atomic_write(&target, out.as_bytes())?;
        self.show_document(conn, &target)
    }

    /// The row `fk` points at, as markdown.
    fn referenced_md(&mut self, snap: &Snapshot, fk: &ForeignKey, values: &[String]) -> String {
        let fetched = self
            .db
            .client(&self.settings, &snap.conn)
            .and_then(|c| db::referenced(c, snap, fk, values));
        match fetched {
            Ok(Some(fields)) => {
                let mut md = format!("\n\n**→ {}** (F12 opens it)\n\n", fk.table);
                for (name, v) in fields.iter().take(20) {
                    let (text, _) = cell::render_cut(&CellValue::from_db(v.as_deref()), 80);
                    md += &format!("- `{name}`: {}\n", text.replace('`', "'"));
                }
                if fields.len() > 20 {
                    md += &format!("- … {} more\n", fields.len() - 20);
                }
                md
            }
            Ok(None) => format!("\n\n**→ {}**: no such row", fk.table),
            Err(e) => format!("\n\n**→ {}**: {e}", fk.table),
        }
    }

    fn hover(&mut self, key: &str, pos: Position) -> Option<Hover> {
        let doc = self.docs.get(key)?;
        let kind = doc.kind();
        let path = doc.path()?;
        let text = doc.text.clone();
        let snap = snapshot(&mut self.snaps, &path)?.clone();
        let line = text.split('\n').nth(pos.line as usize)?;

        let md = match kind {
            Kind::Table if pos.line > 0 => {
                let i = cell_at(line, pos.character)?;
                snap.columns.get(i)?;
                let mut md = column_md(&snap, i);
                let cells = table::split(line);
                if cells.len() == snap.columns.len() {
                    if let Some(r) = row_of_line(&snap, line)
                        && let Some(v) = &snap.rows[r].values[i]
                        && snap.cell(i, &snap.rows[r].values[i]).1
                    {
                        md += &format!("\n\n```\n{v}\n```");
                    }
                    if let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&i))
                        && let Some(values) = fk_values(fk, |c| cell::parse(&cells[c].text).ok())
                    {
                        md += &self.referenced_md(&snap, fk, &values);
                    }
                }
                md
            }
            Kind::Row => {
                let parsed = record::parse(&text, &snap);
                let field = parsed.field_at(pos.line)?;
                let mut md = column_md(&snap, field.col);
                if let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&field.col)) {
                    let value_of = |c: usize| parsed.fields.iter().find(|f| f.col == c).and_then(|f| f.value.clone().ok());
                    if let Some(values) = fk_values(fk, value_of) {
                        md += &self.referenced_md(&snap, fk, &values);
                    }
                }
                md
            }
            _ => return None,
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: None,
        })
    }

    fn completion(&mut self, key: &str, pos: Position) -> Vec<lsp_types::CompletionItem> {
        let Some(doc) = self.docs.get(key) else {
            return vec![];
        };
        let kind = doc.kind();
        let Some(path) = doc.path() else {
            return vec![];
        };
        let text = doc.text.clone();
        let Some(snap) = snapshot(&mut self.snaps, &path) else {
            return vec![];
        };
        let line = text.split('\n').nth(pos.line as usize).unwrap_or("");
        match kind {
            Kind::Table if pos.line == 0 => complete::header(line, pos, snap),
            Kind::Table => {
                let cells = table::split(line);
                let Some(i) = cell_at(line, pos.character) else {
                    return vec![];
                };
                let c = &cells[i];
                let range = if pos.character >= c.start && pos.character <= c.end.max(c.start) {
                    Range::new(Position::new(pos.line, c.start), Position::new(pos.line, c.end.max(pos.character)))
                } else {
                    Range::new(pos, pos)
                };
                complete::values(snap, i, range)
            }
            Kind::Row => {
                let parsed = record::parse(&text, snap);
                let Some(f) = parsed.field_at(pos.line).filter(|f| f.line == pos.line) else {
                    return vec![];
                };
                let range = Range::new(Position::new(pos.line, f.start), Position::new(pos.line, f.end.max(pos.character)));
                complete::values(snap, f.col, range)
            }
            _ => vec![],
        }
    }

    fn code_actions(&mut self, uri: &Uri, range: Range) -> Vec<CodeActionOrCommand> {
        let key = uri.as_str();
        let Some(doc) = self.docs.get(key) else {
            return vec![];
        };
        let arg = || Value::String(key.to_string());
        let mut out = vec![];
        let kind = doc.kind();
        let text = doc.text.clone();
        let Some(path) = doc.path() else {
            return out;
        };

        match kind {
            Kind::Catalog => {
                if self.caps.show_document
                    && let Some((_, table)) = catalog::entry_at(&text, range.start.line)
                {
                    out.push(command(
                        format!("zpg: Open {table}"),
                        CMD_OPEN,
                        vec![arg(), Value::from(range.start.line)],
                    ));
                }
                out.push(command("zpg: Refresh catalog".into(), CMD_REFRESH, vec![arg()]));
                return out;
            }
            Kind::Row => {
                if let Some(snap) = snapshot(&mut self.snaps, &path) {
                    let parsed = record::parse(&text, snap);
                    let d = record::diff(&parsed, snap);
                    if parsed.issues.is_empty() && !d.ops.is_empty() && d.issues.is_empty() {
                        out.push(command("zpg: Apply changes (1 update)".into(), CMD_APPLY, vec![arg()]));
                    }
                }
                out.push(command("zpg: Refresh row (discard changes)".into(), CMD_REFRESH, vec![arg()]));
                return out;
            }
            Kind::Other => return out,
            Kind::Table => {}
        }

        let parsed = table::parse(&text);
        let Some(snap) = snapshot(&mut self.snaps, &path) else {
            return out;
        };
        let d = (!parsed.has_errors()).then(|| diff::diff(&parsed, snap));
        let pending = d.as_ref().is_some_and(|d| !d.ops.is_empty());
        let clean = d.as_ref().is_some_and(|d| !d.issues.iter().any(|i| i.severity == Severity::Error));

        if self.caps.show_document
            && let Some(line) = text.split('\n').nth(range.start.line as usize)
            && range.start.line > 0
            && row_of_line(snap, line).is_some()
        {
            out.push(command("zpg: Open row".into(), CMD_OPEN_ROW, vec![arg(), Value::from(range.start.line)]));
        }
        if snap.read_only.is_none() {
            out.push(insert_row(uri, &parsed, snap, range.start.line));
        }
        if pending && clean {
            let summary = d.as_ref().map(|d| d.summary()).unwrap_or_default();
            out.push(command(format!("zpg: Apply changes ({summary})"), CMD_APPLY, vec![arg()]));
            if self.caps.show_document {
                out.push(command("zpg: Preview SQL".into(), CMD_PREVIEW, vec![arg()]));
            }
        }
        if !pending {
            let page = parsed.header.as_ref().map_or(snap.query.page, |h| h.query.page);
            if snap.rows.len() as u32 >= snap.query.limit {
                out.push(command(
                    format!("zpg: Next page ({})", page + 1),
                    CMD_PAGE,
                    vec![arg(), Value::from(page + 1)],
                ));
            }
            if page > 1 {
                out.push(command(
                    format!("zpg: Previous page ({})", page - 1),
                    CMD_PAGE,
                    vec![arg(), Value::from(page - 1)],
                ));
            }
        }
        out.push(command(
            if pending {
                "zpg: Refresh (discard changes)".into()
            } else {
                "zpg: Refresh".into()
            },
            CMD_REFRESH,
            vec![arg()],
        ));
        if let Some(h) = &parsed.header {
            let mut toggled = h.clone();
            toggled.apply = match h.apply {
                Apply::Save => Apply::Manual,
                Apply::Manual => Apply::Save,
            };
            let title = match toggled.apply {
                Apply::Manual => "zpg: Apply changes manually (apply=manual)",
                Apply::Save => "zpg: Apply changes on save (apply=save)",
            };
            out.push(edit_action(
                title.into(),
                CodeActionKind::REFACTOR,
                uri,
                vec![TextEdit::new(line_range(&text, 0), toggled.render())],
            ));
        }
        out
    }

    /// Column names, and cells that saving would write.
    fn semantic_tokens(&mut self, key: &str) -> Vec<SemanticToken> {
        let Some(doc) = self.docs.get(key) else {
            return vec![];
        };
        let kind = doc.kind();
        let text = doc.text.clone();
        let Some(path) = doc.path() else {
            return vec![];
        };
        let Some(snap) = snapshot(&mut self.snaps, &path) else {
            return vec![];
        };
        // (line, start, end, type)
        let mut spans: Vec<(u32, u32, u32, u32)> = vec![];
        match kind {
            Kind::Table => {
                let parsed = table::parse(&text);
                if let Some(cols) = &parsed.columns {
                    spans.extend(cols.cells.iter().map(|c| (cols.line, c.start, c.end, TOKEN_COLUMN)));
                }
                if !parsed.has_errors() {
                    let cells_of = |line: u32| parsed.rows.iter().find(|r| r.line == line).map(|r| &r.cells);
                    for op in diff::diff(&parsed, snap).ops {
                        match op {
                            diff::Op::Update { line, sets, .. } => {
                                if let Some(cells) = cells_of(line) {
                                    spans.extend(sets.iter().map(|(i, _)| (line, cells[*i].start, cells[*i].end, TOKEN_MODIFIED)));
                                }
                            }
                            diff::Op::Insert { line, .. } => {
                                if let Some(cells) = cells_of(line) {
                                    spans.extend(cells.iter().map(|c| (line, c.start, c.end, TOKEN_INSERTED)));
                                }
                            }
                            diff::Op::Delete { .. } => {}
                        }
                    }
                }
            }
            Kind::Row => {
                let parsed = record::parse(&text, snap);
                for op in record::diff(&parsed, snap).ops {
                    let diff::Op::Update { sets, .. } = op else { continue };
                    for f in parsed.fields.iter().filter(|f| sets.iter().any(|(i, _)| *i == f.col)) {
                        spans.push((f.line, f.start, f.end, TOKEN_MODIFIED));
                        for (k, l) in f.lines.iter().enumerate().skip(1) {
                            spans.push((f.line + k as u32, 0, zpg_core::utf16_len(l), TOKEN_MODIFIED));
                        }
                    }
                }
            }
            _ => {}
        }
        spans.retain(|s| s.2 > s.1);
        spans.sort();
        let mut out = Vec::with_capacity(spans.len());
        let (mut prev_line, mut prev_start) = (0, 0);
        for (line, start, end, typ) in spans {
            let delta_line = line - prev_line;
            out.push(SemanticToken {
                delta_line,
                delta_start: if delta_line == 0 { start - prev_start } else { start },
                length: end - start,
                token_type: typ,
                token_modifiers_bitset: 0,
            });
            (prev_line, prev_start) = (line, start);
        }
        out
    }

    fn publish(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let kind = doc.kind();
        let uri = doc.uri.clone();
        let version = doc.version;
        let text = doc.text.clone();
        let mut issues = vec![];
        let path = doc.path();
        let no_state = |what: &str| Issue::error(0, 0, 8, format!("no saved state for this file; open it from {what}"));
        match (kind, path) {
            (Kind::Table, Some(path)) => {
                let parsed = table::parse(&text);
                issues.extend(parsed.issues.iter().cloned());
                match snapshot(&mut self.snaps, &path) {
                    None => issues.push(no_state("the catalog (or run Refresh)")),
                    Some(snap) if !parsed.has_errors() => {
                        if let Some(h) = &parsed.header
                            && (h.conn != snap.conn || h.table != snap.table)
                        {
                            issues.push(Issue::error(
                                0,
                                0,
                                8,
                                format!("`conn` and `table` must stay {} / {}", snap.conn, snap.table),
                            ));
                        }
                        let d = diff::diff(&parsed, snap);
                        let manual = parsed.header.as_ref().is_some_and(|h| h.apply == Apply::Manual);
                        issues.extend(pending(&d, manual));
                    }
                    Some(_) => {}
                }
            }
            (Kind::Row, Some(path)) => match snapshot(&mut self.snaps, &path) {
                None => issues.push(no_state("its table (F12 on the key)")),
                Some(snap) => {
                    let parsed = record::parse(&text, snap);
                    issues.extend(parsed.issues.iter().cloned());
                    if parsed.issues.is_empty() {
                        issues.extend(pending(&record::diff(&parsed, snap), snap.apply == Apply::Manual));
                    }
                }
            },
            _ => {}
        }
        if let Some(f) = self.failed.get(key) {
            issues.push(f.clone());
        }
        let diags = issues
            .into_iter()
            .map(|i| Diagnostic {
                range: Range::new(Position::new(i.line, i.start), Position::new(i.line, i.end)),
                severity: Some(match i.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                    Severity::Info => DiagnosticSeverity::INFORMATION,
                }),
                source: Some("zpg".into()),
                message: i.message,
                ..Default::default()
            })
            .collect();
        notify::<PublishDiagnostics>(conn, PublishDiagnosticsParams::new(uri, diags, Some(version)))
    }

    fn refresh_tokens(&mut self, conn: &Connection) -> Res<()> {
        if self.caps.tokens_refresh {
            self.send_request::<SemanticTokensRefresh>(conn, ())?;
        }
        Ok(())
    }
}

/// The diff's own issues plus a header note listing what saving would write.
fn pending(d: &Diff, manual: bool) -> Vec<Issue> {
    let mut out = d.issues.clone();
    if !d.ops.is_empty() && !d.issues.iter().any(|i| i.severity == Severity::Error) {
        out.push(Issue {
            line: 0,
            start: 0,
            end: 8,
            severity: Severity::Info,
            message: format!(
                "pending: {}; {}",
                d.summary(),
                if manual { "run the code action “Apply changes”" } else { "save to write them" }
            ),
        });
    }
    out
}

/// Inserts a template row below the cursor's row (or after the last row).
fn insert_row(uri: &Uri, parsed: &Parsed, snap: &Snapshot, cursor_line: u32) -> CodeActionOrCommand {
    let after = parsed
        .rows
        .iter()
        .rev()
        .find(|r| r.line <= cursor_line)
        .or(parsed.rows.last())
        .map(|r| r.line)
        .or(parsed.columns.as_ref().map(|c| c.line + 1))
        .unwrap_or(0);
    let row = format!("| {} |\n", snap.insert_template().join(" | "));
    edit_action(
        "zpg: Insert row".into(),
        CodeActionKind::REFACTOR,
        uri,
        vec![TextEdit::new(
            Range::new(Position::new(after + 1, 0), Position::new(after + 1, 0)),
            row,
        )],
    )
}
