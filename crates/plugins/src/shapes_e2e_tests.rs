//! Real-package acceptance test: installs the packed `figcraft-shapes` example into a disposable
//! store, runs its wasm through this adapter and walks the beta flow end to end. Ignored by default;
//! run with `FIGCRAFT_SHAPES_PACKAGE=/path/figcraft-shapes.craftplugin cargo test -p figcraft-plugins
//! --lib shapes_e2e -- --ignored --nocapture` (see `docs/plugin-testing.md`).
use super::*;
use figcraft_engine::Session;

const ID: &str = "io.github.arnav-hiwarkar.figcraft-shapes";

fn cli(m: &Manager, args: &[&str]) -> Result<String, String> {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = craft_ext_cli::run_plugin(m, &args, &mut out, &mut err);
    let text = String::from_utf8_lossy(&out).into_owned() + &String::from_utf8_lossy(&err);
    if code == 0 { Ok(text) } else { Err(text) }
}

fn named(s: &Session, prefix: &str) -> Vec<(f64, f64)> {
    s.doc().map(|d| d.nodes().filter(|n| n.name.starts_with(prefix)).map(|n| (n.width, n.height)).collect()).unwrap_or_default()
}

fn undo_len(s: &Session) -> usize {
    s.ds().map(|d| d.history.undo.len()).unwrap_or(0)
}

fn panel(rt: &Runtime) -> PluginSurface {
    rt.surfaces().into_iter().find(|s| s.key == format!("{ID}/shapes")).expect("shapes panel registered")
}

fn panel_texts(rt: &Runtime) -> String {
    rt.surfaces().into_iter().find(|s| s.key == format!("{ID}/shapes")).map(|s| serde_json::to_string(&s.tree).unwrap_or_default()).unwrap_or_default()
}

#[test]
#[ignore = "needs FIGCRAFT_SHAPES_PACKAGE (a packed figcraft-shapes example)"]
fn shapes_example_end_to_end() {
    let Some(package) = std::env::var_os("FIGCRAFT_SHAPES_PACKAGE") else {
        eprintln!("FIGCRAFT_SHAPES_PACKAGE is not set; skipping");
        return;
    };
    let root = std::env::temp_dir().join(format!("figcraft-shapes-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let rt = Runtime::new(root.clone()).unwrap();
    let m = rt.manager();

    // Install from file, then the explicit setup: install alone never enables or grants.
    println!("install-file: {}", cli(&m, &["install-file", package.to_str().unwrap()]).unwrap());
    println!("setup: {}", cli(&m, &["setup", ID, "--all", "--grant", "document.edit:document"]).unwrap());

    // 1. Init renders the panel: a column whose first child is a Node::Form.
    let mut s = Session::new();
    rt.load(&mut s).unwrap();
    s.plugins = Some(rt.clone());
    let sf = panel(&rt);
    let tree: Node = serde_json::from_value(sf.tree.clone().expect("init drew the panel")).unwrap();
    let Node::Column { children } = &tree else { panic!("panel root is a column: {tree:?}") };
    let Some(Node::Form { form, values }) = children.first() else { panic!("first child is a form: {children:?}") };
    assert_eq!(form.key, "shapes");
    assert!(form.validate_values(values).is_ok());
    let keys: Vec<&str> = form.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["shape", "size", "count", "color"]);
    let commands: Vec<String> = rt.commands().into_iter().map(|c| c.id).collect();
    assert!(commands.contains(&format!("plugin.{ID}.create-shapes")) && commands.contains(&format!("plugin.{ID}.draw-rect")), "{commands:?}");

    // 2. Form edits reach the guest (count 4, size 50), then the command creates the row as ONE undo step.
    let key = sf.key.clone();
    assert!(!sf.open, "panels start closed until the person opens them (Plugins ▸ Shapes)");
    rt.set_open(&key, true);
    rt.ui_event(&mut s, &key, sf.rev, "count", json!(4.0));
    let sf = panel(&rt);
    rt.ui_event(&mut s, &key, sf.rev, "size", json!(50.0));
    let before = undo_len(&s);
    s.execute(&format!("plugin.{ID}.create-shapes"), &json!({})).unwrap();
    let shapes = named(&s, "Shape ");
    assert_eq!(shapes.len(), 4, "{shapes:?}");
    assert!(shapes.iter().all(|&(w, h)| w == 50.0 && h == 50.0), "{shapes:?}");
    assert_eq!(undo_len(&s), before + 1, "one undo step");
    assert_eq!(s.ds().unwrap().history.undo.last().unwrap().label, "Create shapes · Shapes");
    assert_eq!(s.doc().unwrap().plugins.len(), 1, "the edit recorded the plugin reference");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(named(&s, "Shape ").is_empty(), "undo removes the whole row");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(named(&s, "Shape ").len(), 4);
    // The panel button runs the same code from the panel contribution.
    let sf = panel(&rt);
    rt.ui_event(&mut s, &key, sf.rev, "create-shapes", Value::Null);
    assert_eq!(named(&s, "Shape ").len(), 8);
    assert_eq!(undo_len(&s), before + 2);

    // 3. Revoke the edit permission mid-session. Before the next frame reconciles, the running guest
    // gets a recoverable PermissionNeeded and explains it in its panel; nothing changes.
    println!("revoke: {}", cli(&m, &["revoke", ID, "document.edit:document"]).unwrap());
    let (count, undo) = (named(&s, "Shape ").len(), undo_len(&s));
    s.execute(&format!("plugin.{ID}.create-shapes"), &json!({})).unwrap();
    assert!(panel_texts(&rt).contains("needs \\\"document.edit:document\\\""), "{}", panel_texts(&rt));
    assert_eq!((named(&s, "Shape ").len(), undo_len(&s)), (count, undo), "denied: no shapes, no undo step");
    // The next frame reconciles: a contribution missing a required grant is unloaded.
    s.pump_plugins();
    assert!(rt.commands().is_empty() && rt.surfaces().is_empty());
    assert!(s.execute(&format!("plugin.{ID}.create-shapes"), &json!({})).is_err());
    assert_eq!((named(&s, "Shape ").len(), undo_len(&s)), (count, undo));
    // Grant again: the same session recovers without a restart.
    println!("grant: {}", cli(&m, &["setup", ID, "--all", "--grant", "document.edit:document"]).unwrap());
    s.pump_plugins();
    s.execute(&format!("plugin.{ID}.create-shapes"), &json!({})).unwrap();
    assert_eq!(named(&s, "Shape ").len(), count + 3, "a fresh worker starts from the form defaults (3)");
    assert_eq!(undo_len(&s), undo + 1);
    // Revoked during a tool gesture: the next frame cancels it and rolls back every phase.
    s.execute(&format!("plugin.{ID}.draw-rect"), &json!({})).unwrap();
    let undo = undo_len(&s);
    rt.tool_event(&mut s, "begin", [5.0, 5.0], json!({}));
    rt.tool_event(&mut s, "update", [45.0, 25.0], json!({}));
    assert_eq!(named(&s, "Drawn rectangle"), [(40.0, 20.0)]);
    cli(&m, &["revoke", ID, "document.edit:document"]).unwrap();
    s.pump_plugins();
    assert!(!rt.busy() && rt.overlays().is_empty());
    assert!(named(&s, "Drawn rectangle").is_empty(), "the revoked gesture rolled back");
    assert!(s.message.take().is_some_and(|m| m.contains("revoked")), "the person is told why");
    assert_eq!(undo_len(&s), undo);
    cli(&m, &["setup", ID, "--all", "--grant", "document.edit:document"]).unwrap();
    s.pump_plugins();
    assert_eq!(rt.commands().len(), 2);

    // 4. Tool gesture: begin/update/commit is one undo step; the overlay shows the guide meanwhile.
    s.execute(&format!("plugin.{ID}.draw-rect"), &json!({})).unwrap();
    assert!(rt.active_tool());
    let undo = undo_len(&s);
    rt.tool_event(&mut s, "begin", [10.0, 10.0], json!({}));
    rt.tool_event(&mut s, "update", [60.0, 40.0], json!({}));
    assert!(rt.busy());
    let overlay = rt.overlays();
    assert_eq!(overlay.len(), 1);
    assert_eq!(overlay[0].as_array().map(Vec::len), Some(4), "{overlay:?}");
    assert_eq!(named(&s, "Drawn rectangle"), [(50.0, 30.0)]);
    rt.tool_event(&mut s, "commit", [110.0, 90.0], json!({}));
    assert_eq!(s.message.take(), None);
    assert!(!rt.busy());
    assert!(rt.overlays().is_empty(), "overlay clears at gesture end");
    assert_eq!(named(&s, "Drawn rectangle"), [(100.0, 80.0)]);
    assert_eq!(undo_len(&s), undo + 1, "the whole gesture is one undo step");
    // Cancel (Escape) rolls the gesture back: no new layer, no undo step, overlay gone.
    rt.tool_event(&mut s, "begin", [200.0, 200.0], json!({}));
    rt.tool_event(&mut s, "update", [260.0, 230.0], json!({}));
    assert_eq!(named(&s, "Drawn rectangle").len(), 2);
    rt.tool_event(&mut s, "cancel", [0.0, 0.0], Value::Null);
    assert_eq!(named(&s, "Drawn rectangle"), [(100.0, 80.0)]);
    assert_eq!(undo_len(&s), undo + 1);
    assert!(rt.overlays().is_empty() && !rt.busy() && !rt.active_tool());

    // 5. Save with the plugin list, reopen, and check the statuses `plugin project` prints.
    let file = root.join("shapes.figcraft");
    let total = named(&s, "Shape ").len();
    std::fs::write(&file, figcraft_engine::cmd::file::file_bytes(s.ds().unwrap()).unwrap()).unwrap();
    let mut reopened = Session::empty();
    reopened.execute("file.open", &json!({"path": file})).unwrap();
    let refs = reopened.doc().unwrap().plugins.clone();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].id.as_str(), ID);
    assert!(m.check_project(&refs).unwrap().iter().all(|(_, st)| st.ready()), "installed and set up: Ready");
    println!("project (installed): {:?}", m.check_project(&refs).unwrap());

    // 6. Uninstall: reopening keeps every layer (plugin layers are ordinary layers) and the prompt says missing.
    println!("remove: {}", cli(&m, &["remove", ID]).unwrap());
    let mut after = Session::empty();
    after.execute("file.open", &json!({"path": file})).unwrap();
    assert_eq!(named(&after, "Shape ").len(), total);
    assert_eq!(named(&after, "Drawn rectangle"), [(100.0, 80.0)]);
    let statuses = m.check_project(&after.doc().unwrap().plugins).unwrap();
    println!("project (uninstalled): {statuses:?}");
    assert!(statuses.iter().all(|(_, st)| !st.ready()));
    let fresh = Runtime::new(root.clone()).unwrap();
    after.plugins = Some(fresh.clone());
    after.pump_plugins();
    let prompt = fresh.take_project_prompt().expect("opening shows the project prompt");
    assert!(prompt.iter().any(|(r, st)| r.id.as_str() == ID && !st.ready()), "{prompt:?}");
    let _ = std::fs::remove_dir_all(&root);
}
