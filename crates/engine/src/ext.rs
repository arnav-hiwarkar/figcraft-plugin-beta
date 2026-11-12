//! Document-thread boundary for extension workers.
use crate::{Session, DocState, HistoryEntry, Result};
use serde_json::Value;

#[derive(Clone)]
pub struct PluginCommand { pub id: String, pub label: String, pub plugin: String, pub tool: bool }
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
}
pub struct GroupMark { docs: Vec<DocState>, active: usize, journal: Vec<(String, Value)>, pending: usize, last: Option<(String, Value)> }
impl Session {
    pub fn pump_plugins(&mut self) { if let Some(rt) = self.plugins.clone() { rt.pump(self); } }
    pub fn begin_group(&mut self) -> GroupMark {
        let mark = GroupMark { docs: self.docs.clone(), active: self.active, journal: std::mem::take(&mut self.journal), pending: self.pending_events.len(), last: self.last_command.clone() };
        self.group_depth += 1;
        for d in &mut self.docs { d.history.limit = usize::MAX; }
        mark
    }
    pub fn finish_group(&mut self, mark: GroupMark, label: &str, commit: bool) {
        let pending = std::mem::replace(&mut self.journal, mark.journal);
        self.group_depth = self.group_depth.saturating_sub(1);
        if !commit {
            self.docs = mark.docs; self.active = mark.active; self.last_command = mark.last;
            self.pending_events.truncate(mark.pending);
            // Invalidate render caches even if a preview was drawn during a gesture.
            for d in &mut self.docs { d.revision = d.revision.wrapping_add(1); }
        } else {
            self.journal.extend(pending);
            for before in mark.docs {
                if let Some(d) = self.docs.iter_mut().find(|d| d.uid == before.uid) {
                    let changed = !std::sync::Arc::ptr_eq(&before.doc, &d.doc);
                    d.history = before.history;
                    if changed {
                        d.history.redo.clear();
                        d.history.undo.push(HistoryEntry { label: label.into(), doc: before.doc, selection: before.selection, page: before.page });
                        let excess = d.history.undo.len().saturating_sub(d.history.limit.max(1));
                        d.history.undo.drain(..excess);
                    }
                }
            }
        }
        if self.group_depth == 0 { self.flush_pending_events(); }
    }
}
