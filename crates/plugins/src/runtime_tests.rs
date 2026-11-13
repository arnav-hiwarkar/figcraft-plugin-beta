use super::*;
use craft_ext_core::tier::Tier;
use craft_ext_store::state::{AppSetup, Origin, PackageRec, ReleaseRec, STATE_FILE, StoreState};
use figcraft_engine::figcraft_collab;

struct Fixture {
    rt: Arc<Runtime>,
    owner: Contribution,
    state: StoreState,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!("figcraft-plugin-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        let rt = Runtime::new(root).unwrap();
        let id = ExtensionId::new("test.plugin").unwrap();
        let version = Version::parse("1.0.0").unwrap();
        let permissions: Vec<Permission> = serde_json::from_value(json!(["document.read:document", "document.edit:document"])).unwrap();
        let owner = Contribution {
            plugin: id.clone(),
            version,
            name: "Test".into(),
            id: "command".into(),
            label: "Test".into(),
            kind: ContributionKind::Command,
            declared: permissions.clone().into(),
            worker: "test".into(),
            reference: PluginRef {
                id: id.clone(),
                version,
                sha256: Some("digest".into()),
                source: Source::Local,
                name: "Test".into(),
                contributions: vec!["command".into()],
                state_versions: Some(BTreeMap::from([("command".into(), 1)])),
            },
        };
        let mut state = StoreState::default();
        state.packages.insert(
            id.clone(),
            PackageRec {
                origin: Origin::Dev,
                releases: BTreeMap::from([(version, ReleaseRec { origin: None, sha256: "digest".into(), tier: Tier::Dev, installed: 0 })]),
            },
        );
        state.apps.insert(
            rt.app.clone(),
            BTreeMap::from([(
                id,
                AppSetup {
                    active: Some(version),
                    enabled: ["command".into()].into(),
                    grants: permissions.into_iter().collect(),
                    ..Default::default()
                },
            )]),
        );
        lock(&rt.state).contributions.push(owner.clone());
        let f = Self { rt, owner, state };
        f.save();
        f
    }
    fn save(&self) {
        std::fs::write(self.rt.root.join(STATE_FILE), serde_json::to_vec(&self.state).unwrap()).unwrap();
    }
    fn dispatch(&self, s: &mut Session, calls: Vec<(&str, Value)>, commit: bool) -> (Vec<Result<Value, OpError>>, Result<(), String>) {
        let inv = self.rt.invocation(&self.owner);
        let context =
            Dispatch { read_only: false, uid: s.ds().map_or(0, |d| d.uid), hash: Some("digest".into()), surface: None, completion: Arc::new(Mutex::new(None)) };
        let mut replies = Vec::new();
        let kinds = std::iter::once(Kind::Begin)
            .chain(calls.into_iter().map(|(op, a)| Kind::Call(op.into(), a)))
            .chain(std::iter::once(Kind::Finish(commit)));
        for kind in kinds {
            let (reply, rx) = mpsc::channel();
            self.rt.tx.send(Request { inv: inv.clone(), context: context.clone(), kind, reply }).unwrap();
            replies.push(rx);
        }
        self.rt.serve(s);
        let replies = replies.into_iter().map(|r| r.recv_timeout(Duration::from_secs(1)).unwrap()).collect();
        let done = lock(&context.completion).take().unwrap();
        (replies, done)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.rt.root);
    }
}

#[test]
fn invalid_operation_is_recoverable_and_read_only_event_stays_clean() {
    let f = Fixture::new();
    let mut s = Session::new();
    let before = s.ds().unwrap().doc.clone();
    let (replies, done) = f.dispatch(&mut s, vec![("node.get", json!({"id":99999})), ("document.info", json!({}))], true);
    assert!(replies[1].is_err());
    assert!(replies[2].is_ok());
    assert!(done.is_ok());
    assert!(Arc::ptr_eq(&before, &s.ds().unwrap().doc));
    assert!(s.ds().unwrap().history.undo.is_empty());
    assert!(s.doc().unwrap().plugins.is_empty());
}

#[test]
fn no_document_init_can_use_ui_and_recover_from_document_error() {
    let f = Fixture::new();
    let mut s = Session::empty();
    let (replies, done) = f.dispatch(&mut s, vec![("document.info", json!({})), ("ui.notify", json!({"message":"ready"}))], true);
    assert!(replies[0].is_ok());
    assert!(replies[1].as_ref().unwrap_err().message.contains("no document"));
    assert!(replies[2].is_ok());
    assert!(done.is_ok());
}

#[test]
fn explicit_rollback_reports_failure_even_after_successful_operations() {
    let f = Fixture::new();
    let mut s = Session::new();
    let before = s.ds().unwrap().doc.clone();
    let (_, done) = f.dispatch(&mut s, vec![("node.create", json!({"type":"rectangle","x":1,"y":2,"width":10,"height":10}))], false);
    assert!(done.is_err());
    assert_eq!(*before, *s.ds().unwrap().doc);
    assert!(s.ds().unwrap().history.undo.is_empty());
}

#[test]
fn grant_reads_fail_closed_and_generation_changes_are_immediate() {
    let mut f = Fixture::new();
    let inv = f.rt.invocation(&f.owner);
    assert_eq!(f.rt.grants.generation(&f.owner.plugin), 0);
    f.state.apps.get_mut(&f.rt.app).unwrap().get_mut(&f.owner.plugin).unwrap().grant_generation += 1;
    f.save();
    let (reply, _) = mpsc::channel();
    let s = Session::new();
    let r = Request {
        inv,
        context: Dispatch { read_only: false, uid: s.ds().unwrap().uid, hash: Some("digest".into()), surface: None, completion: Arc::new(Mutex::new(None)) },
        kind: Kind::Begin,
        reply,
    };
    assert!(f.rt.validate(&s, &r).is_err());
    assert_eq!(f.rt.grants.generation(&f.owner.plugin), 1);
    std::fs::write(f.rt.root.join(STATE_FILE), "invalid").unwrap();
    assert!(f.rt.grants.stopped(&f.owner.plugin, f.owner.version));
    assert!(!f.rt.grants.allowed(&f.owner.plugin, &f.owner.declared[0]));
}

#[test]
fn references_survive_collaboration_diff_snapshot_and_server_restart() {
    use figcraft_collab::replica::Mode;
    use figcraft_collab::{Limits, Replica, Server};
    let f = Fixture::new();
    let doc = figcraft_doc::Document::new("Project");
    let mut changed = doc.clone();
    changed.plugins.push(f.owner.reference.clone());
    let mut replica = Replica::new(doc);
    let diff = figcraft_collab::diff(&replica, &changed).unwrap();
    replica.apply(&diff.changes, Mode::Strict).unwrap();
    assert_eq!(replica.document().plugins, changed.plugins);
    let snapshot = replica.snapshot().unwrap();
    assert_eq!(Replica::from_snapshot(&snapshot).unwrap().document().plugins, changed.plugins);
    let server = Server::new(changed.clone(), "token", Limits::default());
    let state = serde_json::from_slice(&serde_json::to_vec(&server.state().unwrap()).unwrap()).unwrap();
    assert_eq!(Server::restore(state, "token", Limits::default()).unwrap().document().plugins, changed.plugins);
}

#[test]
fn edits_record_reference_once_and_form_one_undo_step() {
    let f = Fixture::new();
    let mut s = Session::new();
    let (r, done) = f.dispatch(&mut s, vec![("node.create", json!({"type":"rectangle","x":1,"y":2,"width":10,"height":10}))], true);
    assert!(r[1].is_ok(), "{:?}", r[1]);
    assert!(done.is_ok());
    assert_eq!(s.doc().unwrap().plugins, vec![f.owner.reference.clone()]);
    assert_eq!(s.ds().unwrap().history.undo.len(), 1);
    let before = s.ds().unwrap().doc.clone();
    let (_, done) = f.dispatch(&mut s, vec![("document.info", json!({}))], true);
    assert!(done.is_ok());
    assert!(Arc::ptr_eq(&before, &s.ds().unwrap().doc));
    assert_eq!(s.ds().unwrap().history.undo.len(), 1);
}

fn mock_worker(f: &Fixture, rollback_commit: bool) -> std::thread::JoinHandle<()> {
    let (tx, rx) = mpsc::sync_channel::<Job>(4);
    lock(&f.rt.state).workers.insert(f.owner.worker.clone(), tx);
    let requests = f.rt.tx.clone();
    std::thread::spawn(move || {
        for job in rx {
            let phase = job.event["phase"].as_str().unwrap_or("");
            let mut kinds = vec![Kind::Begin];
            if phase == "begin" {
                kinds.push(Kind::Call("node.create".into(), json!({"type":"rectangle","x":1,"y":2,"width":10,"height":10})));
            }
            kinds.push(Kind::Finish(!(rollback_commit && phase == "commit")));
            for kind in kinds {
                let (reply, rx) = mpsc::channel();
                requests.send(Request { inv: job.inv.clone(), context: job.context.clone(), kind, reply }).unwrap();
                let result = rx.recv_timeout(Duration::from_secs(2)).unwrap();
                // Simulate a guest which ignores errors and returns status zero.
                if phase == "begin" {
                    assert!(result.is_ok(), "{result:?}");
                }
            }
            job.done.send(Ok(())).unwrap();
        }
    })
}

#[test]
fn tool_commit_rechecks_original_generation_and_rolls_back_all_phases() {
    let mut f = Fixture::new();
    let mut s = Session::new();
    let original = s.doc().unwrap().clone();
    let worker = mock_worker(&f, false);
    lock(&f.rt.state).selected_tool = Some(f.owner.clone());
    f.rt.tool_event(&mut s, "begin", [0., 0.], Value::Null);
    assert_ne!(*s.doc().unwrap(), original);
    f.state.apps.get_mut(&f.rt.app).unwrap().get_mut(&f.owner.plugin).unwrap().grant_generation += 1;
    f.save();
    f.rt.tool_event(&mut s, "commit", [0., 0.], Value::Null);
    assert_eq!(*s.doc().unwrap(), original);
    assert!(s.ds().unwrap().history.undo.is_empty());
    lock(&f.rt.state).workers.clear();
    worker.join().unwrap();
}

#[test]
fn failed_phase_rolls_back_gesture_even_when_guest_returns_zero() {
    let f = Fixture::new();
    let mut s = Session::new();
    let original = s.doc().unwrap().clone();
    let worker = mock_worker(&f, true);
    lock(&f.rt.state).selected_tool = Some(f.owner.clone());
    f.rt.tool_event(&mut s, "begin", [0., 0.], Value::Null);
    assert_ne!(*s.doc().unwrap(), original);
    f.rt.tool_event(&mut s, "commit", [0., 0.], Value::Null);
    assert_eq!(*s.doc().unwrap(), original);
    assert!(s.message.is_some());
    lock(&f.rt.state).workers.clear();
    worker.join().unwrap();
}

#[derive(Default)]
struct LiveScript {
    events: Vec<(u64, figcraft_engine::live::Event)>,
    sent: Vec<figcraft_collab::message::ClientMsg>,
}
struct Link(Arc<Mutex<LiveScript>>);
impl figcraft_engine::live::LiveLink for Link {
    fn host(&mut self, _: u64, _: figcraft_engine::live::HostOptions) -> Result<figcraft_engine::live::SessionInfo, String> {
        Err("unused".into())
    }
    fn join(&mut self, _: u64, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn leave(&mut self, _: u64) {}
    fn presence(&mut self, _: u64, _: &figcraft_engine::live::Presence) {}
    fn send(&mut self, _: u64, msg: figcraft_collab::message::ClientMsg) {
        lock(&self.0).sent.push(msg);
    }
    fn poll(&mut self) -> Vec<(u64, figcraft_engine::live::Event)> {
        std::mem::take(&mut lock(&self.0).events)
    }
}

#[test]
fn nested_plugin_group_publishes_once_on_commit_and_nothing_on_rollback() {
    use figcraft_collab::message::{ClientMsg, ServerMsg};
    use figcraft_collab::{Client, Limits, Outgoing, Server};
    let doc = figcraft_doc::Document::new("Shared");
    let mut server = Server::new(doc.clone(), "", Limits::default());
    let ClientMsg::Hello(hello) = Client::hello("f", "", "Me", "") else { panic!() };
    let (_, out) = server.join(hello).unwrap();
    let welcome = out
        .into_iter()
        .find_map(|o| match o {
            Outgoing::To(_, w @ ServerMsg::Welcome { .. }) => Some(w),
            _ => None,
        })
        .unwrap();
    let mut s = Session::new();
    let script = Arc::new(Mutex::new(LiveScript::default()));
    s.live.install(Box::new(Link(script.clone())));
    let join = s.execute("collab.join", &json!({"link":"figcraft://join?host=127.0.0.1:47900&file=f&token=t"})).unwrap();
    let session = join["session"].as_u64().unwrap();
    use figcraft_engine::live::{Access, Event, Role, SessionInfo};
    lock(&script).events.push((
        session,
        Event::Welcome {
            info: SessionInfo {
                role: Role::Guest,
                me: 1,
                name: "Me".into(),
                color: "#0D99FF".into(),
                file_name: "Shared".into(),
                pages: vec![],
                link: "figcraft://join?host=127.0.0.1:47900&file=f&token=t".into(),
                url: "ws://127.0.0.1:47900/?file=f&token=t".into(),
                address: "127.0.0.1:47900".into(),
                network: false,
                access: Access::Edit,
                view: None,
            },
            peers: vec![],
        },
    ));
    lock(&script).events.push((session, figcraft_engine::live::Event::Sync(welcome)));
    s.live_poll();
    lock(&script).sent.clear();
    for commit in [false, true] {
        let before = s.doc().unwrap().clone();
        let outer = s.begin_group();
        for _ in 0..2 {
            let inner = s.begin_group();
            s.execute("shape.rectangle", &json!({"x":1,"y":2,"width":10,"height":10})).unwrap();
            s.finish_group(inner, "phase", true);
            assert!(lock(&script).sent.is_empty(), "uncommitted edit escaped");
        }
        s.finish_group(outer, "gesture", commit);
        if commit {
            assert_eq!(lock(&script).sent.len(), 1);
        } else {
            assert!(lock(&script).sent.is_empty());
            assert_eq!(*s.doc().unwrap(), before);
        }
    }
}

#[test]
fn startup_init_failures_do_not_pause_plugin() {
    let f = Fixture::new();
    let mut s = Session::empty();
    let (tx, rx) = mpsc::sync_channel::<Job>(4);
    lock(&f.rt.state).workers.insert(f.owner.worker.clone(), tx);
    let worker = std::thread::spawn(move || {
        for job in rx {
            let _ = job.done.send(Err("startup error".into()));
        }
    });
    for _ in 0..3 {
        assert!(f.rt.deliver(&mut s, &f.owner, json!({"event":"init"}), true).is_err());
    }
    assert!(!lock(&f.rt.state).failures.contains_key(&f.owner.plugin));
    assert!(f.rt.deliver(&mut s, &f.owner, json!({"event":"command"}), true).is_err());
    assert_eq!(lock(&f.rt.state).failures[&f.owner.plugin], 1);
    lock(&f.rt.state).workers.clear();
    worker.join().unwrap();
}

#[test]
fn store_generation_change_removes_disabled_contributions_and_workers() {
    let mut f = Fixture::new();
    let mut s = Session::new();
    {
        let mut st = lock(&f.rt.state);
        st.generation = Some(f.state.app_fingerprint(&f.rt.app));
        st.selected_tool = Some(f.owner.clone());
        st.surfaces.insert(
            "test/panel".into(),
            Surface {
                view: PluginSurface { key: "test/panel".into(), title: "Test".into(), tree: None, rev: 0, window: false, open: true },
                owner: f.owner.clone(),
            },
        );
    }
    let (tx, _rx) = mpsc::sync_channel(4);
    lock(&f.rt.state).workers.insert(f.owner.worker.clone(), tx);
    let setup = f.state.apps.get_mut(&f.rt.app).unwrap().get_mut(&f.owner.plugin).unwrap();
    setup.active = None;
    setup.enabled.clear();
    f.state.generation += 1;
    f.save();
    f.rt.pump(&mut s);
    assert!(f.rt.commands().is_empty());
    assert!(f.rt.surfaces().is_empty());
    assert!(!f.rt.active_tool());
    assert!(lock(&f.rt.state).workers.is_empty());
    assert_eq!(lock(&f.rt.state).generation, Some(f.state.app_fingerprint(&f.rt.app)));
}

#[test]
fn denied_single_operation_does_not_abort_event() {
    let mut f = Fixture::new();
    let mut s = Session::new();
    let before = s.ds().unwrap().doc.clone();
    f.owner.declared = vec![Permission::parse("document.read:document").unwrap()].into();
    let (replies, done) = f.dispatch(&mut s, vec![("node.create", json!({"type":"rectangle"})), ("document.info", json!({}))], true);
    assert_eq!(replies[1].as_ref().unwrap_err().code, Code::Denied);
    assert!(replies[2].is_ok());
    assert!(done.is_ok());
    assert!(Arc::ptr_eq(&before, &s.ds().unwrap().doc));
}

#[test]
fn form_patches_are_staged_atomic_and_revision_checked() {
    let f = Fixture::new();
    let mut s = Session::new();
    let k = format!("{}/panel", f.owner.plugin);
    let tree = json!({"type":"form","form":{"schema":1,"key":"settings","title":"Settings","params":[{"key":"amount","label":"Amount","kind":{"type":"range","min":0,"max":10,"default":1}}]},"values":{}});
    lock(&f.rt.state).surfaces.insert(k.clone(), Surface {
        view: PluginSurface { key:k.clone(), title:"Test".into(), tree:Some(tree.clone()), rev:1, window:false, open:true }, owner:f.owner.clone()
    });
    let inv = f.rt.invocation(&f.owner);
    let mut staged = Vec::new();
    let patch = |base, rev, ops| json!({"panel":"panel","patch":{"schema":1,"base_rev":base,"rev":rev,"ops":ops}});
    assert!(f.rt.op(&mut s, &inv, "ui.panel.set", patch(1,2,json!([{"op":"hide","key":"amount"},{"op":"set","key":"amount","value":11}])), &mut staged).is_err());
    assert!(staged.is_empty());
    f.rt.op(&mut s, &inv, "ui.panel.set", patch(1,2,json!([{"op":"hide","key":"amount"}])), &mut staged).unwrap();
    assert_eq!(lock(&f.rt.state).surfaces[&k].view.tree, Some(tree));
    f.rt.apply(&inv, staged);
    let mut staged = Vec::new();
    assert!(f.rt.op(&mut s, &inv, "ui.panel.set", patch(1,3,json!([])), &mut staged).is_err());
    f.rt.op(&mut s, &inv, "ui.panel.set", patch(2,3,json!([{"op":"show","key":"amount"},{"op":"set","key":"amount","value":5}])), &mut staged).unwrap();
    f.rt.apply(&inv, staged);
    let st = lock(&f.rt.state);
    let node:Node = serde_json::from_value(st.surfaces[&k].view.tree.clone().unwrap()).unwrap();
    let Node::Form(craft_ext_core::ui::tree::FormNode { form, values, .. }) = node else { panic!("expected form") };
    assert!(form.params[0].visible_when.is_none());
    assert_eq!(values["amount"], craft_ext_core::form::Value::Number(5.0));
    assert_eq!(st.surfaces[&k].view.rev, 3);
}

#[test]
fn selection_set_maps_to_the_engine_select_command() {
    let f = Fixture::new();
    let mut s = Session::new();
    let id = s.execute("node.create", &json!({"type":"rectangle","select":false})).unwrap()["id"].as_u64().unwrap();
    let (replies, done) = f.dispatch(&mut s, vec![("selection.set", json!({"ids":[id]}))], true);
    assert!(replies[1].is_ok(), "{:?}", replies[1]);
    assert!(done.is_ok());
    assert_eq!(s.selection(), [figcraft_doc::NodeId(id)]);
}

/// The smallest session module: the ABI exports and a `craft_event` that succeeds without calls.
const OK_MODULE: &str = r#"(module
  (memory (export "memory") 32)
  (global $top (mut i32) (i32.const 1048576))
  (func (export "craft_abi") (result i32) (i32.const 1))
  (func (export "craft_reset") (global.set $top (i32.const 1048576)))
  (func (export "craft_alloc") (param $n i32) (result i32) (local $p i32)
    (local.set $p (global.get $top))
    (global.set $top (i32.add (global.get $top) (i32.and (i32.add (local.get $n) (i32.const 7)) (i32.const -8))))
    (local.get $p))
  (func (export "craft_event") (param i32 i32) (result i32) (i32.const 0)))"#;

#[test]
fn paused_plugin_is_listed_and_resume_reinstantiates_it_with_a_clear_count() {
    let f = Fixture::new();
    let mut s = Session::new();
    let (tx, rx) = mpsc::sync_channel::<Job>(4);
    lock(&f.rt.state).workers.insert(f.owner.worker.clone(), tx);
    let failing = std::thread::spawn(move || {
        for job in rx {
            let _ = job.done.send(Err("trap: unreachable".into()));
        }
    });
    assert!(f.rt.paused().is_empty());
    assert!(f.rt.resume(&mut s, "test.plugin").is_err(), "not paused yet");
    for _ in 0..PAUSE_AFTER {
        assert!(f.rt.deliver(&mut s, &f.owner, json!({"event":"command"}), true).is_err());
    }
    let paused = f.rt.paused();
    assert_eq!(paused.len(), 1);
    assert_eq!((paused[0].id.as_str(), paused[0].name.as_str(), paused[0].reason.as_str()), ("test.plugin", "Test", "trap: unreachable"));
    assert!(f.rt.deliver(&mut s, &f.owner, json!({"event":"command"}), true).unwrap_err().contains("paused"));
    // The stored program is what resume instantiates afresh.
    let plugin = Plugin::load("test.plugin", &wat::parse_str(OK_MODULE).unwrap()).unwrap();
    lock(&f.rt.state).programs.insert(f.owner.worker.clone(), Program::Wasm(plugin));
    f.rt.resume(&mut s, "test.plugin").unwrap();
    // The failed instance's worker was replaced (its queue closed, so its thread ended).
    failing.join().unwrap();
    assert!(f.rt.paused().is_empty());
    assert!(!lock(&f.rt.state).failures.contains_key(&f.owner.plugin), "the count is cleared");
    f.rt.deliver(&mut s, &f.owner, json!({"event":"command","id":"command"}), true).unwrap();
    assert!(f.rt.resume(&mut s, "test.plugin").is_err(), "resuming twice is refused");
    // Through the session command the UI and scripts use.
    let mut s2 = Session::new();
    s2.plugins = Some(f.rt.clone());
    assert_eq!(s2.execute("plugins.paused", &json!({})).unwrap(), json!([]));
}

/// A worker that records each event and answers with `calls` (op, args) in one transaction.
fn recording_worker(f: &Fixture, calls: impl Fn(&Value) -> Vec<(String, Value)> + Send + 'static) -> (Arc<Mutex<Vec<Value>>>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::sync_channel::<Job>(8);
    lock(&f.rt.state).workers.insert(f.owner.worker.clone(), tx);
    let requests = f.rt.tx.clone();
    let waker = f.rt.waker.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let handle = std::thread::spawn(move || {
        for job in rx {
            let event = job.event.clone();
            let kinds = std::iter::once(Kind::Begin)
                .chain(calls(&job.event).into_iter().map(|(op, a)| Kind::Call(op, a)))
                .chain(std::iter::once(Kind::Finish(true)));
            let bridge = Bridge { contract: contract().unwrap(), tx: requests.clone(), context: Arc::new(Mutex::new(Some(job.context.clone()))), waker: waker.clone() };
            for kind in kinds {
                let _ = bridge.ask(&job.inv, kind);
            }
            // Record only after the transaction ends, so a test waiting on the log never races it.
            lock(&log).push(event);
            // Notifications are delivered without waiting, so their receiver may be gone.
            let _ = job.done.send(Ok(()));
        }
    });
    (seen, handle)
}

fn canvas_panel(rev_ops: usize) -> Value {
    json!({"type":"canvas","id":"pad","label":"Pad","width":200.0,"height":100.0,"ops":vec![json!({"op":"rect","min":[0.0,0.0],"max":[10.0,10.0],"fill":[1.0,0.0,0.0,1.0]}); rev_ops]})
}

#[test]
fn canvas_widget_pointer_events_reach_the_plugin_and_stale_revisions_are_dropped() {
    let f = Fixture::new();
    let mut s = Session::new();
    let key = format!("{}/panel", f.owner.plugin);
    lock(&f.rt.state).surfaces.insert(
        key.clone(),
        Surface { view: PluginSurface { key: key.clone(), title: "Panel".into(), tree: Some(canvas_panel(1)), rev: 1, window: false, open: true }, owner: f.owner.clone() },
    );
    // The guest redraws its panel on every pointer event (a new revision each time).
    let (seen, worker) = recording_worker(&f, |e| if e["event"] == "ui" { vec![("ui.panel.set".into(), json!({"panel":"panel","tree":canvas_panel(2)}))] } else { vec![] });
    f.rt.ui_event(&mut s, &key, 1, "pad", json!({"kind":"down","at":[12.5,40.0]}));
    let rev = f.rt.surfaces()[0].rev;
    assert_eq!(rev, 2, "the plugin's redraw is a new revision");
    // An event drawn against revision 1 is stale now and never reaches the plugin.
    f.rt.ui_event(&mut s, &key, 1, "pad", json!({"kind":"drag","at":[13.0,41.0]}));
    f.rt.ui_event(&mut s, &key, rev, "pad", json!({"kind":"up","at":[20.0,42.0]}));
    // A closed panel takes no events either.
    f.rt.set_open(&key, false);
    f.rt.ui_event(&mut s, &key, rev, "pad", json!({"kind":"down","at":[0.0,0.0]}));
    let seen = lock(&seen).clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0], json!({"event":"ui","surface":"panel","rev":1,"widget":"pad","value":{"kind":"down","at":[12.5,40.0]}}));
    assert_eq!(seen[1]["value"], json!({"kind":"up","at":[20.0,42.0]}));
    assert_eq!(seen[1]["rev"], 2);
    lock(&f.rt.state).workers.clear();
    worker.join().unwrap();
}

#[test]
fn canvas_overlay_shows_during_a_gesture_clears_at_its_end_and_refuses_bad_coordinates() {
    let mut f = Fixture::new();
    f.owner.kind = ContributionKind::Tool;
    lock(&f.rt.state).contributions[0].kind = ContributionKind::Tool;
    let mut s = Session::new();
    let (seen, worker) = recording_worker(&f, |e| match e["phase"].as_str() {
        Some("begin" | "update") => {
            let [x, y] = [e["at"][0].as_f64().unwrap(), e["at"][1].as_f64().unwrap()];
            vec![("canvas.overlay.set".into(), json!({"ops":[{"op":"line","from":[0.0,0.0],"to":[x,y],"width":1.0,"rgba":[1.0,0.0,1.0,1.0]}]}))]
        }
        _ => vec![],
    });
    lock(&f.rt.state).selected_tool = Some(f.owner.clone());
    f.rt.tool_event(&mut s, "begin", [5.0, 6.0], Value::Null);
    f.rt.tool_event(&mut s, "update", [50.0, 60.0], Value::Null);
    let overlays = f.rt.overlays();
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0][0]["to"], json!([50.0, 60.0]), "the latest draw list replaces the previous one");
    f.rt.tool_event(&mut s, "commit", [50.0, 60.0], Value::Null);
    assert!(f.rt.overlays().is_empty(), "cleared at gesture end");
    assert_eq!(lock(&seen).iter().map(|e| e["phase"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>(), ["begin", "update", "commit"]);
    // Out-of-range coordinates are refused before anything is staged.
    let (replies, _) = f.dispatch(&mut s, vec![("canvas.overlay.set", json!({"ops":[{"op":"circle","center":[1e12,0.0],"radius":1.0,"fill":[1.0,1.0,1.0,1.0]}]}))], true);
    assert!(replies[1].is_err());
    assert!(f.rt.overlays().is_empty());
    lock(&f.rt.state).workers.clear();
    worker.join().unwrap();
}

#[test]
fn expanded_contract_uses_validated_engine_commands_and_bounded_arguments() {
    let f = Fixture::new();
    let mut s = Session::new();
    let (reply, done) = f.dispatch(&mut s, vec![
        ("page.add", json!({"name":"Plugin page"})),
        ("style.create", json!({"type":"paint","name":"Accent","value":[{"type":"solid","color":"#336699"}]})),
    ], true);
    assert!(reply[1].is_ok(), "{reply:?}");
    assert!(done.is_ok());
    assert_eq!(s.ds().unwrap().history.undo.len(), 1);
    let inv = f.rt.invocation(&f.owner);
    for op in ["image.replace", "image.place"] {
        assert!(f.rt.op(&mut s, &inv, op, json!({"path":"/tmp/forbidden"}), &mut vec![]).is_err());
    }
    assert!(validate_args(&json!({"ids":vec![1;4097]}),0).is_err());
    assert!(validate_args(&json!({"text":"x".repeat(256*1024)}),0).is_err());
    let contract = crate::contract::contract().unwrap();
    for op in crate::contract::EDIT_COMMANDS.iter().chain(crate::contract::QUERY_COMMANDS) {
        assert!(figcraft_engine::find_command(op).is_some(), "stale engine operation {op}");
        assert!(contract.ops.iter().any(|s| s.id == *op));
    }
}

#[test]
fn ui_v2_and_shared_overlay_validation_reach_the_adapter() {
    let f = Fixture::new();
    let mut s = Session::new();
    let inv = f.rt.invocation(&f.owner);
    let mut changes = Vec::new();
    assert!(f.rt.op(&mut s, &inv, "canvas.overlay.set", json!({"ops":[{"op":"line","from":[0,0],"to":[10,20],"width":1,"rgba":[1,0,0,1]}]}), &mut changes).is_ok());
    assert!(f.rt.op(&mut s, &inv, "canvas.overlay.set", json!({"ops":[{"op":"circle","center":[0,0],"radius":-1,"fill":[1,0,0,1]}]}), &mut changes).is_err());
    assert!(Node::parse(&json!({"type":"scroll","children":[{"type":"label","text":"UI 1.1"}]})).is_ok());
}

#[test]
fn export_returns_svg_without_files_and_new_operations_obey_revocation() {
    let mut f = Fixture::new();
    let mut s = Session::new();
    let id = s.execute("node.create", &json!({"type":"rectangle","width":20,"height":20})).unwrap()["id"].clone();
    let (reply, done) = f.dispatch(&mut s, vec![("export.svg", json!({"ids":[id]}))], true);
    assert!(done.is_ok());
    assert!(reply[1].as_ref().unwrap()["svg"].as_str().unwrap().contains("<svg"));
    let before = s.doc().unwrap().pages.len();
    let setup = f.state.apps.get_mut(&f.rt.app).unwrap().get_mut(&f.owner.plugin).unwrap();
    setup.grants.clear(); setup.grant_generation += 1; f.save();
    let (reply, _) = f.dispatch(&mut s, vec![("page.add",json!({"name":"Denied"}))], true);
    assert!(reply[1].is_err());
    assert_eq!(s.doc().unwrap().pages.len(), before);
}

#[test]
fn document_changed_notifications_cannot_create_an_edit_feedback_loop() {
    let f = Fixture::new(); let mut s = Session::new();
    let before = s.doc().unwrap().nodes().count();
    let (_, worker) = recording_worker(&f, |_| vec![("node.create".into(), json!({"type":"rectangle"}))]);
    f.rt.deliver(&mut s, &f.owner, json!({"event":"app","name":"document.changed"}), true).unwrap();
    assert_eq!(s.doc().unwrap().nodes().count(), before);
    assert!(s.ds().unwrap().history.undo.is_empty());
    lock(&f.rt.state).workers.clear(); worker.join().unwrap();
}

#[test]
fn live_session_layer_ids_pass_argument_bounds() {
    // In a live session ids are `client << 32 | counter`, far past the 1e8 coordinate bound.
    let f = Fixture::new();
    let mut s = Session::new();
    s.doc_mut().unwrap().set_client_id(7);
    let id = s.execute("node.create", &json!({"type":"rectangle","width":20,"height":20})).unwrap()["id"].as_u64().unwrap();
    assert!(id > 1 << 32, "{id}");
    let (reply, done) = f.dispatch(&mut s, vec![
        ("node.update", json!({"id":id,"props":{"name":"Live"}})),
        ("selection.set", json!({"ids":[id]})),
        ("object.group", json!({"ids":[id]})),
    ], true);
    assert!(reply[1..4].iter().all(Result::is_ok), "{reply:?}");
    assert!(done.is_ok());
    // Coordinates keep their bound.
    assert!(validate_args(&json!({"x":-1e9}), 0).is_err());
    assert!(validate_args(&json!({"x":1.5e8}), 0).is_err());
    for key in ["width", "height", "zoom", "x", "time"] {
        assert!(validate_args(&json!({key:u64::MAX}), 0).is_err(), "{key}");
    }
    assert!(validate_args(&json!({"props":{"width":u64::MAX}}), 0).is_err());
    assert!(validate_args(&json!({"ids":[id],"parent":id}), 0).is_ok());
}

#[test]
fn comments_cannot_be_signed_as_someone_else() {
    let f = Fixture::new();
    let mut s = Session::new();
    let (reply, _) = f.dispatch(&mut s, vec![("comment.add", json!({"x":0,"y":0,"text":"hi","author":"Sam"}))], true);
    assert!(reply[1].as_ref().unwrap_err().to_string().contains("author"), "{reply:?}");
    let (reply, done) = f.dispatch(&mut s, vec![("comment.add", json!({"x":0,"y":0,"text":"hi"}))], true);
    assert!(reply[1].is_ok() && done.is_ok(), "{reply:?}");
    let c = reply[1].as_ref().unwrap()["id"].clone();
    let (reply, _) = f.dispatch(&mut s, vec![("comment.reply", json!({"id":c,"text":"me too","author":"Sam"}))], true);
    assert!(reply[1].is_err());
}

/// Serves worker requests until `done` holds (or two seconds pass).
fn serve_until(f: &Fixture, s: &mut Session, done: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(2);
    while !done() && Instant::now() < until {
        f.rt.serve(s);
        std::thread::sleep(Duration::from_millis(5));
    }
    f.rt.serve(s);
}

#[test]
fn document_changed_follows_content_not_selection_view_or_rollback_and_wakes_the_ui() {
    let mut f = Fixture::new();
    let events: Permission = serde_json::from_value(json!("app.events")).unwrap();
    f.state.apps.get_mut(&f.rt.app).unwrap().get_mut(&f.owner.plugin).unwrap().grants.insert(events.clone());
    f.save();
    let declared: Vec<Permission> = f.owner.declared.iter().cloned().chain([events]).collect();
    lock(&f.rt.state).contributions[0].declared = declared.into();
    lock(&f.rt.state).generation = Store::at(&f.rt.root).state().map(|s| s.app_fingerprint(&f.rt.app)).ok();
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woken.clone();
    f.rt.set_waker(Arc::new(move || { w.fetch_add(1, Ordering::Relaxed); }));
    // The handler selects and scrolls (both allowed in a read-only notification).
    let (seen, worker) = recording_worker(&f, |e| if e["name"] == "document.changed" {
        vec![("selection.set".into(), json!({"ids":[]})), ("viewport.set".into(), json!({"zoom":2.0}))]
    } else { vec![] });
    let names = |seen: &Arc<Mutex<Vec<Value>>>| lock(seen).iter().filter_map(|e| e["name"].as_str().map(str::to_string)).collect::<Vec<_>>();
    let mut s = Session::new();
    // node.create selects the new node by default; keep it unselected so selecting it is a change.
    let id = s.execute("node.create", &json!({"type":"rectangle","width":20,"height":20,"select":false})).unwrap()["id"].clone();
    f.rt.pump(&mut s);
    // The first pump sends document.opened, selection.changed and page.changed, in that order.
    serve_until(&f, &mut s, || woken.load(Ordering::Relaxed) > 0 && names(&seen).iter().any(|n| n == "page.changed") && lock(&f.rt.deferred).is_empty());
    assert!(woken.load(Ordering::Relaxed) > 0, "a queued worker request wakes the UI");
    assert!(!f.rt.wants_pump(&s), "nothing left to deliver");
    lock(&seen).clear();
    // Selection and view changes are not content changes.
    s.execute("select.set", &json!({"ids":[id]})).unwrap();
    s.execute("view.set", &json!({"zoom":3.0})).unwrap();
    assert!(f.rt.wants_pump(&s), "the selection changed since the last pump");
    f.rt.pump(&mut s);
    serve_until(&f, &mut s, || !names(&seen).is_empty());
    assert_eq!(names(&seen), ["selection.changed"]);
    // A rolled-back event leaves the content as it was: no notification.
    lock(&seen).clear();
    let _ = f.dispatch(&mut s, vec![("node.create", json!({"type":"rectangle"}))], false);
    f.rt.pump(&mut s);
    serve_until(&f, &mut s, || false);
    assert!(names(&seen).is_empty(), "{:?}", names(&seen));
    // A content change notifies once; the handler's selection and scroll do not re-trigger it.
    s.execute("select.set", &json!({"ids":[]})).unwrap();
    f.rt.pump(&mut s);
    serve_until(&f, &mut s, || false);
    lock(&seen).clear();
    s.execute("node.set", &json!({"ids":[id],"props":{"name":"Renamed"}})).unwrap();
    for _ in 0..4 { f.rt.pump(&mut s); serve_until(&f, &mut s, || false); }
    assert_eq!(names(&seen), ["document.changed"]);
    lock(&f.rt.state).workers.clear();
    worker.join().unwrap();
}
