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
            Dispatch { uid: s.ds().map_or(0, |d| d.uid), hash: Some("digest".into()), surface: None, completion: Arc::new(Mutex::new(None)) };
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
        context: Dispatch { uid: s.ds().unwrap().uid, hash: Some("digest".into()), surface: None, completion: Arc::new(Mutex::new(None)) },
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
        st.generation = Some(f.state.generation);
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
    assert_eq!(lock(&f.rt.state).generation, Some(1));
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
    let Node::Form { form, values } = node else { panic!("expected form") };
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
