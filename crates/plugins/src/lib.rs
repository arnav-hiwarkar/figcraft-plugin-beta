//! Shared Craft session host bridge. The document receiver must be serviced on the document thread.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use craft_ext_host::api::{AppApi, AppContract, Code, Invocation, OpError};
use serde_json::Value;
use std::sync::{Arc, mpsc};
use std::time::Duration;

pub mod contract;
pub mod grants;
pub use contract::contract;

pub fn safe_mode() -> bool {
    std::env::var_os("FIGCRAFT_SAFE_MODE").is_some_and(|v| v == "1")
}

/// A request is owned by the document thread. It must discard cancelled requests, and
/// retain transaction snapshots until Finish so a timeout can never leave a partial edit.
pub enum Request {
    Begin(Invocation),
    Call(Invocation, String, Value),
    Finish(Invocation, bool),
}
pub struct Pending {
    pub request: Request,
    pub reply: mpsc::SyncSender<Result<Value, OpError>>,
}

pub struct Bridge {
    contract: AppContract,
    tx: mpsc::SyncSender<Pending>,
}
impl Bridge {
    pub fn channel() -> Result<(Arc<Self>, mpsc::Receiver<Pending>), OpError> {
        let (tx, rx) = mpsc::sync_channel(64);
        Ok((Arc::new(Self { contract: contract()?, tx }), rx))
    }
    fn request(&self, inv: &Invocation, request: Request) -> Result<Value, OpError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.try_send(Pending { request, reply: tx }).map_err(|_| OpError::new(Code::Stale, "document thread busy or closed"))?;
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => {
                inv.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                Err(OpError::new(Code::Cancelled, "document request timed out"))
            }
        }
    }
}
impl AppApi for Bridge {
    fn contract(&self) -> &AppContract {
        &self.contract
    }
    fn begin(&self, inv: &Invocation) -> Result<(), OpError> {
        if safe_mode() {
            return Err(OpError::denied("FigCraft safe mode"));
        }
        self.request(inv, Request::Begin(inv.clone())).map(|_| ())
    }
    fn call(&self, inv: &Invocation, op: &str, args: Value) -> Result<Value, OpError> {
        self.request(inv, Request::Call(inv.clone(), op.into(), args))
    }
    fn finish(&self, inv: &Invocation, commit: bool) {
        if self.request(inv, Request::Finish(inv.clone(), commit && !inv.cancelled())).is_err() {
            inv.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// One worker owns one module instance. Callers must keep one handle per (plugin, module).
pub struct Worker {
    tx: mpsc::SyncSender<(Invocation, Value, mpsc::SyncSender<Result<(), String>>)>,
}
impl Worker {
    pub fn start(
        plugin: craft_ext_host::wasm::Plugin,
        api: Arc<dyn AppApi>,
        grants: Arc<dyn craft_ext_host::api::Grants>,
        log: craft_ext_host::wasm::LogFn,
    ) -> Result<Self, OpError> {
        if safe_mode() {
            return Err(OpError::denied("FigCraft safe mode"));
        }
        let (tx, rx) = mpsc::sync_channel::<(Invocation, Value, mpsc::SyncSender<Result<(), String>>)>(16);
        std::thread::Builder::new()
            .name("figcraft-plugin".into())
            .spawn(move || {
                let mut session = craft_ext_host::wasm::Session::new(&plugin, api, grants, log);
                while let Ok((inv, event, reply)) = rx.recv() {
                    let result = match &mut session {
                        Ok(session) => session.dispatch(inv, &event).map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = reply.send(result);
                }
            })
            .map_err(|e| OpError::new(Code::Internal, e.to_string()))?;
        Ok(Self { tx })
    }
    pub fn dispatch(&self, inv: Invocation, event: Value) -> Result<mpsc::Receiver<Result<(), String>>, OpError> {
        if serde_json::to_vec(&event).map_err(|e| OpError::invalid(e.to_string()))?.len() > craft_ext_host::wasm::MAX_CALL_BYTES {
            return Err(OpError::invalid("event too large"));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.try_send((inv, event, tx)).map_err(|_| OpError::new(Code::Stale, "plugin worker busy or closed"))?;
        Ok(rx)
    }
}

pub mod runtime;
pub use runtime::{Runtime, activate, manager, runtime};

/// Refreshes the catalogue and kill list in the background when the last refresh is older than six
/// hours (B8). Call at startup and periodically; the store change reaches GrantView on next read.
pub fn refresh_in_background() {
    if safe_mode() { return; }
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    if std::thread::Builder::new().name("figcraft-plugin-refresh".into()).spawn(|| {
        let mut backoff = 60u64;
        loop {
            let result = manager().and_then(|m| {
                let now = craft_ext_store::store::now();
                let last = m.store.state().map_err(|e|e.to_string())?.last_refresh;
                if now.saturating_sub(last) < 6 * 3600 {
                    return Ok((6u64 * 3600).saturating_sub(now.saturating_sub(last)));
                }
                m.refresh(now).map_err(|e|e.to_string())?;
                Ok(6 * 3600)
            });
            let delay = match result {
                Ok(delay) => { backoff = 60; delay },
                Err(e) => { log::info!("plugins: catalogue refresh failed: {e}"); let delay=backoff; backoff=(backoff*2).min(15*60); delay },
            };
            std::thread::sleep(Duration::from_secs(delay.max(1)));
        }
    }).is_err() { STARTED.store(false, std::sync::atomic::Ordering::Relaxed); }
}

/// `figcraft-cli plugin …`: `plugin contract` prints the contract reference, `plugin project <file>`
/// prints the project's plugin statuses (never downloads); the rest goes to the shared library.
pub fn run_cli(args: &[String], project: Option<&[craft_ext_core::project::PluginRef]>, out: &mut dyn std::io::Write, err: &mut dyn std::io::Write) -> i32 {
    if args.first().is_some_and(|a| a == "contract") {
        return match contract() {
            Ok(c) => i32::from(write!(out, "{}", c.to_markdown()).is_err()),
            Err(e) => {
                let _ = writeln!(err, "error: {e}");
                1
            }
        };
    }
    let m = match manager() {
        Ok(m) => m,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            return 1;
        }
    };
    if let Some(refs) = project {
        return match m.check_project(refs) {
            Ok(statuses) => {
                let rows: Vec<Value> = statuses.iter().map(|(r, s)| serde_json::json!({"id": r.id.as_str(), "name": r.name, "version": r.version.to_string(), "status": format!("{s:?}")})).collect();
                let json = args.iter().any(|a| a == "--json");
                i32::from(craft_ext_cli::emit(out, &Value::Array(rows), json).is_err())
            }
            Err(e) => {
                let _ = writeln!(err, "error: {e}");
                1
            }
        };
    }
    craft_ext_cli::run_plugin(&m, args, out, err)
}

#[cfg(test)]
mod tests {
    #[test]
    fn plugin_contract_prints_the_contract_markdown() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(super::run_cli(&["contract".into()], None, &mut out, &mut err), 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("FigCraft") && text.contains("node.create"), "{text}");
    }
}
