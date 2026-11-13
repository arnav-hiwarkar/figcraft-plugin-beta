//! Document-thread boundary for extension workers.
use crate::{Session, DocState, HistoryEntry, Result};
use serde_json::Value;

#[derive(Clone)]
pub struct PluginCommand { pub id: String, pub label: String, pub plugin: String, pub tool: bool }
/// A plugin paused after repeated failures this session (never saved): its id, name and last error.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PausedPlugin { pub id: String, pub name: String, pub reason: String }
/// Viewport-only commands: they never touch the document, so they run while a plugin tool gesture
/// is open (the gesture's coordinates are recomputed from the new view on the next pointer event).
pub const NAVIGATION_COMMANDS: &[&str] = &[
    "view.zoomIn", "view.zoomOut", "view.zoom100", "view.zoom50", "view.zoom200", "view.zoomFit",
    "view.zoomSelection", "view.zoomAt", "view.pan", "view.set", "view.get",
];
#[derive(Clone)]
pub struct PluginSurface { pub key: String, pub title: String, pub tree: Option<Value>, pub rev: u64, pub window: bool, pub open: bool }
pub trait PluginRuntime: Send + Sync {
    fn commands(&self) -> Vec<PluginCommand>;
    fn run_command(&self, s: &mut Session, id: &str, params: &Value) -> Result<Value>;
    fn pump(&self, s: &mut Session);
    fn surfaces(&self) -> Vec<PluginSurface>;
    fn ui_event(&self, s: &mut Session, key: &str, rev: u64, widget: &str, value: Value);
    fn set_open(&self, key: &str, open: bool);
    fn tool_event(&self, s: &mut Session, phase: &str, at: [f64; 2], modifiers: Value);
    fn active_tool(&self) -> bool;
    /// The command id of the selected plugin tool, for the toolbar's highlight.
    fn selected_tool(&self) -> Option<String> { None }
    fn busy(&self) -> bool;
    fn overlays(&self) -> Vec<Value>;
    /// Plugins paused after three failures this session.
    fn paused(&self) -> Vec<PausedPlugin> { Vec::new() }
    /// Clears a paused plugin's failure count and starts fresh instances of its modules.
    fn resume(&self, _s: &mut Session, _plugin: &str) -> std::result::Result<(), String> { Err("plugin is not paused".into()) }
}
pub struct GroupMark { docs: Vec<DocState>, active: usize, journal: Vec<(String, Value)>, pending: usize, last: Option<(String, Value)> }
impl Session {
    pub fn pump_plugins(&mut self) { if let Some(rt) = self.plugins.clone() { rt.pump(self); } }
    /// `plugins.paused` and `plugins.resume {id}`: session plugin state, reachable from scripts
    /// (`figcraft-cli run --plugins`), automation and the UI. `None` for any other command.
    pub(crate) fn plugin_session_command(&mut self, id: &str, params: &Value) -> Option<Result<Value>> {
        let rt = self.plugins.clone();
        match id {
            "plugins.paused" => Some(Ok(serde_json::json!(rt.map(|rt| rt.paused()).unwrap_or_default()))),
            "plugins.resume" => Some((|| {
                let rt = rt.ok_or_else(|| crate::EngineError::Other("plugins are not loaded in this session".into()))?;
                let plugin = params.get("id").and_then(Value::as_str).ok_or_else(|| crate::EngineError::Other("plugins.resume needs {\"id\": plugin id}".into()))?;
                if rt.busy() { return Err(crate::EngineError::Other("a plugin gesture is in progress".into())); }
                rt.resume(self, plugin).map_err(crate::EngineError::Other)?;
                Ok(serde_json::json!({"resumed": plugin}))
            })()),
            _ => None,
        }
    }
    pub fn begin_group(&mut self) -> GroupMark {
        let mark = GroupMark { docs: self.docs.clone(), active: self.active, journal: std::mem::take(&mut self.journal), pending: self.pending_events.len(), last: self.last_command.clone() };
        for d in &mark.docs { self.live_hold(d.uid, true); }
        self.group_depth += 1;
        for d in &mut self.docs { d.history.limit = usize::MAX; }
        mark
    }
    pub fn finish_group(&mut self, mark: GroupMark, label: &str, commit: bool) {
        let uids: Vec<_> = mark.docs.iter().map(|d| d.uid).collect();
        let pending = std::mem::replace(&mut self.journal, mark.journal);
        self.group_depth = self.group_depth.saturating_sub(1);
        if !commit {
            // The view is not part of the transaction: zooming or panning during a gesture stays.
            let views: Vec<_> = self.docs.iter().map(|d| (d.uid, d.view.clone())).collect();
            self.docs = mark.docs; self.active = mark.active; self.last_command = mark.last;
            for d in &mut self.docs { if let Some((_, v)) = views.iter().find(|(uid, _)| *uid == d.uid) { d.view = v.clone(); } }
            self.pending_events.truncate(mark.pending);
            // Invalidate render caches even if a preview was drawn during a gesture.
            for d in &mut self.docs { d.revision = d.revision.wrapping_add(1); }
        } else {
            self.journal.extend(pending);
            for before in mark.docs {
                let live = self.live_doc(before.uid);
                if let Some(d) = self.docs.iter_mut().find(|d| d.uid == before.uid) {
                    let changed = !std::sync::Arc::ptr_eq(&before.doc, &d.doc);
                    d.history = before.history;
                    if changed && !live {
                        d.history.redo.clear();
                        d.history.undo.push(HistoryEntry { label: label.into(), doc: before.doc, selection: before.selection, page: before.page });
                        let excess = d.history.undo.len().saturating_sub(d.history.limit.max(1));
                        d.history.undo.drain(..excess);
                    }
                }
            }
        }
        for uid in uids {
            self.live_hold(uid, false);
            if commit { self.live_edit(uid, Some(label)); }
        }
        if self.group_depth == 0 { self.flush_pending_events(); }
    }
}
