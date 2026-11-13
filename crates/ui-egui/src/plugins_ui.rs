//! Craft plugins in the desktop UI (native only): the Plugins menu (Plugins ▸ <plugin> ▸ <label>),
//! the Plugins window (the shared `craft_ext_ui::ManagerView`), the project prompt, host-drawn
//! plugin panels and `ui.window.open` windows (from `craft_ext_core::ui::Node` trees), plugin tools
//! (toolbar strip, canvas pointer gestures, Escape cancels and rolls back) and canvas overlays.
//! Plugin code never runs here: widget changes and pointer events go to the plugin runtime, which
//! services document operations on this (the document) thread.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use craft_ext_core::ui::tree::{DrawOp, Node};
use craft_ext_ui::plugin_ui::{PluginUi, EngineChoice, Host, Surface, Xf, paint_ops};
use craft_ext_store::Manager;
use craft_ext_ui::{ManagerView, ProjectPrompt};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::tr;
use crate::theme::Tokens;

/// Catalogue and kill-list refresh period while the app runs (B8).
const REFRESH_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Default)]
struct State {
    view: Option<ManagerView>,
    manager: Option<Arc<Manager>>,
    manager_error: Option<String>,
    manager_open: bool,
    prompt: Option<ProjectPrompt>,
    refreshed: Option<Instant>,
    /// The last pointer position sent during a tool gesture (page units), so a still pointer
    /// sends no updates.
    last_at: Option<[f64; 2]>,
    /// The press under way began as canvas navigation (Space or middle drag, the zoom tool).
    nav_press: bool,
    /// Plugins whose Resume was clicked in the Plugins window this frame.
    resume: Vec<String>,
    renderer: PluginUi,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn manager(st: &mut State) -> Option<Arc<Manager>> {
    if st.manager.is_none() && st.manager_error.is_none() {
        match figcraft_plugins::manager() {
            Ok(m) => st.manager = Some(Arc::new(m)),
            Err(e) => st.manager_error = Some(e),
        }
    }
    st.manager.clone()
}

/// Opens the Plugins window (menu, Preferences).
pub fn open_manager() {
    STATE.with(|st| st.borrow_mut().manager_open = true);
}

/// Early every frame: serve plugin requests and deliver app events, refresh the catalogue at
/// startup and every six hours, pick up a project-open prompt, and take Escape for a plugin tool.
pub fn frame(app: &mut FigApp, ctx: &egui::Context) {
    let choice = if app.session.prefs.opaque.get("plugin_ui_engine").and_then(Value::as_str) == Some("framework") { EngineChoice::Framework } else { EngineChoice::Egui };
    STATE.with(|st| { let mut st = st.borrow_mut(); if st.renderer.choice() != choice { st.renderer = PluginUi::new(choice); } });
    app.session.pump_plugins();
    // Docked panels: new ones get a place, and each panel's open flag follows the saved layout.
    crate::plugin_dock::sync(app);
    if let Some(m) = app.session.message.take() {
        app.toast(m);
    }
    let Some(rt) = app.session.plugins.clone() else { return };
    let prompt = STATE.with(|st| {
        let mut st = st.borrow_mut();
        if st.refreshed.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
            st.refreshed = Some(Instant::now());
            figcraft_plugins::refresh_in_background();
        }
        st.prompt.is_none()
    });
    if prompt && let Some(statuses) = figcraft_plugins::runtime().and_then(|r| r.take_project_prompt()) {
        STATE.with(|st| st.borrow_mut().prompt = Some(ProjectPrompt::new(statuses)));
    }
    // Escape ends a plugin tool (cancelling and rolling back an open gesture) before any shortcut
    // sees it.
    if rt.active_tool() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        rt.tool_event(&mut app.session, "cancel", [0.0, 0.0], Value::Null);
        STATE.with(|st| st.borrow_mut().last_at = None);
    }
    // A live gesture needs preview frames; installed commands and idle panels do not.
    if rt.busy() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// The project prompt, the Plugins window (with the plugins paused this session and their Resume
/// action), and plugin windows (`ui.window.open`; panels dock in [`crate::plugin_dock`]).
pub fn show(app: &mut FigApp, ctx: &egui::Context) {
    let paused = app.session.plugins.as_ref().map(|rt| rt.paused()).unwrap_or_default();
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if st.prompt.is_some()
            && let Some(m) = manager(&mut st)
            && let Some(p) = st.prompt.as_mut()
            && p.show(ctx, &m).is_some()
        {
            st.prompt = None;
        }
        if st.manager_open {
            let mut open = true;
            egui::Window::new(tr("Plugins")).id(egui::Id::unique("figcraft-plugins-manager")).open(&mut open).default_width(560.0).show(ctx, |ui| {
                if figcraft_plugins::safe_mode() {
                    ui.label(tr("Safe mode: no plugin code runs this session. Changes apply after a restart without FIGCRAFT_SAFE_MODE=1."));
                }
                match manager(&mut st) {
                    Some(m) => {
                        let mut resume = Vec::new();
                        st.view.get_or_insert_with(ManagerView::new).show_with_app_problems(ui, &m, |ui| paused_problems(ui, &paused, &mut resume));
                        st.resume.extend(resume);
                    },
                    None => {
                        ui.label(format!("{}: {}", tr("Plugins are unavailable"), st.manager_error.clone().unwrap_or_default()));
                    }
                }
            });
            st.manager_open = open;
        }
    });
    for id in STATE.with(|st| std::mem::take(&mut st.borrow_mut().resume)) {
        app.run("plugins.resume", json!({"id": id}));
    }
    let Some(rt) = app.session.plugins.clone() else { return };
    let mut events: Vec<(String, u64, String, Value)> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    for sf in rt.surfaces().into_iter().filter(|s| s.open && s.window) {
        let mut open = true;
        let window = egui::Window::new(&sf.title)
            .id(egui::Id::unique(("figcraft-plugin-surface", &sf.key)))
            .open(&mut open)
            .resizable(true)
            .default_width(280.0);
        window.show(ctx, |ui| match sf.tree.as_ref().and_then(|t| serde_json::from_value::<Node>(t.clone()).ok()) {
            Some(node) => {
                let out = render_surface(ui, &sf.key, sf.rev, &node);
                events.extend(out.into_iter().map(|(w, v)| (sf.key.clone(), sf.rev, w, v)));
            }
            None => {
                ui.label(tr("The plugin has not drawn this panel yet."));
            }
        });
        if !open {
            closed.push(sf.key.clone());
        }
    }
    for key in closed {
        rt.set_open(&key, false);
    }
    for (key, rev, widget, value) in events {
        rt.ui_event(&mut app.session, &key, rev, &widget, value);
    }
    if let Some(m) = app.session.message.take() {
        app.toast(m);
    }
}

/// The Plugins top-level menu: Manage plugins…, the panels, then each plugin's commands and tools.
/// Returns whether an item ran.
pub fn menu(app: &mut FigApp, ui: &mut Ui, t: &Tokens) -> bool {
    use crate::menus::{item, separator, submenu};
    if item(ui, tr("Manage plugins…"), "", true, false, t) {
        open_manager();
        return true;
    }
    let Some(rt) = app.session.plugins.clone() else { return false };
    let panels: Vec<_> = rt.surfaces().into_iter().filter(|s| !s.window).collect();
    let commands = rt.commands();
    if !panels.is_empty() || !commands.is_empty() {
        separator(ui);
    }
    for p in &panels {
        if item(ui, &p.title, "", true, p.open, t) {
            crate::plugin_dock::toggle(app, &p.key);
            return true;
        }
    }
    let mut plugins: Vec<&str> = commands.iter().map(|c| c.plugin.as_str()).collect();
    plugins.dedup();
    let selected = rt.selected_tool();
    let mut picked = None;
    for plugin in plugins {
        let key = format!("Plugins/{plugin}");
        if let Some(id) = submenu(ui, &key, plugin, t, |ui| {
            let mut picked = None;
            for c in commands.iter().filter(|c| c.plugin == plugin) {
                if item(ui, &c.label, "", true, c.tool && selected.as_deref() == Some(c.id.as_str()), t) {
                    picked = Some(c.id.clone());
                }
            }
            picked
        }) {
            picked = Some(id);
        }
    }
    if let Some(id) = picked {
        app.run(&id, json!({}));
        return true;
    }
    false
}

/// The Plugins window's problems from this session: each plugin paused after three failures, its
/// last error, and Resume (clears the count and starts the plugin afresh; pauses are never saved).
fn paused_problems(ui: &mut Ui, paused: &[figcraft_engine::ext::PausedPlugin], resume: &mut Vec<String>) {
    if paused.is_empty() {
        return;
    }
    ui.group(|ui| {
        ui.label(egui::RichText::new(tr("Problems this session")).strong());
        for p in paused {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} ({}): {}", p.name, p.id, tr("paused after three failures")));
                if ui.button(tr("Resume")).clicked() {
                    resume.push(p.id.clone());
                }
            });
            if !p.reason.is_empty() {
                ui.label(egui::RichText::new(p.reason.chars().take(300).collect::<String>()).small());
            }
        }
    });
}

/// Preferences ▸ Plugins: opens the Plugins window.
pub fn preferences_row(app: &mut FigApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(tr("Plugins"));
        if crate::widgets::button(ui, tr("Manage plugins…"), false, true).clicked() {
            open_manager();
        }
        let mut framework = app.session.prefs.opaque.get("plugin_ui_engine").and_then(Value::as_str) == Some("framework");
        let before = framework;
        egui::ComboBox::from_id_salt("plugin-ui-engine").selected_text(if framework { "Framework (egui fallback)" } else { "Egui" }).show_ui(ui, |ui| {
            ui.selectable_value(&mut framework, false, "Egui");
            ui.selectable_value(&mut framework, true, "Framework (egui fallback)");
        });
        if framework != before {
            app.session.prefs.opaque.insert("plugin_ui_engine".into(), json!(if framework { "framework" } else { "egui" }));
            app.session.prefs_unsaved = true;
        }
    });
}

/// The plugin tools as a small strip above the toolbar pill: one cell per tool, showing its
/// label's first letter (the label is its tooltip).
pub fn tool_strip(app: &mut FigApp, ui: &mut Ui, pill: Rect, t: &Tokens) {
    let Some(rt) = app.session.plugins.clone() else { return };
    let tools: Vec<_> = rt.commands().into_iter().filter(|c| c.tool).collect();
    if tools.is_empty() {
        return;
    }
    let selected = rt.selected_tool();
    let cell = 32.0;
    let width = 8.0 * 2.0 + cell * tools.len() as f32 + 4.0 * tools.len().saturating_sub(1) as f32;
    let strip = Rect::from_min_size(pos2(pill.min.x, pill.min.y - 8.0 - 40.0), vec2(width, 40.0));
    ui.interact(strip, ui.scope_id().with("plugin_tools"), Sense::CLICK | Sense::DRAG);
    ui.painter().rect_filled(strip, CornerRadius::same(11), t.panel);
    ui.painter().rect_stroke(strip, CornerRadius::same(11), Stroke::new(1.0, t.border), StrokeKind::Inside);
    let mut x = strip.min.x + 8.0;
    for c in tools {
        let r = Rect::from_min_size(pos2(x, strip.center().y - cell / 2.0), vec2(cell, cell));
        let active = selected.as_deref() == Some(c.id.as_str());
        let tip = format!("{} · {}", c.label, c.plugin);
        let resp = ui.interact(r, ui.scope_id().with(("plugin_tool", &c.id)), Sense::click()).on_hover_text(&tip);
        crate::a11y::toggle(&resp, &tip, active);
        let bg = if active {
            t.accent
        } else if resp.hovered() {
            t.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(r, CornerRadius::same(5), bg);
        let letter: String = c.label.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
        ui.painter().text(r.center(), Align2::CENTER_CENTER, letter, crate::theme::regular(14.0), if active { t.accent_on } else { t.text });
        if resp.clicked() {
            if active {
                rt.tool_event(&mut app.session, "cancel", [0.0, 0.0], Value::Null);
            } else {
                app.run(&c.id, json!({}));
            }
        }
        x += cell + 4.0;
    }
}

/// Canvas pointer input while a plugin tool is selected: primary press → `begin` (at the press
/// point), moves → `update`, release → `commit` (the whole gesture is one undo step). `navigating`:
/// the canvas navigation ([`crate::canvas`]) has the pointer this frame (Space or middle drag, the
/// zoom tool); a press it took stays navigation until every button is up and never reaches the
/// plugin. Coordinates are page units from the view as it is this frame, so a zoom or pan mid-gesture
/// sends an `update` with the pointer's new page point.
pub fn canvas_input(app: &mut FigApp, ui: &Ui, resp: &egui::Response, navigating: bool) {
    let Some(rt) = app.session.plugins.clone() else { return };
    if !rt.active_tool() {
        return;
    }
    let ctx = ui.ctx();
    let (m, primary_down, any_down, hover, origin) =
        ctx.input(|i| (i.modifiers, i.pointer.primary_down(), i.pointer.any_down(), i.pointer.hover_pos(), i.pointer.press_origin()));
    let modifiers = json!({"shift": m.shift, "alt": m.alt, "cmd": m.command, "ctrl": m.ctrl});
    let page = |app: &FigApp, p: egui::Pos2| {
        let q = app.to_page(p);
        [q.x, q.y]
    };
    let skip = STATE.with(|st| {
        let mut st = st.borrow_mut();
        let skip = navigating || st.nav_press;
        st.nav_press = (st.nav_press || navigating) && any_down;
        skip
    });
    if skip {
        // A gesture whose button went up while navigating ends where the plugin last saw it.
        if rt.busy() && !primary_down {
            let at = STATE.with(|st| st.borrow_mut().last_at.take()).unwrap_or_default();
            rt.tool_event(&mut app.session, "commit", at, modifiers);
        }
    } else if let Some(pos) = resp.interact_pointer_pos().or(hover) {
        let at = page(app, pos);
        let started = resp.drag_started_by(egui::PointerButton::Primary) || (resp.clicked() && !rt.busy());
        if started && !rt.busy() {
            let from = if resp.clicked() { at } else { origin.map_or(at, |o| page(app, o)) };
            STATE.with(|st| st.borrow_mut().last_at = Some(from));
            rt.tool_event(&mut app.session, "begin", from, modifiers.clone());
        }
        if rt.busy() && STATE.with(|st| st.borrow().last_at) != Some(at) {
            STATE.with(|st| st.borrow_mut().last_at = Some(at));
            rt.tool_event(&mut app.session, "update", at, modifiers.clone());
        }
        if rt.busy() && !primary_down {
            STATE.with(|st| st.borrow_mut().last_at = None);
            rt.tool_event(&mut app.session, "commit", at, modifiers);
        }
    }
    if let Some(msg) = app.session.message.take() {
        app.toast(msg);
    }
    // The canvas draws the pan or zoom cursor over this while Space or Z is held.
    if resp.hovered() && !skip {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
}

/// The plugins' canvas overlays, using the same draw vocabulary as panel canvases.
pub fn draw_overlay(app: &FigApp, painter: &egui::Painter) {
    let Some(rt) = app.session.plugins.clone() else { return };
    let zoom = app.session.ds().map_or(1.0, |d| d.view.zoom) as f32;
    for list in rt.overlays() {
        let Ok(ops) = serde_json::from_value::<Vec<DrawOp>>(list) else { continue };
        paint_ops(painter, Xf::new(app.to_screen(0.0, 0.0), zoom), painter.clip_rect(), &ops, &|_| None);
    }
}

struct FigHost;
impl Host for FigHost {
    fn tr(&self, english: &str) -> String { tr(english).to_string() }
    fn form_titles(&self) -> bool { true }
}

/// Shared renderer for docked panels, windows, forms and custom canvases.
pub fn render_surface(ui: &mut Ui, key: &str, rev: u64, tree: &Node) -> Vec<(String, Value)> {
    STATE.with(|st| st.borrow_mut().renderer.draw(ui, Surface { key, rev, tree }, &mut FigHost))
        .into_iter().filter_map(|e| serde_json::to_value(e.value).ok().map(|v| (e.widget, v))).collect()
}
