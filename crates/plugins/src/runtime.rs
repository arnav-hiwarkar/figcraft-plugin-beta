//! Workers own guest instances; only pump executes document operations.
use std::{collections::{BTreeMap, VecDeque}, sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc}, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}, path::PathBuf};
use craft_ext_core::{id::{AppId, ExtensionId}, version::Version, manifest::ContributionKind, permission::Permission, project::{PluginRef, Source, Status}, ui::Node};
use craft_ext_host::{api::{AppApi, Grants, Invocation, OpError, Code, authorize}, wasm::{Plugin, Session as Guest}};
use craft_ext_store::{Manager, Store, view::GrantView};
use figcraft_engine::{Session, EngineError, ext::{PluginRuntime, PluginCommand, PluginSurface, GroupMark}};
use serde_json::{Value, json};
use crate::{contract, grants::LiveGrants};
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(PoisonError::into_inner) }
fn err(e: impl ToString) -> OpError { OpError::invalid(e.to_string()) }
fn key(i: &Invocation) -> usize { Arc::as_ptr(&i.cancel) as usize }
#[derive(Clone)]
struct Dispatch { uid: u64, hash: Option<String>, surface: Option<(String,u64)>, completion: Arc<Mutex<Option<Result<(),String>>>> }
enum Kind { Begin, Call(String,Value), Finish(bool) }
struct Request { inv: Invocation, context: Dispatch, kind: Kind, reply: mpsc::Sender<Result<Value,OpError>> }
struct Bridge { contract: craft_ext_host::api::AppContract, tx: mpsc::SyncSender<Request>, context: Arc<Mutex<Option<Dispatch>>> }
impl Bridge {
    fn ask(&self, inv: &Invocation, kind: Kind) -> Result<Value,OpError> {
        let (tx,rx)=mpsc::channel();
        let context=lock(&self.context).clone().ok_or_else(|| err("no event context"))?;
        self.tx.try_send(Request { inv: inv.clone(), context, kind, reply: tx }).map_err(|_| err("document queue full"))?;
        rx.recv_timeout(Duration::from_secs(7)).unwrap_or_else(|_| { inv.cancel.store(true,Ordering::Relaxed); Err(OpError::new(Code::Cancelled,"document request timed out")) })
    }
}
impl AppApi for Bridge {
    fn contract(&self)->&craft_ext_host::api::AppContract { &self.contract }
    fn authorizes_on_owner_thread(&self)->bool { true }
    fn begin(&self,i:&Invocation)->Result<(),OpError> { self.ask(i,Kind::Begin).map(|_|()) }
    fn call(&self,i:&Invocation,op:&str,a:Value)->Result<Value,OpError> { self.ask(i,Kind::Call(op.into(),a)) }
    fn finish(&self,i:&Invocation,c:bool) { if self.ask(i,Kind::Finish(c)).is_err() { i.cancel.store(true,Ordering::Relaxed); } }
}
struct Job { inv: Invocation, context: Dispatch, event: Value, done: mpsc::Sender<Result<(),String>> }
#[derive(Clone)]
struct Contribution { plugin: ExtensionId, version: Version, name: String, id: String, label: String, kind: ContributionKind, declared: Arc<[Permission]>, worker: String, reference: PluginRef }
struct Surface { view: PluginSurface, owner: Contribution }
struct Gesture { mark: GroupMark, uid: u64, owner: Contribution, generation: u64 }
#[derive(Default)]
struct State { form_metadata: BTreeMap<String,Value>, form_flags: BTreeMap<String,Value>, contributions: Vec<Contribution>, workers: BTreeMap<String,mpsc::SyncSender<Job>>, surfaces: BTreeMap<String,Surface>, overlays: BTreeMap<String,Value>, selected_tool: Option<Contribution>, gesture: Option<Gesture>, generation: Option<u64>, seen: Option<(u64,Value,u64)>, prompt: Option<Vec<(PluginRef,Status)>>, failures: BTreeMap<ExtensionId,u32> }
pub struct Runtime { root: PathBuf, app: AppId, grants: Arc<LiveGrants>, contract: craft_ext_host::api::AppContract, state: Mutex<State>, rx: Mutex<mpsc::Receiver<Request>>, tx: mpsc::SyncSender<Request>, deferred: Mutex<VecDeque<Request>>, interactive: AtomicBool }
pub fn manager() -> Result<Manager,String> {
    let root=craft_ext_store::location::root_for(craft_ext_store::location::Os::current(),|k|std::env::var(k).ok()).map_err(|e|e.to_string())?;
    let c=contract().map_err(|e|e.to_string())?;
    Ok(Manager::new(Store::at(root),Some(c.app),c.provided,craft_ext_host::native::platform()))
}
static RUNTIME: std::sync::OnceLock<Arc<Runtime>> = std::sync::OnceLock::new();
/// The runtime [`activate`] started in this process, if any.
pub fn runtime() -> Option<Arc<Runtime>> { RUNTIME.get().cloned() }
/// Starts plugins for the app's session: loads enabled contributions (not in safe mode) and attaches
/// the runtime. Call once, after startup files are open.
pub fn activate(s:&mut Session) -> Result<Arc<Runtime>,String> {
    if let Some(rt)=runtime() { s.plugins=Some(rt.clone()); return Ok(rt); }
    let m=manager()?;
    let rt=Runtime::new(m.store.root().to_path_buf())?;
    if !crate::safe_mode() { rt.load(s)?; }
    s.plugins=Some(rt.clone());
    let _=RUNTIME.set(rt.clone());
    Ok(rt)
}
impl Runtime {
    pub fn new(root:PathBuf)->Result<Arc<Self>,String> {
        let c=contract().map_err(|e|e.to_string())?;
        let grants=LiveGrants { root:root.clone(), app:c.app.clone() };
        let (tx,rx)=mpsc::sync_channel(64);
        Ok(Arc::new(Self { root, app:c.app.clone(), grants:Arc::new(grants), contract:c, state:Mutex::new(State::default()), rx:Mutex::new(rx), tx, deferred:Mutex::new(VecDeque::new()), interactive:AtomicBool::new(false) }))
    }
    pub fn manager(&self)->Manager { Manager::new(Store::at(&self.root),Some(self.app.clone()),self.contract.provided.clone(),craft_ext_host::native::platform()) }
    pub fn take_project_prompt(&self)->Option<Vec<(PluginRef,Status)>> { lock(&self.state).prompt.take() }
    fn load(&self,s:&mut Session)->Result<(),String> {
        let store=Store::at(&self.root); let saved=store.state().map_err(|e|e.to_string())?;
        let releases=GrantView::open(&self.root,self.app.clone()).and_then(|v|v.enabled_contributions(&self.app)).map_err(|e|e.to_string())?;
        // Never mark a mixed registry snapshot current after a concurrent management write.
        if store.state().map_err(|e|e.to_string())?.generation!=saved.generation {return Err("plugin store changed during reconciliation".into());}
        for (id,version,dir,manifest,enabled) in releases {
            let hash=saved.packages.get(&id).and_then(|p|p.releases.get(&version)).map(|r|r.sha256.clone());
            let source=if saved.packages.get(&id).is_some_and(|p|matches!(p.origin,craft_ext_store::state::Origin::Catalogue{..})) { Source::Catalogue } else { Source::Local };
            for c in manifest.contributions_for(&self.app).filter(|c|enabled.contains(&c.id) && c.contract.check(&self.contract.provided).is_none()) {
                if !matches!(c.kind,ContributionKind::Command|ContributionKind::Panel|ContributionKind::Tool|ContributionKind::EventHook) { continue; }
                let module=if manifest.native.is_some() { "native" } else { c.module.as_deref().ok_or("session has no module")? };
                let worker=format!("{id}@{version}/{module}");
                if !lock(&self.state).workers.contains_key(&worker) {
                    let code=if let Some(n)=&manifest.native {
                        let exe=dir.join(n.executables.get(&craft_ext_host::native::platform()).ok_or("no executable for this platform")?);
                        let work=store.private_dir(&self.app,&id); std::fs::create_dir_all(&work).map_err(|e|e.to_string())?;
                        Program::Native(id.clone(),version,exe,work)
                    } else {
                        use std::io::Read;
                        let mut bytes=Vec::new(); std::fs::File::open(dir.join(module)).map_err(|e|e.to_string())?.take(craft_ext_host::wasm::MAX_MODULE_BYTES as u64+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
                        Program::Wasm(Plugin::load(id.as_str(),&bytes).map_err(|e|e.to_string())?)
                    };
                    let tx=self.spawn(code)?; lock(&self.state).workers.insert(worker.clone(),tx);
                }
                let owner=Contribution { plugin:id.clone(),version,name:manifest.name.clone(),id:c.id.clone(),label:c.label.clone(),kind:c.kind.clone(),declared:c.permissions.iter().chain(&c.optional_permissions).cloned().collect::<Vec<_>>().into(),worker,reference:PluginRef { id:id.clone(),version,sha256:hash.clone(),source,name:manifest.name.clone(),contributions:enabled.iter().cloned().collect(),state_versions:Some(manifest.contributions_for(&self.app).filter(|c|enabled.contains(&c.id)).map(|c|(c.id.clone(),c.state_version)).collect()) } };
                let mut st=lock(&self.state);
                if c.kind==ContributionKind::Panel { st.surfaces.insert(format!("{id}/{}",c.id),Surface { view:PluginSurface { key:format!("{id}/{}",c.id),title:c.label.clone(),tree:None,rev:0,window:false,open:false },owner:owner.clone() }); }
                st.contributions.push(owner);
            }
        }
        lock(&self.state).generation=Some(saved.generation);
        let cs=lock(&self.state).contributions.clone();
        for c in cs { if let Err(e)=self.deliver(s,&c,json!({"event":"init"}),true) { log::warn!("plugin {} init: {e}",c.plugin); } }
        Ok(())
    }
    fn spawn(&self,program:Program)->Result<mpsc::SyncSender<Job>,String> {
        let (tx,rx)=mpsc::sync_channel::<Job>(64); let context=Arc::new(Mutex::new(None));
        let api=Arc::new(Bridge { contract:contract().map_err(|e|e.to_string())?,tx:self.tx.clone(),context:context.clone() });
        let grants=self.grants.clone();
        std::thread::Builder::new().name("figcraft-plugin".into()).spawn(move || {
            let log:craft_ext_host::wasm::LogFn=Arc::new(|id,_,text|log::info!("plugin {id}: {}",text.chars().take(1000).collect::<String>()));
            enum Runner { Wasm(Guest), Native(craft_ext_host::native::Helper) }
            let mut runner=match program { Program::Wasm(p)=>Guest::new(&p,api,grants,log).map(Runner::Wasm),Program::Native(id,v,exe,work)=>craft_ext_host::native::Helper::start(&id,v,&exe,&work,api,grants,log).map(Runner::Native) };
            loop {
                if let Ok(Runner::Native(h))=&mut runner { if !h.supervise() { break; } }
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(job)=> { *lock(&context)=Some(job.context); let result=match &mut runner { Ok(Runner::Wasm(g))=>g.dispatch(job.inv,&job.event).map_err(|e|e.to_string()),Ok(Runner::Native(h))=>h.dispatch(job.inv,&job.event).map_err(|e|e.to_string()),Err(e)=>Err(e.to_string()) }; let _=job.done.send(result); *lock(&context)=None; }
                    Err(mpsc::RecvTimeoutError::Timeout)=>continue,
                    Err(_)=>break,
                }
            }
        }).map_err(|e|e.to_string())?;
        Ok(tx)
    }
    fn invocation(&self,c:&Contribution)->Invocation { Invocation { app:self.app.clone(), plugin:c.plugin.clone(),version:c.version,contribution:c.id.clone(),grant_generation:self.grants.generation(&c.plugin),cancel:Arc::new(AtomicBool::new(false)),deadline:Some(Instant::now()+Duration::from_secs(8)),label:format!("{} · {}",c.label,c.name),declared:c.declared.clone() } }
    fn deliver(&self,s:&mut Session,c:&Contribution,event:Value,wait:bool)->Result<(),String> {
        // UID zero is a document-independent init context; document operations report no document.
        let uid=s.ds().map_or(0,|d|d.uid); let inv=self.invocation(c);
        let init=event["event"]=="init";
        let completion=Arc::new(Mutex::new(None));
        if lock(&self.state).failures.get(&c.plugin).copied().unwrap_or(0)>=3 { return Err("plugin paused after three failures".into()); }
        let surface=event["surface"].as_str().zip(event["rev"].as_u64()).map(|(k,r)|(format!("{}/{k}",c.plugin),r));
        let context=Dispatch { uid,hash:c.reference.sha256.clone(),surface,completion:completion.clone() };
        let (done,rx)=mpsc::channel();
        lock(&self.state).workers.get(&c.worker).ok_or("worker stopped")?.try_send(Job { inv:inv.clone(),context,event,done }).map_err(|_|"worker queue full")?;
        if !wait { return Ok(()); }
        let deadline=Instant::now()+Duration::from_secs(9);
        loop {
            self.serve(s);
            match rx.try_recv() { Ok(r)=> { let r=r.and_then(|_|lock(&completion).take().unwrap_or_else(||Err("plugin transaction did not complete".into()))); if r.is_err() && !init { *lock(&self.state).failures.entry(c.plugin.clone()).or_default()+=1; } return r; },Err(mpsc::TryRecvError::Disconnected)=>return Err("worker stopped".into()),_=>{} }
            if Instant::now()>deadline { inv.cancel.store(true,Ordering::Relaxed); return Err("plugin event timed out".into()); }
            // A bounded receive avoids spinning while wasm executes between calls.
            let request=lock(&self.rx).recv_timeout(Duration::from_millis(2));
            if let Ok(r)=request { lock(&self.deferred).push_back(r); }
        }
    }
    fn validate(&self,s:&Session,r:&Request)->Result<(),OpError> {
        if crate::safe_mode() || s.ds().map_or(0,|d|d.uid)!=r.context.uid { return Err(OpError::new(Code::Stale,"document changed")); }
        let state=Store::at(&self.root).state().map_err(err)?;
        let setup=state.apps.get(&self.app).and_then(|a|a.get(&r.inv.plugin)).ok_or_else(||OpError::denied("plugin disabled"))?;
        if setup.active!=Some(r.inv.version) || !setup.enabled.contains(&r.inv.contribution) || r.inv.cancelled() || self.grants.stopped(&r.inv.plugin,r.inv.version) || setup.grant_generation!=r.inv.grant_generation { return Err(OpError::denied("plugin or contribution revoked")); }
        let hash=state.packages.get(&r.inv.plugin).and_then(|p|p.releases.get(&r.inv.version)).map(|r|r.sha256.clone());
        if hash!=r.context.hash { return Err(OpError::new(Code::Stale,"plugin release changed")); }
        if let Some((k,rev))=&r.context.surface { if !lock(&self.state).surfaces.get(k).is_some_and(|s|s.view.open && s.view.rev==*rev) { return Err(OpError::new(Code::Stale,"panel changed")); } }
        Ok(())
    }
    fn serve(&self,s:&mut Session) {
        let first=lock(&self.deferred).pop_front().or_else(||lock(&self.rx).try_recv().ok());
        let Some(begin)=first else { return };
        if !matches!(begin.kind,Kind::Begin) { let _=begin.reply.send(Err(err("event not open"))); return; }
        if let Err(e)=self.validate(s,&begin) { *lock(&begin.context.completion)=Some(Err(e.to_string())); let _=begin.reply.send(Err(e)); return; }
        *lock(&begin.context.completion)=Some(Err("plugin transaction aborted".into()));
        let before=s.ds().ok().map(|d|d.doc.clone());
        let mark=s.begin_group(); let selected=s.selection().to_vec(); let mut changes=Vec::new(); let mut failed=false;
        let _=begin.reply.send(Ok(json!({"selection":selected})));
        let deadline=Instant::now()+Duration::from_secs(5);
        loop {
            if begin.inv.cancelled() || Instant::now()>deadline { s.finish_group(mark,&begin.inv.label,false); return; }
            let r=match lock(&self.rx).recv_timeout(Duration::from_millis(20)) { Ok(r)=>r,Err(mpsc::RecvTimeoutError::Timeout)=>continue,Err(_)=>{ s.finish_group(mark,&begin.inv.label,false);return; } };
            if key(&r.inv)!=key(&begin.inv) { lock(&self.deferred).push_back(r); continue; }
            match r.kind {
                Kind::Call(ref op,ref args)=> {
                    let valid=self.validate(s,&begin);
                    failed|=valid.is_err();
                    let out=valid.and_then(|_|authorize(&self.contract,self.grants.as_ref(),&begin.inv,op,args)).and_then(|_|self.op(s,&begin.inv,op,args.clone(),&mut changes));
                    let _=r.reply.send(out);
                }
                Kind::Finish(commit)=> {
                    let valid=self.validate(s,&begin); let commit=commit && !failed && valid.is_ok();
                    if commit && before.as_ref().is_some_and(|before|s.ds().is_ok_and(|d|!Arc::ptr_eq(before,&d.doc))) {
                        let reference=lock(&self.state).contributions.iter().find(|c|c.plugin==begin.inv.plugin && c.id==begin.inv.contribution).map(|c|c.reference.clone());
                        if let Some(reference)=reference {
                            if s.doc().is_ok_and(|d|!d.plugins.iter().any(|p|p.id==reference.id)) {
                                if let Ok(d)=s.doc_mut() { d.plugins.push(reference); }
                            }
                        }
                    }
                    s.finish_group(mark,&begin.inv.label,commit);
                    if commit { self.apply(&begin.inv,changes); }
                    let result=valid.and_then(|_|if commit {Ok(Value::Null)} else {Err(OpError::new(Code::Cancelled,"plugin transaction rolled back"))});
                    *lock(&begin.context.completion)=Some(result.as_ref().map(|_|()).map_err(ToString::to_string));
                    let _=r.reply.send(result); return;
                }
                Kind::Begin=> { failed=true; let _=r.reply.send(Err(err("nested begin"))); }
            }
        }
    }
    fn op(&self,s:&mut Session,i:&Invocation,op:&str,a:Value,changes:&mut Vec<(String,Value)>)->Result<Value,OpError> {
        if s.ds().is_err() && !op.starts_with("ui.") && op!="canvas.overlay.set" { return Err(err("no document")); }
        match op {
            "document.info"=>Ok(json!({"name":s.doc().map_err(err)?.name,"uid":s.ds().map_err(err)?.uid})),
            "page.list"=>Ok(json!(s.doc().map_err(err)?.pages.iter().filter_map(|id|s.doc().ok()?.node(*id)).collect::<Vec<_>>())),
            "selection.get"=>Ok(json!({"ids":s.selection()})),
            "viewport.get"=>serde_json::to_value(&s.ds().map_err(err)?.view).map_err(err),
            "node.get"=> { let id=node_id(&a)?; s.doc().map_err(err)?.node(id).ok_or_else(||err("node missing")).and_then(|n|serde_json::to_value(n).map_err(err)) },
            "node.create"|"node.update"|"node.delete"|"selection.set"=> {
                let command=match op { "node.update"=>"node.set","node.delete"=>"edit.delete",other=>other };
                if op=="node.delete" { let id=node_id(&a)?; if s.doc().map_err(err)?.node(id).is_none() {return Err(err("node missing"));} s.ds_mut().map_err(err)?.selection=vec![id]; }
                let mut a=a; if op=="node.update" { if let Some(props)=a.get("props").cloned() { a=json!({"ids":[node_id(&a)?],"props":props}); } }
                let rt=s.plugins.take(); let out=s.execute(command,&a).map_err(err); s.plugins=rt; out
            }
            "ui.panel.set"|"ui.window.open"=> {
                let mut a = a;
                if let Some(object) = a.as_object_mut() {
                    for field in ["patch_rev", "form_metadata", "form_flags"] { object.remove(field); }
                }
                if let Some(raw) = a.get("patch") {
                    let patch: craft_ext_core::form::Patch = serde_json::from_value(raw.clone()).map_err(err)?;
                    let id = if op == "ui.window.open" { a["id"].as_str() } else { a["panel"].as_str() }.ok_or_else(||err("surface missing"))?;
                    let k = format!("{}/{id}", i.plugin);
                    let st = lock(&self.state);
                    let sf = st.surfaces.get(&k).ok_or_else(||err("surface missing"))?;
                    let tree = changes.iter().rev().find(|(o, prior)| o == op && prior.get("panel") == a.get("panel") && prior.get("id") == a.get("id"));
                    let base_rev = tree.map_or(sf.view.rev, |(_,a)| a["patch_rev"].as_u64().unwrap_or(sf.view.rev.saturating_add(1)));
                    let raw_tree = tree.map(|(_,a)| a["tree"].clone()).or_else(||sf.view.tree.clone()).ok_or_else(||err("form missing"))?;
                    let node: Node = serde_json::from_value(raw_tree).map_err(err)?;
                    let Node::Form { mut form, values } = node else { return Err(err("patch requires root form")); };
                    let mut state = craft_ext_core::form::FormState { rev: base_rev, values, ..Default::default() };
                    // Preserve original conditions in staged metadata; show/enable restore them.
                    let metadata = match tree { Some((_,a)) => a.get("form_metadata").cloned(), None => st.form_metadata.get(&k).cloned() };
                    if let Some(metadata) = metadata { form = serde_json::from_value(metadata).map_err(err)?; }
                    let mut flags = match tree { Some((_,a)) => a.get("form_flags").cloned(), None => st.form_flags.get(&k).cloned() }.unwrap_or(json!({}));
                    state.hidden = serde_json::from_value(flags["hidden"].clone()).unwrap_or_default();
                    state.disabled = serde_json::from_value(flags["disabled"].clone()).unwrap_or_default();
                    form.apply_patch(&mut state, &patch).map_err(err)?;
                    a["form_metadata"] = serde_json::to_value(&form).map_err(err)?;
                    flags = json!({"hidden":state.hidden,"disabled":state.disabled});
                    a["form_flags"] = flags;
                    for p in &mut form.params {
                        if state.hidden.contains(&p.key) { p.visible_when = Some(craft_ext_core::form::Cond::Any(vec![])); }
                        if state.disabled.contains(&p.key) { p.enabled_when = Some(craft_ext_core::form::Cond::Any(vec![])); }
                    }
                    a["tree"] = serde_json::to_value(Node::Form { form, values: state.values }).map_err(err)?;
                    a["patch_rev"] = json!(patch.rev);
                    if op == "ui.window.open" && a.get("title").is_none() { a["title"] = json!(sf.view.title); }
                }
                let tree=a.get("tree").ok_or_else(||err("tree missing"))?;
                let n:Node=serde_json::from_value(tree.clone()).map_err(err)?; n.validate().map_err(|e|err(e.0))?;
                if op=="ui.panel.set" { let panel=a["panel"].as_str().ok_or_else(||err("panel missing"))?; if !lock(&self.state).surfaces.get(&format!("{}/{panel}",i.plugin)).is_some_and(|sf|!sf.view.window) { return Err(err("undeclared panel")); } }
                else { let id=a["id"].as_str().ok_or_else(||err("id missing"))?; if id.is_empty() || id.len()>64 || a["title"].as_str().is_none_or(|t|t.len()>200) {return Err(err("invalid window"));} }
                if changes.len()>=64 {return Err(err("UI event budget exceeded"));}
                changes.push((op.into(),a)); Ok(Value::Null)
            }
            "canvas.overlay.set"=> { let ops=a.get("ops").cloned().unwrap_or(json!([])); let draw:Vec<craft_ext_core::ui::DrawOp>=serde_json::from_value(ops.clone()).map_err(err)?; validate_draw(&draw)?; changes.push((op.into(),ops)); Ok(Value::Null) },
            "ui.notify"=> { changes.push((op.into(),a)); Ok(Value::Null) },
            _=>Err(OpError::new(Code::NotFound,"unknown operation")),
        }
    }
    fn apply(&self,i:&Invocation,changes:Vec<(String,Value)>) {
        let mut st=lock(&self.state);
        let Some(owner)=st.contributions.iter().find(|c|c.plugin==i.plugin && c.id==i.contribution).cloned() else {return};
        for (op,a) in changes {
            if op=="canvas.overlay.set" { st.overlays.insert(i.plugin.to_string(),a); continue; }
            if op=="ui.notify" { log::info!("plugin {}: {}",i.plugin,a); continue; }
            let window=op=="ui.window.open"; let id=if window {a["id"].as_str()} else {a["panel"].as_str()}.unwrap_or_default(); let k=format!("{}/{id}",i.plugin);
            if window && !st.surfaces.contains_key(&k) && st.surfaces.values().filter(|s|s.view.window && s.owner.plugin==i.plugin).count()>=4 {continue;}
            if let Some(metadata) = a.get("form_metadata") { st.form_metadata.insert(k.clone(), metadata.clone()); } else { st.form_metadata.remove(&k); }
            if let Some(flags) = a.get("form_flags") { st.form_flags.insert(k.clone(), flags.clone()); } else { st.form_flags.remove(&k); }
            let sf=st.surfaces.entry(k.clone()).or_insert_with(||Surface {view:PluginSurface {key:k,title:a["title"].as_str().unwrap_or("Plugin").into(),tree:None,rev:0,window,open:true},owner:owner.clone()});
            sf.view.tree=Some(a["tree"].clone()); sf.view.rev=a["patch_rev"].as_u64().unwrap_or_else(||sf.view.rev.saturating_add(1)); if window {sf.view.open=true;}
        }
    }
}
fn node_id(a:&Value)->Result<figcraft_doc::NodeId,OpError> { a["id"].as_u64().map(figcraft_doc::NodeId).ok_or_else(||err("id missing")) }
fn validate_draw(ops:&[craft_ext_core::ui::DrawOp])->Result<(),OpError> {
    if ops.len()>craft_ext_core::ui::MAX_DRAW_OPS || serde_json::to_vec(ops).map_err(err)?.len()>1024*1024 {return Err(err("overlay budget exceeded"));}
    let v=serde_json::to_value(ops).map_err(err)?;
    fn finite(v:&Value)->bool { match v {Value::Number(n)=>n.as_f64().is_some_and(|n|n.is_finite() && n.abs()<=1e8),Value::Array(a)=>a.iter().all(finite),Value::Object(o)=>o.values().all(finite),_=>true} }
    if !finite(&v) {return Err(err("invalid draw coordinates"));} Ok(())
}
enum Program { Wasm(Plugin), Native(ExtensionId,Version,PathBuf,PathBuf) }
impl PluginRuntime for Runtime {
    fn commands(&self)->Vec<PluginCommand> { lock(&self.state).contributions.iter().filter(|c|matches!(c.kind,ContributionKind::Command|ContributionKind::Tool)).map(|c|PluginCommand {id:format!("plugin.{}.{}",c.plugin,c.id),label:c.label.clone(),plugin:c.name.clone(),tool:c.kind==ContributionKind::Tool}).collect() }
    fn run_command(&self,s:&mut Session,id:&str,p:&Value)->figcraft_engine::Result<Value> {
        let c=lock(&self.state).contributions.iter().find(|c|format!("plugin.{}.{}",c.plugin,c.id)==id).cloned().ok_or_else(||EngineError::Other("plugin command missing".into()))?;
        if c.kind==ContributionKind::Tool { self.tool_event(s,"cancel",[0.,0.],Value::Null); lock(&self.state).selected_tool=Some(c); return Ok(Value::Null); }
        if self.busy() { return Err(EngineError::Other("plugin gesture in progress".into())); }
        self.deliver(s,&c,json!({"event":"command","id":c.id,"params":p}),true).map_err(EngineError::Other)?; Ok(Value::Null)
    }
    fn pump(&self,s:&mut Session) {
        self.interactive.store(true,Ordering::Relaxed);
        if crate::safe_mode() { self.tool_event(s,"cancel",[0.,0.],Value::Null); return;}
        let generation=Store::at(&self.root).state().map(|s|s.generation).ok();
        if generation!=lock(&self.state).generation {
            self.tool_event(s,"cancel",[0.,0.],Value::Null);
            { let mut st=lock(&self.state); st.workers.clear(); st.contributions.clear(); st.surfaces.clear(); st.form_metadata.clear(); st.form_flags.clear(); st.overlays.clear(); st.selected_tool=None; st.failures.clear(); st.generation=None; }
            if generation.is_some() { if let Err(e)=self.load(s) {log::warn!("plugin reconciliation: {e}");} }
        }
        let cancel={let st=lock(&self.state); st.gesture.as_ref().is_some_and(|g|s.ds().map_or(true,|d|d.uid!=g.uid) || self.grants.stopped(&g.owner.plugin,g.owner.version) || self.grants.generation(&g.owner.plugin)!=g.generation)};
        if cancel {self.tool_event(s,"cancel",[0.,0.],Value::Null);}
        for _ in 0..64 { if lock(&self.deferred).is_empty() && lock(&self.rx).try_recv().map(|r|lock(&self.deferred).push_back(r)).is_err() {break;} self.serve(s); }
        let Ok(d)=s.ds() else {lock(&self.state).seen=None;return}; let now=(d.uid,json!(d.selection),d.page.0);
        let previous=lock(&self.state).seen.replace(now.clone());
        let mut events=Vec::new();
        if previous.as_ref().is_none_or(|p|p.0!=now.0) {
            let statuses=self.manager().check_project(&d.doc.plugins).unwrap_or_default(); if statuses.iter().any(|(_,s)|!s.ready()) {lock(&self.state).prompt=Some(statuses);}
            let cs=lock(&self.state).contributions.clone();
            for c in cs {let _=self.deliver(s,&c,json!({"event":"init"}),true);}
            events.push("document.opened");
        }
        if previous.as_ref().is_none_or(|p|p.1!=now.1) {events.push("selection.changed");}
        if previous.as_ref().is_none_or(|p|p.2!=now.2 || p.0!=now.0) {events.push("page.changed");}
        let cs=lock(&self.state).contributions.clone();
        if !self.busy() { for c in cs {if c.declared.iter().any(|p|*p==Permission::AppEvents) && self.grants.allowed(&c.plugin,&Permission::AppEvents) {for event in &events {let _=self.deliver(s,&c,json!({"event":"app","type":event,"name":event}),false);}}} }
    }
    fn surfaces(&self)->Vec<PluginSurface> {lock(&self.state).surfaces.values().map(|s|s.view.clone()).collect()}
    fn set_open(&self,k:&str,o:bool) {if let Some(s)=lock(&self.state).surfaces.get_mut(k) {s.view.open=o;}}
    fn ui_event(&self,s:&mut Session,k:&str,rev:u64,w:&str,v:Value) { let c=lock(&self.state).surfaces.get(k).filter(|sf|sf.view.open && sf.view.rev==rev).map(|sf|sf.owner.clone()); if let Some(c)=c {if !self.busy() {let surface=k.strip_prefix(&format!("{}/",c.plugin)).unwrap_or(k); if let Err(e)=self.deliver(s,&c,json!({"event":"ui","surface":surface,"rev":rev,"widget":w,"value":v}),true) {s.message=Some(e);}}} }
    fn active_tool(&self)->bool {lock(&self.state).selected_tool.is_some()}
    fn selected_tool(&self)->Option<String> {lock(&self.state).selected_tool.as_ref().map(|c|format!("plugin.{}.{}",c.plugin,c.id))}
    fn busy(&self)->bool {lock(&self.state).gesture.is_some()}
    fn overlays(&self)->Vec<Value> {lock(&self.state).overlays.values().cloned().collect()}
    fn tool_event(&self,s:&mut Session,phase:&str,at:[f64;2],modifiers:Value) {
        let c=lock(&self.state).selected_tool.clone(); let Some(c)=c else{return};
        if phase=="begin" { if self.busy() {return;} let uid=s.ds().map_or(0,|d|d.uid); lock(&self.state).gesture=Some(Gesture {mark:s.begin_group(),uid,owner:c.clone(),generation:self.grants.generation(&c.plugin)}); }
        if !self.busy() {if phase=="cancel" {lock(&self.state).selected_tool=None;} return;}
        let mut result=self.deliver(s,&c,json!({"event":"tool","phase":phase,"at":at,"modifiers":modifiers}),true);
        if phase=="commit" || phase=="cancel" || result.is_err() {
            let gesture=lock(&self.state).gesture.take();
            if let Some(g)=gesture {let mut inv=self.invocation(&g.owner); inv.grant_generation=g.generation;
                let (reply,_)=mpsc::channel();
                let original=Request {inv,context:Dispatch {uid:g.uid,hash:g.owner.reference.sha256.clone(),surface:None,completion:Arc::new(Mutex::new(None))},kind:Kind::Begin,reply};
                let valid=self.validate(s,&original);
                if let Err(e)=&valid { result=Err(e.to_string()); }
                s.finish_group(g.mark,&format!("{} · {}",c.label,c.name),phase=="commit" && result.is_ok() && valid.is_ok());}
            lock(&self.state).overlays.clear();
            if phase=="cancel" || result.is_err() {lock(&self.state).selected_tool=None;}
        }
        if let Err(e)=result {s.message=Some(e);}
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
