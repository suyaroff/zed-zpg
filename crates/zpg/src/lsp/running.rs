//! Cancelling a long statement or a program like `pg_dump`. The server
//! answers one message at a time, so while something runs, a thread of its
//! own reads Zed's messages: it answers code actions with *Cancel*, and
//! cancels on that command or when Zed cancels the progress shown in the
//! status bar. Everything else waits for the run to end.

use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{Notification as _, Progress, WorkDoneProgressCancel};
use lsp_types::request::{CodeActionRequest, ExecuteCommand, Request as _, WorkDoneProgressCreate};
use lsp_types::{
    CodeActionOrCommand, Command, NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress,
    WorkDoneProgressBegin, WorkDoneProgressCancelParams, WorkDoneProgressCreateParams, WorkDoneProgressEnd,
};
use postgres::Client;

use super::{Res, Server, notify};
use crate::db::Canceller;

pub const CMD_CANCEL: &str = "zpg.cancel";

/// How to stop what runs.
#[derive(Clone)]
pub enum Stop {
    /// A statement: Postgres's cancel request.
    Query(Canceller),
    /// A program: the waiting loop sees the flag and kills it.
    Flag(Arc<AtomicBool>),
}

/// The statement or program being run.
pub struct Running {
    pub stop: Stop,
    /// For the action's title: *Cancel <what>*.
    pub what: String,
    /// Of the status-bar progress.
    pub token: String,
}

pub type Shared = Arc<Mutex<Option<Running>>>;

/// Reads `from` on a thread of its own and passes on what it doesn't handle itself.
pub fn forward(from: Receiver<Message>, replies: Sender<Message>, running: Shared) -> Receiver<Message> {
    let (tx, rx) = crossbeam_channel::unbounded();
    thread::spawn(move || {
        for msg in from {
            match intercept(&msg, &running) {
                Some(Some(reply)) => {
                    let _ = replies.send(reply);
                }
                Some(None) => {}
                None => {
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

fn cancel(running: &Shared) {
    match running.lock().ok().and_then(|r| r.as_ref().map(|r| r.stop.clone())) {
        // Fails only when the server can't be reached; the run then ends on its own.
        Some(Stop::Query(c)) => {
            let _ = c.cancel();
        }
        Some(Stop::Flag(f)) => f.store(true, Ordering::Relaxed),
        None => {}
    }
}

/// `Some(reply)` when handled here (`Some(None)`: nothing to reply), `None` to pass on.
fn intercept(msg: &Message, running: &Shared) -> Option<Option<Message>> {
    let conn = running.lock().ok()?.as_ref().map(|r| (r.what.clone(), r.token.clone()))?;
    match msg {
        Message::Request(Request { id, method, .. }) if method == CodeActionRequest::METHOD => {
            let action =
                CodeActionOrCommand::Command(Command::new(format!("zpg: Cancel {}", conn.0), CMD_CANCEL.into(), None));
            Some(Some(Response::new_ok(id.clone(), vec![action]).into()))
        }
        Message::Request(Request { id, method, params })
            if method == ExecuteCommand::METHOD
                && params.get("command").and_then(|c| c.as_str()) == Some(CMD_CANCEL) =>
        {
            cancel(running);
            Some(Some(Response::new_ok(id.clone(), ()).into()))
        }
        Message::Notification(Notification { method, params }) if method == WorkDoneProgressCancel::METHOD => {
            let p: WorkDoneProgressCancelParams = serde_json::from_value(params.clone()).ok()?;
            if p.token == NumberOrString::String(conn.1) {
                cancel(running);
                Some(None)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Postgres's message for a statement cancelled on request.
pub fn cancelled(message: &str) -> bool {
    message.starts_with("canceling statement due to user request")
}

impl Server {
    /// Shows `title` in the status bar, cancellable; returns its token.
    fn progress_begin(&mut self, conn: &Connection, title: &str) -> Res<NumberOrString> {
        let token = NumberOrString::String(format!("zpg-run-{}", self.next_id + 1));
        if self.caps.progress {
            self.send_request::<WorkDoneProgressCreate>(conn, WorkDoneProgressCreateParams { token: token.clone() })?;
            let begin = WorkDoneProgressBegin {
                title: format!("zpg: {title}"),
                cancellable: Some(true),
                message: Some("ctrl-. to cancel".into()),
                percentage: None,
            };
            notify::<Progress>(
                conn,
                ProgressParams {
                    token: token.clone(),
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(begin)),
                },
            )?;
        }
        Ok(token)
    }

    /// Ends the progress and forgets what ran.
    fn progress_end(&mut self, conn: &Connection, token: NumberOrString) -> Res<()> {
        if let Ok(mut r) = self.running.lock() {
            *r = None;
        }
        if self.caps.progress {
            notify::<Progress>(
                conn,
                ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd { message: None })),
                },
            )?;
        }
        Ok(())
    }

    fn set_running(&mut self, stop: Stop, what: String, token: &NumberOrString) {
        if let (Ok(mut r), NumberOrString::String(t)) = (self.running.lock(), token) {
            *r = Some(Running { stop, what, token: t.clone() });
        }
    }

    /// Runs `f` on the client of `conn_name` with `title` in the status bar;
    /// until it returns, ctrl-. offers to cancel it. The inner error is the
    /// connection's.
    pub(super) fn cancellable<T>(
        &mut self,
        conn: &Connection,
        conn_name: &str,
        title: &str,
        f: impl FnOnce(&mut Client) -> T,
    ) -> Res<Result<T, String>> {
        if let Err(e) = self.db.client(&self.settings, conn_name) {
            return Ok(Err(e));
        }
        let token = self.progress_begin(conn, title)?;
        if let Some(canceller) = self.db.canceller(conn_name) {
            self.set_running(Stop::Query(canceller), format!("the statement running on {conn_name}"), &token);
        }
        let out = self.db.client(&self.settings, conn_name).map(f);
        self.progress_end(conn, token)?;
        Ok(out)
    }

    /// Waits for `child` with `title` in the status bar; ctrl-. offers
    /// *Cancel <title>*, which kills it. `None` when cancelled.
    pub(super) fn wait_child(&mut self, conn: &Connection, title: &str, child: &mut Child) -> Res<Option<ExitStatus>> {
        let token = self.progress_begin(conn, title)?;
        let flag = Arc::new(AtomicBool::new(false));
        self.set_running(Stop::Flag(flag.clone()), title.to_string(), &token);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if flag.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            thread::sleep(Duration::from_millis(100));
        };
        self.progress_end(conn, token)?;
        Ok(status)
    }
}
