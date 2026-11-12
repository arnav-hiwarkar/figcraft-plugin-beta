//! The player's side of the prototyping tail, written from the public documentation: controller
//! buttons play key/gamepad triggers (from the system's controllers and injected through the
//! control channel; no physical controller needed). Each case states the rule and a one-line derivation; "docs silent" cases pin what
//! FigCraft does. The model's side is the engine's `proto_tail_tests`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use figcraft_doc::NodeId;
use figcraft_engine::Session;
use serde_json::{Value, json};

use crate::Services;
use crate::present_tests::{Ui as Harness, id};

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn frame(s: &mut Session, name: &str, x: f64) -> NodeId {
    id(&ex(s, "frame.create", json!({"x": x, "y": 0, "width": 300, "height": 400, "name": name})))
}

fn key_trigger(s: &mut Session, layer: NodeId, key: &str, action: Value) {
    ex(
        s,
        "proto.interaction.add",
        json!({"id": layer, "trigger": {"type": "keyPress", "key": key}, "action": action, "transition": {"type": "instant"}}),
    );
}

fn screen(u: &Harness) -> String {
    u.app.inspect()["presenter"]["name"].as_str().unwrap_or_default().to_string()
}

/// Controllers as the system reports them: the buttons held (bit i: standard index i), and how
/// often the player asked.
struct FakePads {
    held: Arc<AtomicU32>,
    polls: Arc<AtomicUsize>,
}

impl Services for FakePads {
    fn gamepad_held(&mut self) -> u32 {
        self.polls.fetch_add(1, Ordering::Relaxed);
        self.held.load(Ordering::Relaxed)
    }
}

/// Screens A (presented) and B: a layer on A whose "Gamepad A" trigger goes to B, one on B whose
/// "Gamepad B" trigger goes back; the fake controllers attached.
fn pads_setup() -> (Harness, Arc<AtomicU32>, Arc<AtomicUsize>) {
    let mut s = Session::new();
    let a = frame(&mut s, "A", 0.0);
    let b = frame(&mut s, "B", 400.0);
    let on_a = id(&ex(&mut s, "shape.rectangle", json!({"x": 10, "y": 10, "width": 30, "height": 30, "parent": a})));
    let on_b = id(&ex(&mut s, "shape.rectangle", json!({"x": 410, "y": 10, "width": 30, "height": 30, "parent": b})));
    key_trigger(&mut s, on_a, "Gamepad A", json!({"type": "navigate", "dest": b}));
    key_trigger(&mut s, on_b, "Gamepad B", json!({"type": "back"}));
    let (held, polls) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut u = Harness::new(s);
    u.app.services = Box::new(FakePads { held: held.clone(), polls: polls.clone() });
    u.run("proto.present", json!({"start": a}));
    (u, held, polls)
}

#[test]
fn controller_buttons_play_key_gamepad_triggers() {
    let (mut u, held, polls) = pads_setup();
    assert_eq!(u.app.inspect()["presenter"]["gamepad"], json!(true));
    assert_eq!(screen(&u), "A");
    // (buttons held, screen after, derivation)
    let steps = [
        (0b01, "B", "pressing the trigger's button anywhere on the screen plays it, like its key"),
        (0b01, "B", "a button held down plays once, not again every frame"),
        (0b11, "A", "another button while one is held: B's trigger goes back"),
        (0b00, "A", "releasing does nothing (the trigger is a press)"),
        (1 << 2, "A", "a button no trigger names does nothing"),
        (0b01, "B", "pressed again after letting go: plays again"),
    ];
    for (bits, want, why) in steps {
        held.store(bits, Ordering::Relaxed);
        u.frames(2, Vec::new);
        assert_eq!(screen(&u), want, "held {bits:#b}: {why}");
    }
    assert!(polls.load(Ordering::Relaxed) >= steps.len() * 2, "polled every frame while presenting");
    // The control channel presses buttons too: by short name, trigger name or index.
    let ctx = u.ctx.clone();
    held.store(0, Ordering::Relaxed);
    for (button, want) in [(json!("B"), "A"), (json!("gamepad a"), "B"), (json!(1), "A")] {
        crate::control::handle(&mut u.app, &ctx, "ui.input", &json!({"events": [{"gamepad": button}]})).unwrap();
        u.frames(2, Vec::new);
        assert_eq!(screen(&u), want, "{button}");
    }
    for junk in [json!("Z"), json!(17), json!(-1), json!(null), json!([0])] {
        let e = crate::control::handle(&mut u.app, &ctx, "ui.input", &json!({"events": [{"gamepad": junk}]})).unwrap_err();
        assert!(e.contains("unknown controller button"), "{junk}: {e}");
    }
}

#[test]
fn prototypes_without_gamepad_triggers_dont_poll() {
    let mut s = Session::new();
    let a = frame(&mut s, "A", 0.0);
    let held = Arc::new(AtomicU32::new(1));
    let polls = Arc::new(AtomicUsize::new(0));
    let mut u = Harness::new(s);
    u.app.services = Box::new(FakePads { held, polls: polls.clone() });
    let ctx = u.ctx.clone();
    // Nothing plays: a controller button needs a prototype to go to.
    let e = crate::control::handle(&mut u.app, &ctx, "ui.input", &json!({"events": [{"gamepad": "A"}]})).unwrap_err();
    assert!(e.contains("prototype that is playing"), "{e}");
    u.run("proto.present", json!({"start": a}));
    u.frames(5, Vec::new);
    assert_eq!(u.app.inspect()["presenter"]["gamepad"], json!(false));
    assert_eq!(polls.load(Ordering::Relaxed), 0, "no gamepad triggers: the controllers are left alone");
}

#[test]
fn the_interaction_editor_picks_a_controller_button() {
    let mut s = Session::new();
    let a = frame(&mut s, "A", 0.0);
    let layer = id(&ex(&mut s, "shape.rectangle", json!({"x": 10, "y": 10, "width": 30, "height": 30, "parent": a})));
    key_trigger(&mut s, layer, "K", json!({"type": "back"}));
    ex(&mut s, "select.set", json!({"ids": [layer]}));
    let mut u = crate::layers_drag_tests::Ui::new(s);
    u.app.ui.right_tab = crate::RightTab::Prototype;
    crate::proto_panel::open_editor(&u.ctx, Some((layer, 0)));
    u.ctx.enable_accesskit();
    u.frame(vec![]);
    let at = |u: &mut crate::layers_drag_tests::Ui, name: &str| {
        let nodes = u.frame_output(vec![]).platform_output.accesskit_update.unwrap().nodes;
        let b = nodes.iter().map(|(_, n)| n).find(|n| n.label() == Some(name) || n.value() == Some(name)).and_then(|n| n.bounds());
        let b = b.unwrap_or_else(|| panic!("no control {name:?}"));
        let p = egui::Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
        u.button(p, true);
        u.button(p, false);
    };
    // The key field keeps recording keys; the menu beside it names the controller's buttons.
    at(&mut u, "Controller");
    // Each with what PlayStation and Switch controllers call the same button.
    let shown = crate::live_tests::flat(&u.frame_output(vec![]).shapes);
    for names in ["✕ · B", "□ · Y", "L2 · ZL", "Options · +", "PS · Home"] {
        assert!(crate::live_tests::has_text(&shown, names), "{names}");
    }
    at(&mut u, "Gamepad Menu");
    let trigger = u.app.session.doc().unwrap().node(layer).unwrap().interactions[0].trigger.clone();
    assert_eq!(trigger, figcraft_doc::proto::Trigger::KeyPress { key: "Gamepad Menu".into() });
}
