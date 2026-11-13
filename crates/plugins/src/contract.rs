//! Contract permission rules are conservative: caller-provided selection flags are never trusted.
use craft_ext_core::{
    contract::Provided,
    id::AppId,
    permission::{DocScope, Permission},
};
use craft_ext_host::api::{AppContract, EventSpec, OpError, OpKind, OpSpec, SlotSpec, none};
use serde_json::Value;
fn read(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentRead(DocScope::Document)]
}
fn selection(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentRead(DocScope::Selection)]
}
fn edit(_: &Value) -> Vec<Permission> {
    vec![Permission::DocumentEdit(DocScope::Document)]
}
fn window(_: &Value) -> Vec<Permission> {
    vec![Permission::UiWindow]
}

pub fn contract() -> Result<AppContract, OpError> {
    use OpKind::{Edit, Query, Ui};
    // (id, kind, rule, permission text, summary, args, result). Args and results are what the adapter
    // (`runtime.rs`) accepts and returns; node.* forward to the engine commands of the same shape.
    let ops = [
        ("document.info", Query, read as fn(&Value) -> Vec<Permission>, "document.read:document", "The active document's name and session id", "{}", "{name, uid}"),
        ("page.list", Query, read, "document.read:document", "The document's pages as layer objects", "{}", "[page]"),
        ("selection.get", Query, selection, "document.read:selection", "The selected layer ids", "{}", "{ids: [id]}"),
        ("node.get", Query, read, "document.read:document (selection-only support deferred)", "One layer as JSON", "{id}", "layer"),
        (
            "node.create",
            Edit,
            edit,
            "document.edit:document",
            "Create a layer (the engine's node.create)",
            "{type: rectangle|ellipse|polygon|star|line|arrow|vector|text|frame|section|component, x?, y?, width?, height?, parent?, index?, name?, text?, props? {node.set keys, e.g. fill: \"#3380e6\"}, select?}",
            "{id}",
        ),
        ("node.update", Edit, edit, "document.edit:document (selection-only support deferred)", "Change a layer's properties (the engine's node.set)", "{id, props: {x?, y?, width?, height?, name?, fill?, …}}", "engine result"),
        ("node.delete", Edit, edit, "document.edit:document", "Delete a layer", "{id}", "engine result"),
        ("selection.set", Query, read, "document.read:document", "Replace the selection (the engine's select.set)", "{ids: [id]}", "engine result"),
        ("viewport.get", Query, none, "free", "The canvas view (pan and zoom)", "{}", "view"),
        ("ui.panel.set", Ui, none, "free", "Draw a declared panel; applied when the event commits", "{panel, tree} | {panel, patch}", "null"),
        ("ui.window.open", Ui, window, "ui.window", "Open or redraw a plugin window (at most four)", "{id, title, tree} | {id, patch}", "null"),
        ("ui.notify", Ui, none, "free", "Log a short message for the person", "{message}", "null"),
        ("canvas.overlay.set", Ui, none, "free", "Draw guides on the canvas in page units; cleared when a tool gesture ends", "{ops: [line|rect|circle|text draw op]}", "null"),
    ]
    .into_iter()
    .map(|(id, kind, requires, permission_rule, summary, args, result)| OpSpec { id, kind, summary, args, result, requires, permission_rule })
    .collect();
    Ok(AppContract {
        app: AppId::new("figcraft").map_err(|e| OpError::invalid(e.to_string()))?,
        display_name: "FigCraft",
        provided: Provided::new(&[("figcraft.session", 1, 0), ("resource.font", 1, 0), ("craft-theme", 1, 0)]),
        ops,
        events: [
            ("init", "{}", false, "Startup, and again when a document opens; draw panels here"),
            ("command", "{id, params}", false, "A command contribution ran (menu, Actions, CLI `run --plugins`)"),
            ("ui", "{surface,rev,widget,value}", false, "A panel or window widget changed; form widgets send the parameter key"),
            ("tool", "{phase:begin|update|commit|cancel,at:[x,y],modifiers}", false, "Pointer gesture of the selected tool, in page units; Escape sends cancel"),
            ("app", "{type|name:selection.changed|document.opened|page.changed}", true, "Something changed in the app"),
            ("dispose", "{}", false, "The worker is stopping"),
        ]
        .into_iter()
        .map(|(name, payload, needs_app_events, summary)| EventSpec { name, summary, payload, needs_app_events })
        .collect(),
        slots: [
            ("command", "Plugins ▸ <plugin> ▸ <label>"),
            ("panel", "Floating host-drawn panel, toggled by its title in the Plugins menu (docking pending)"),
            ("tool", "Plugin tool strip above the main toolbar, and Plugins ▸ <plugin> ▸ <label>"),
            ("event-hook", "App event subscriptions"),
            ("font", "App-local font list"),
        ]
        .into_iter()
        .map(|(kind, summary)| SlotSpec { kind, summary })
        .collect(),
        notes: "Each event is one undo step labelled \"<label> · <plugin>\"; a failed, timed-out or revoked event rolls back. Tool gestures (begin..commit) are one step; cancel rolls back. Coordinates are document units. No raster.filter contract is advertised until image-effect dispatch is integrated. Font/theme activation is pending. node.get/update/delete currently require document scope; selected-target authorization must use host-captured scope, never guest flags. Panels and windows render Node::Form (every parameter kind, defaults, host-evaluated conditions; unknown components show their fallback); form widget events carry the parameter key and choices their option id. {panel|id, patch} patches a root form: patch schema 1, base_rev is the surface revision, rev must increase, published atomically with the event. Optional wasm describe/update exports are unused until raster.filter is offered.",
    })
}
