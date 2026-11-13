//! Plugin glue: viewport commands during a plugin gesture, the view across a rolled-back group, and
//! the session commands for paused plugins.
use crate::ext::{PausedPlugin, PluginCommand, PluginRuntime, PluginSurface};
use crate::{Result, Session};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    busy: Mutex<bool>,
    resumed: Mutex<Vec<String>>,
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
    fn paused(&self) -> Vec<PausedPlugin> {
        vec![PausedPlugin { id: "org.example.p".into(), name: "P".into(), reason: "trap".into() }]
    }
    fn resume(&self, _: &mut Session, plugin: &str) -> std::result::Result<(), String> {
        self.resumed.lock().unwrap().push(plugin.into());
        Ok(())
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
    assert!(s.execute("plugins.resume", &json!({"id": "org.example.p"})).is_err(), "no resume mid-gesture");
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

#[test]
fn paused_plugins_are_listed_and_resumed_by_command() {
    let mut s = Session::new();
    assert_eq!(s.execute("plugins.paused", &json!({})).unwrap(), json!([]), "no runtime: nothing paused");
    assert!(s.execute("plugins.resume", &json!({"id": "x.y"})).is_err());
    let rt = Arc::new(Fake::default());
    s.plugins = Some(rt.clone());
    assert_eq!(s.execute("plugins.paused", &json!({})).unwrap()[0]["id"], "org.example.p");
    assert!(s.execute("plugins.resume", &json!({})).is_err(), "the id is required");
    s.execute("plugins.resume", &json!({"id": "org.example.p"})).unwrap();
    assert_eq!(*rt.resumed.lock().unwrap(), vec!["org.example.p".to_string()]);
}
