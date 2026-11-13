//! Plugin hosting in the window: canvas navigation with a plugin tool selected (never sent to the
//! plugin; gesture coordinates follow the view), docked plugin panels, and the canvas widget's
//! pointer events.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, TouchPhase, vec2};
use figcraft_engine::Session;
use figcraft_engine::ext::{PluginCommand, PluginRuntime, PluginSurface};
use serde_json::{Value, json};

use crate::plugin_dock::{Layout, Side};
use crate::{FigApp, NoServices};

/// A plugin runtime with a tool selected that records what it is sent.
#[derive(Default)]
struct Fake {
    events: Mutex<Vec<(String, [f64; 2])>>,
    busy: Mutex<bool>,
    surfaces: Mutex<Vec<PluginSurface>>,
}
impl PluginRuntime for Fake {
    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }
    fn run_command(&self, _: &mut Session, _: &str, _: &Value) -> figcraft_engine::Result<Value> {
        Ok(Value::Null)
    }
    fn pump(&self, _: &mut Session) {}
    fn surfaces(&self) -> Vec<PluginSurface> {
        self.surfaces.lock().unwrap().clone()
    }
    fn ui_event(&self, _: &mut Session, _: &str, _: u64, _: &str, _: Value) {}
    fn set_open(&self, key: &str, open: bool) {
        for s in self.surfaces.lock().unwrap().iter_mut().filter(|s| s.key == key) {
            s.open = open;
        }
    }
    fn tool_event(&self, _: &mut Session, phase: &str, at: [f64; 2], _: Value) {
        let mut busy = self.busy.lock().unwrap();
        if phase == "begin" {
            *busy = true;
        } else if !*busy {
            return;
        }
        if phase == "commit" || phase == "cancel" {
            *busy = false;
        }
        self.events.lock().unwrap().push((phase.into(), at));
    }
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

struct Win {
    ctx: egui::Context,
    app: FigApp,
    rt: Arc<Fake>,
    time: f64,
    pointer: Pos2,
}

impl Win {
    fn new() -> Self {
        let rt = Arc::new(Fake::default());
        let mut session = Session::new();
        session.plugins = Some(rt.clone());
        let mut w =
            Self { ctx: egui::Context::default(), app: FigApp::new(session, Box::new(NoServices)), rt, time: 0.0, pointer: Pos2::new(720.0, 450.0) };
        for _ in 0..3 {
            w.frame(vec![], Modifiers::NONE);
        }
        w.pointer = w.app.canvas.rect.center();
        w.frame(vec![], Modifiers::NONE);
        w
    }

    fn frame(&mut self, mut events: Vec<Event>, modifiers: Modifiers) {
        self.time += 1.0 / 60.0;
        events.splice(0..0, [Event::ModifiersChanged(modifiers), Event::PointerMoved(self.pointer)]);
        let input =
            RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))), time: Some(self.time), events, ..Default::default() };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.ui(ui));
        out.textures_delta.clear();
    }

    fn button(&mut self, button: PointerButton, pressed: bool, modifiers: Modifiers) {
        self.frame(vec![Event::PointerButton { pos: self.pointer, button, pressed, modifiers }], modifiers);
    }

    fn move_to(&mut self, p: Pos2, modifiers: Modifiers) {
        self.pointer = p;
        self.frame(vec![], modifiers);
    }

    fn drag(&mut self, button: PointerButton, by: egui::Vec2, modifiers: Modifiers) {
        self.button(button, true, modifiers);
        for i in 1..=5 {
            let p = self.pointer + by / 5.0 * i as f32 - by / 5.0 * (i - 1) as f32;
            self.move_to(p, modifiers);
        }
        self.button(button, false, modifiers);
        self.frame(vec![], modifiers);
    }

    fn view(&self) -> (f64, f64, f64) {
        let v = &self.app.session.ds().unwrap().view;
        (v.zoom, v.cx, v.cy)
    }

    fn page(&self, p: Pos2) -> [f64; 2] {
        let q = self.app.to_page(p);
        [q.x, q.y]
    }

    fn sent(&self) -> Vec<(String, [f64; 2])> {
        self.rt.events.lock().unwrap().clone()
    }
}

fn zoom_wheel() -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        source: MouseWheelSource::Unknown,
        delta: vec2(0.0, 3.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    }
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

#[test]
fn wheel_zoom_scroll_and_middle_or_space_drag_navigate_and_never_reach_the_plugin() {
    let mut w = Win::new();
    let before = w.view();
    w.frame(vec![zoom_wheel()], Modifiers::COMMAND);
    for _ in 0..30 {
        w.frame(vec![], Modifiers::NONE);
    }
    assert!(w.view().0 > before.0, "Cmd/Ctrl+wheel zooms: {before:?} → {:?}", w.view());
    let before = w.view();
    w.frame(
        vec![Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            source: MouseWheelSource::Unknown,
            delta: vec2(0.0, -3.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
    );
    for _ in 0..30 {
        w.frame(vec![], Modifiers::NONE);
    }
    assert!(w.view().2 > before.2, "the wheel scrolls");
    let before = w.view();
    w.drag(PointerButton::Middle, vec2(100.0, 0.0), Modifiers::NONE);
    assert!(w.view().1 < before.1 - 1.0, "middle-drag pans: {before:?} → {:?}", w.view());
    // Space + primary drag pans, and its release is no click for the plugin.
    let before = w.view();
    w.frame(
        vec![Event::Key { key: egui::Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
        Modifiers::NONE,
    );
    w.button(PointerButton::Primary, true, Modifiers::NONE);
    for _ in 0..5 {
        let p = w.pointer + vec2(0.0, 20.0);
        w.move_to(p, Modifiers::NONE);
    }
    w.frame(
        vec![Event::Key { key: egui::Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }],
        Modifiers::NONE,
    );
    w.button(PointerButton::Primary, false, Modifiers::NONE);
    w.frame(vec![], Modifiers::NONE);
    assert!(w.view().2 < before.2 - 1.0, "space-drag pans: {before:?} → {:?}", w.view());
    // A quick Space+click is navigation too.
    w.frame(
        vec![Event::Key { key: egui::Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
        Modifiers::NONE,
    );
    w.button(PointerButton::Primary, true, Modifiers::NONE);
    w.button(PointerButton::Primary, false, Modifiers::NONE);
    w.frame(
        vec![Event::Key { key: egui::Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }],
        Modifiers::NONE,
    );
    assert!(w.sent().is_empty(), "navigation reached the plugin: {:?}", w.sent());
    // Zoom shortcuts (commands) too.
    let z = w.view().0;
    w.app.run("view.zoomIn", json!({}));
    assert!(w.view().0 > z);
    assert!(w.sent().is_empty());
}

#[test]
fn a_gesture_gets_page_coordinates_that_follow_a_zoom_or_pan_mid_gesture() {
    let mut w = Win::new();
    let press = w.pointer;
    let at_press = w.page(press);
    w.button(PointerButton::Primary, true, Modifiers::NONE);
    let p = press + vec2(40.0, 30.0);
    w.move_to(p, Modifiers::NONE);
    w.move_to(p + vec2(1.0, 0.0), Modifiers::NONE);
    let p = p + vec2(1.0, 0.0);
    let sent = w.sent();
    assert_eq!(sent.first().map(|e| e.0.as_str()), Some("begin"), "{sent:?}");
    assert!(close(sent[0].1, at_press), "begin is at the press point: {:?} vs {at_press:?}", sent[0].1);
    assert!(close(sent.last().unwrap().1, w.page(p)));
    // Wheel zoom keeps the page point under the pointer; the gesture carries on.
    let (zoom, at_before) = (w.view().0, w.page(p));
    w.frame(vec![zoom_wheel()], Modifiers::COMMAND);
    for _ in 0..30 {
        w.frame(vec![], Modifiers::NONE);
    }
    assert!(w.view().0 > zoom);
    assert!(close(w.sent().last().unwrap().1, w.page(p)));
    // A zoom shortcut (about the view's centre) with the button still held and the pointer still:
    // the plugin hears the pointer's new page point.
    let at_before = {
        let _ = at_before;
        w.page(p)
    };
    w.app.run("view.zoomIn", json!({}));
    w.frame(vec![], Modifiers::NONE);
    let (phase, at) = w.sent().last().cloned().unwrap();
    assert_eq!(phase, "update");
    assert!(!close(at, at_before) && close(at, w.page(p)), "update at the zoomed page point: {at:?} vs {:?}", w.page(p));
    // A middle-drag pan mid-gesture is navigation: no update while panning, the pointer's new page
    // point afterwards.
    let n = w.sent().len();
    let cx = w.view().1;
    w.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE }], Modifiers::NONE);
    w.move_to(p + vec2(60.0, 0.0), Modifiers::NONE);
    w.move_to(p, Modifiers::NONE);
    w.move_to(p + vec2(-60.0, 0.0), Modifiers::NONE);
    w.frame(
        vec![Event::PointerButton { pos: w.pointer, button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE }],
        Modifiers::NONE,
    );
    assert!((w.view().1 - cx).abs() > 1.0, "panned");
    // Still primary-held; the gesture continues once the pan is over.
    w.move_to(w.pointer + vec2(0.0, 1.0), Modifiers::NONE);
    let sent = w.sent();
    assert!(sent[n..].iter().all(|e| e.0 == "update"));
    assert!(close(sent.last().unwrap().1, w.page(w.pointer)));
    w.button(PointerButton::Primary, false, Modifiers::NONE);
    let (phase, at) = w.sent().last().cloned().unwrap();
    assert_eq!(phase, "commit");
    assert!(close(at, w.page(w.pointer)));
    assert!(!w.rt.busy());
}

fn panel(key: &str) -> PluginSurface {
    PluginSurface { key: key.into(), title: key.into(), tree: Some(json!({"type":"label","text":"hello"})), rev: 1, window: false, open: false }
}

#[test]
fn panels_dock_with_saved_layout_and_come_back_where_they_were() {
    let mut w = Win::new();
    *w.rt.surfaces.lock().unwrap() = vec![panel("p.a/one"), panel("p.b/two")];
    w.frame(vec![], Modifiers::NONE);
    let layout = Layout::read(&w.app.session.prefs);
    assert!(!layout.visible("p.a/one"), "panels start hidden");
    assert!(w.app.session.prefs.opaque.contains_key(crate::plugin_dock::PREF_KEY));
    let canvas = w.app.canvas.rect;
    // Shown from its menu item: docked (the canvas gets narrower), the runtime's open flag follows.
    crate::plugin_dock::toggle(&mut w.app, "p.a/one");
    w.frame(vec![], Modifiers::NONE);
    w.frame(vec![], Modifiers::NONE);
    assert!(w.rt.surfaces()[0].open);
    assert!(w.app.canvas.rect.width() < canvas.width() - 100.0, "the dock takes room beside the canvas");
    assert!(w.app.canvas.rect.max.x < canvas.max.x - 100.0, "on the right");
    // Moved to the left sidebar, with a width of its own.
    let mut layout = Layout::read(&w.app.session.prefs);
    layout.move_to("p.a/one", Side::Left);
    layout.set_width(Side::Left, 300.0);
    layout.write(&mut w.app.session.prefs);
    w.frame(vec![], Modifiers::NONE);
    w.frame(vec![], Modifiers::NONE);
    assert!(w.app.canvas.rect.min.x > canvas.min.x + 250.0, "docked on the left: {:?}", w.app.canvas.rect);
    // Its plugin is disabled: the panel disappears and the canvas takes the room back.
    w.rt.surfaces.lock().unwrap().retain(|s| s.key != "p.a/one");
    w.frame(vec![], Modifiers::NONE);
    w.frame(vec![], Modifiers::NONE);
    assert!((w.app.canvas.rect.min.x - canvas.min.x).abs() < 1.0);
    // Enabled again (the runtime makes it closed): it reopens on the left at its width.
    w.rt.surfaces.lock().unwrap().push(panel("p.a/one"));
    w.frame(vec![], Modifiers::NONE);
    w.frame(vec![], Modifiers::NONE);
    assert!(w.rt.surfaces().iter().find(|s| s.key == "p.a/one").unwrap().open);
    assert!((w.app.canvas.rect.min.x - canvas.min.x - 300.0).abs() < 2.0, "{:?}", w.app.canvas.rect);
    // Hidden again from the menu.
    crate::plugin_dock::toggle(&mut w.app, "p.a/one");
    w.frame(vec![], Modifiers::NONE);
    assert!(!w.rt.surfaces().iter().find(|s| s.key == "p.a/one").unwrap().open);
}

#[test]
fn layout_tabs_order_activity_and_limits() {
    let mut l = Layout::default();
    let keys: Vec<String> = ["a/1", "b/2", "c/3"].iter().map(|s| s.to_string()).collect();
    for k in &keys {
        l.set_visible(k, true);
    }
    assert_eq!(l.tabs(Side::Right, &keys), keys);
    assert_eq!(l.active_tab(Side::Right, &keys).as_deref(), Some("c/3"), "the panel shown last is in front");
    l.move_to("a/1", Side::Left);
    assert_eq!(l.tabs(Side::Left, &keys), ["a/1"]);
    l.move_to("a/1", Side::Right);
    assert_eq!(l.tabs(Side::Right, &keys), ["b/2", "c/3", "a/1"], "moved back: last");
    // Absent (plugin disabled) panels are not tabs but keep their place.
    assert_eq!(l.tabs(Side::Right, &keys[1..]), ["b/2", "c/3"]);
    assert!(l.visible("a/1"));
    l.set_width(Side::Right, f32::NAN);
    assert_eq!(l.width(Side::Right), crate::plugin_dock::DOCK_DEFAULT);
    l.set_width(Side::Right, 1e9);
    assert_eq!(l.width(Side::Right), *crate::plugin_dock::DOCK_WIDTH.end());
    let mut prefs = figcraft_engine::Prefs::default();
    prefs.opaque.insert(crate::plugin_dock::PREF_KEY.into(), json!("junk"));
    assert_eq!(Layout::read(&prefs), Layout::default(), "a broken setting reads as the default");
    assert!(l.write(&mut prefs) && !l.write(&mut prefs));
    assert_eq!(Layout::read(&prefs), l);
}

/// The canvas widget in a plugin panel: press → `down` at the press point, moves → `drag`, release
/// → `up`, in the widget's own coordinates; a click is a `down` and an `up`.
#[test]
fn canvas_widget_reports_down_drag_and_up_in_canvas_coordinates() {
    let node: craft_ext_core::ui::Node =
        serde_json::from_value(json!({"type":"canvas","id":"pad","label":"Pad","width":200.0,"height":100.0,"ops":[]})).unwrap();
    let ctx = egui::Context::default();
    let origin = Pos2::new(20.0, 30.0);
    let mut time = 0.0;
    let mut run = |events: Vec<Event>| -> Vec<(String, Value)> {
        time += 1.0 / 60.0;
        let input =
            RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), time: Some(time), events, ..Default::default() };
        let mut got = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            egui::Area::new(egui::Id::unique("plugin-canvas-test")).fixed_pos(origin).show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                got = crate::plugins_ui::render_surface(ui, "p/panel", 1, &node);
            });
        });
        out.textures_delta.clear();
        got
    };
    let at = |p: Pos2| Event::PointerMoved(p);
    let press = |p: Pos2, down: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: Modifiers::NONE };
    for _ in 0..3 {
        run(vec![]);
    }
    let mut all = Vec::new();
    let p = origin + vec2(10.0, 20.0);
    run(vec![at(p)]);
    all.extend(run(vec![press(p, true)]));
    for i in 1..=4 {
        all.extend(run(vec![at(p + vec2(10.0 * i as f32, 5.0 * i as f32))]));
    }
    let end = p + vec2(40.0, 20.0);
    all.extend(run(vec![press(end, false)]));
    all.extend(run(vec![]));
    let kinds: Vec<&str> = all.iter().map(|(_, v)| v["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"down"), "{all:?}");
    assert_eq!(kinds.last(), Some(&"up"), "{all:?}");
    assert!(kinds[1..kinds.len() - 1].iter().all(|k| *k == "drag") && kinds.len() >= 3, "{all:?}");
    assert_eq!(all[0].1["at"], json!([10.0, 20.0]), "down at the press point");
    assert_eq!(all.last().unwrap().1["at"], json!([50.0, 40.0]));
    // A click: down then up, each once.
    let q = origin + vec2(100.0, 50.0);
    run(vec![at(q)]);
    let mut click = run(vec![press(q, true)]);
    click.extend(run(vec![press(q, false)]));
    click.extend(run(vec![]));
    assert_eq!(click.iter().map(|(_, v)| v["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["down", "up"], "{click:?}");
    assert!(click.iter().all(|(id, v)| id == "pad" && v["at"] == json!([100.0, 50.0])));
}
