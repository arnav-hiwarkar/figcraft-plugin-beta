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
    let ops = [
        ("document.info", Query, read as fn(&Value) -> Vec<Permission>, "document.read:document"),
        ("page.list", Query, read, "document.read:document"),
        ("selection.get", Query, selection, "document.read:selection"),
        ("node.get", Query, read, "document.read:document (selection-only support deferred)"),
        ("node.create", Edit, edit, "document.edit:document"),
        ("node.update", Edit, edit, "document.edit:document (selection-only support deferred)"),
        ("node.delete", Edit, edit, "document.edit:document"),
        ("selection.set", Query, read, "document.read:document"),
        ("viewport.get", Query, none, "free"),
        ("ui.panel.set", Ui, none, "free"),
        ("ui.window.open", Ui, window, "ui.window"),
        ("ui.notify", Ui, none, "free"),
        ("canvas.overlay.set", Ui, none, "free"),
    ]
    .into_iter()
    .map(|(id, kind, requires, permission_rule)| OpSpec { id, kind, summary: id, args: "{}", result: "{}", requires, permission_rule })
    .collect();
    Ok(AppContract {
        app: AppId::new("figcraft").map_err(|e| OpError::invalid(e.to_string()))?,
        display_name: "FigCraft",
        provided: Provided::new(&[("figcraft.session", 1, 0), ("resource.font", 1, 0), ("craft-theme", 1, 0)]),
        ops,
        events: [
            ("init", "{}", false),
            ("command", "{id}", false),
            ("ui", "{surface,rev,widget,value}", false),
            ("tool", "{phase:begin|update|commit|cancel,at:[x,y],modifiers}", false),
            ("app", "{type:selection.changed|document.opened|page.changed}", true),
            ("dispose", "{}", false),
        ]
        .into_iter()
        .map(|(name, payload, needs_app_events)| EventSpec { name, summary: name, payload, needs_app_events })
        .collect(),
        slots: [
            ("command", "Plugins ▸ <plugin> ▸ <label>"),
            ("panel", "Dockable host-drawn panel"),
            ("tool", "Toolbar"),
            ("event-hook", "App event subscriptions"),
            ("font", "App-local font list"),
        ]
        .into_iter()
        .map(|(kind, summary)| SlotSpec { kind, summary })
        .collect(),
        notes: "Adapter foundation only: document receiver and UI integration are pending; see FigCraft docs/plugin-migration.md. Coordinates are document units. No raster.filter contract is advertised until image-effect dispatch is integrated. Font/theme activation is pending. node.get/update/delete currently require document scope; selected-target authorization must use host-captured scope, never guest flags.",
    })
}
