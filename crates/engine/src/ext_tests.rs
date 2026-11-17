use crate::ext::{PluginCommand, PluginRuntime, PluginSurface};
use crate::{Result, Session};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    busy: Mutex<bool>,
}
impl PluginRuntime for Fake {
    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }
    fn run_command(&self, _: &mut Session, _: &str, _: &Value) -> Result<Value> {
        Ok(Value::Null)
    }
    fn pump(&self, _: &mut Session) {}
    fn surfaces(&self) -> Vec<PluginSurface> {
        Vec::new()
    }
    fn ui_event(&self, _: &mut Session, _: &str, _: u64, _: &str, _: Value) {}
    fn set_open(&self, _: &str, _: bool) {}
    fn tool_event(&self, _: &mut Session, _: &str, _: [f64; 2], _: Value) {}
    fn active_tool(&self) -> bool {
        true
    }
    fn busy(&self) -> bool {
        *self.busy.lock().unwrap()
    }
    fn overlays(&self) -> Vec<Value> {
        Vec::new()
    }
}

#[test]
fn navigation_commands_run_during_a_plugin_gesture_and_edits_do_not() {
    let mut s = Session::new();
    let rt = Arc::new(Fake::default());
    s.plugins = Some(rt.clone());
    *rt.busy.lock().unwrap() = true;
    let zoom = s.ds().unwrap().view.zoom;
    s.execute("view.zoomIn", &json!({})).unwrap();
    assert!(s.ds().unwrap().view.zoom > zoom);
    s.execute("view.pan", &json!({"dx": 1})).unwrap();
    assert!(
        s.execute("node.create", &json!({"type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10})).is_err(),
        "edits wait for the gesture"
    );
}

#[test]
fn a_rolled_back_group_keeps_the_view_it_ended_with() {
    let mut s = Session::new();
    let before = s.doc().unwrap().clone();
    let mark = s.begin_group();
    s.execute("node.create", &json!({"type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    s.execute("view.zoomIn", &json!({})).unwrap();
    let view = s.ds().unwrap().view.clone();
    s.finish_group(mark, "gesture", false);
    assert_eq!(*s.doc().unwrap(), before, "the edit rolled back");
    assert_eq!(s.ds().unwrap().view, view, "the zoom stayed");
}

