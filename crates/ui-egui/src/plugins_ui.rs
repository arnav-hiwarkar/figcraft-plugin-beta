//! Craft plugins in the desktop UI (native only): the Plugins menu (Plugins ▸ <plugin> ▸ <label>),
//! the Plugins window (the shared `craft_ext_ui::ManagerView`), the project prompt, host-drawn
//! plugin panels and `ui.window.open` windows (from `craft_ext_core::ui::Node` trees), plugin tools
//! (toolbar strip, canvas pointer gestures, Escape cancels and rolls back) and canvas overlays.
//! Plugin code never runs here: widget changes and pointer events go to the plugin runtime, which
//! services document operations on this (the document) thread.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use craft_ext_core::ui::{DrawOp, Node};
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
    // Keep observing shared-store changes even when no contribution is currently enabled.
    ctx.request_repaint_after(Duration::from_secs(1));
    app.session.pump_plugins();
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
    // Plugin work arrives from workers: keep frames coming while plugins are loaded.
    if !(rt.surfaces().is_empty() && rt.commands().is_empty()) {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// The project prompt, the Plugins window, and plugin panels and windows.
pub fn show(app: &mut FigApp, ctx: &egui::Context) {
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
                    Some(m) => st.view.get_or_insert_with(ManagerView::new).show(ui, &m),
                    None => {
                        ui.label(format!("{}: {}", tr("Plugins are unavailable"), st.manager_error.clone().unwrap_or_default()));
                    }
                }
            });
            st.manager_open = open;
        }
    });
    let Some(rt) = app.session.plugins.clone() else { return };
    let mut events: Vec<(String, u64, String, Value)> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    let right = app.canvas.rect.right_top() + vec2(-300.0, 48.0);
    for (i, sf) in rt.surfaces().into_iter().filter(|s| s.open).enumerate() {
        let mut open = true;
        let mut window = egui::Window::new(&sf.title).id(egui::Id::unique(("figcraft-plugin-surface", &sf.key))).open(&mut open).resizable(true).default_width(280.0);
        // Panels stack along the canvas's right edge until moved; windows open where egui puts them.
        if !sf.window {
            window = window.default_pos(right + vec2(0.0, 40.0 * i as f32));
        }
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
            rt.set_open(&p.key, !p.open);
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

/// Preferences ▸ Plugins: opens the Plugins window.
pub fn preferences_row(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(tr("Plugins"));
        if crate::widgets::button(ui, tr("Manage plugins…"), false, true).clicked() {
            open_manager();
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

/// Canvas pointer input while a plugin tool is selected: press → `begin`, moves → `update`,
/// release → `commit` (the whole gesture is one undo step). Returns true when the plugin tool took
/// the canvas input this frame.
pub fn canvas_input(app: &mut FigApp, ui: &Ui, resp: &egui::Response) -> bool {
    let Some(rt) = app.session.plugins.clone() else { return false };
    if !rt.active_tool() {
        return false;
    }
    let ctx = ui.ctx();
    let m = ctx.input(|i| i.modifiers);
    let modifiers = json!({"shift": m.shift, "alt": m.alt, "cmd": m.command, "ctrl": m.ctrl});
    let page = |app: &FigApp, p: egui::Pos2| {
        let q = app.to_page(p);
        [q.x, q.y]
    };
    let Some(pos) = resp.interact_pointer_pos().or_else(|| ctx.input(|i| i.pointer.hover_pos())) else { return true };
    let at = page(app, pos);
    if resp.drag_started() || (resp.clicked() && !rt.busy()) {
        STATE.with(|st| st.borrow_mut().last_at = Some(at));
        rt.tool_event(&mut app.session, "begin", at, modifiers.clone());
    }
    if resp.dragged() && rt.busy() && STATE.with(|st| st.borrow().last_at) != Some(at) {
        STATE.with(|st| st.borrow_mut().last_at = Some(at));
        rt.tool_event(&mut app.session, "update", at, modifiers.clone());
    }
    if (resp.drag_stopped() || resp.clicked()) && rt.busy() {
        STATE.with(|st| st.borrow_mut().last_at = None);
        rt.tool_event(&mut app.session, "commit", at, modifiers);
    }
    if let Some(msg) = app.session.message.take() {
        app.toast(msg);
    }
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    true
}

fn rgba(c: [f32; 4]) -> Color32 {
    let u = |v: f32| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
    Color32::from_rgba_unmultiplied(u(c[0]), u(c[1]), u(c[2]), u(c[3]))
}

/// The plugins' `canvas.overlay.set` draw lists, in page units, over the canvas.
pub fn draw_overlay(app: &FigApp, painter: &egui::Painter) {
    let Some(rt) = app.session.plugins.clone() else { return };
    let zoom = app.session.ds().map_or(1.0, |d| d.view.zoom) as f32;
    let at = |q: [f32; 2]| app.to_screen(f64::from(q[0]), f64::from(q[1]));
    for list in rt.overlays() {
        let Ok(ops) = serde_json::from_value::<Vec<DrawOp>>(list) else { continue };
        for op in ops.iter().take(craft_ext_core::ui::MAX_DRAW_OPS) {
            match op {
                DrawOp::Line { from, to, width, rgba: c } => {
                    painter.line_segment([at(*from), at(*to)], Stroke::new(width.clamp(0.0, 64.0), rgba(*c)));
                }
                DrawOp::Rect { min, max, fill, stroke } => {
                    let r = Rect::from_two_pos(at(*min), at(*max));
                    painter.rect_filled(r, 0.0, rgba(*fill));
                    if let Some(s) = stroke {
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, rgba(*s)), StrokeKind::Inside);
                    }
                }
                DrawOp::Circle { center, radius, fill } => {
                    painter.circle_filled(at(*center), (radius * zoom).clamp(0.0, 1e5), rgba(*fill));
                }
                DrawOp::Text { at: a, text, size, rgba: c } => {
                    painter.text(at(*a), Align2::LEFT_TOP, text, egui::FontId::proportional(size.clamp(4.0, 96.0)), rgba(*c));
                }
            }
        }
    }
}

/// Deepest tree drawn; `Node::validate` bounds trees, this keeps the renderer bounded regardless.
const MAX_DEPTH: usize = 32;

/// The one renderer seam for plugin panels and windows: draws a validated UI tree (including
/// `Node::Form`) and returns the widget changes as (widget id or form parameter key, value). Today
/// this is egui; swapping in craft-framework's `render_form`/components replaces only this function.
pub fn render_surface(ui: &mut Ui, key: &str, rev: u64, tree: &Node) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    node_ui(ui, key, rev, tree, 0, &mut out);
    out
}

/// Draws one tree; widget changes are pushed to `out` as (widget id, value). Sliders and text
/// fields keep their edit state per widget until released/committed, so a redraw with a new
/// revision mid-edit does not snap the value back.
fn node_ui(ui: &mut Ui, key: &str, rev: u64, n: &Node, depth: usize, out: &mut Vec<(String, Value)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let kids = |ui: &mut Ui, children: &[Node], out: &mut Vec<(String, Value)>| children.iter().for_each(|c| node_ui(ui, key, rev, c, depth + 1, out));
    match n {
        Node::Component { fallback, .. } => node_ui(ui, key, rev, fallback, depth + 1, out),
        Node::Form { form, values } => form_ui(ui, key, rev, form, values, out),
        Node::Column { children } => {
            ui.vertical(|ui| kids(ui, children, out));
        }
        Node::Row { children } => {
            ui.horizontal_wrapped(|ui| kids(ui, children, out));
        }
        Node::Group { label, children } => {
            ui.group(|ui| {
                ui.label(egui::RichText::new(label).strong());
                kids(ui, children, out);
            });
        }
        Node::Label { text } => {
            ui.label(text);
        }
        Node::Separator => {
            ui.separator();
        }
        Node::Button { id, label } => {
            if ui.push_id((key, id), |ui| ui.button(label)).inner.clicked() {
                out.push((id.clone(), Value::Null));
            }
        }
        Node::Checkbox { id, label, value } => {
            let mut v = *value;
            if ui.push_id((key, id), |ui| ui.checkbox(&mut v, label)).inner.changed() {
                out.push((id.clone(), json!(v)));
            }
        }
        Node::Slider { id, label, value, min, max } => {
            let (lo, hi) = if min.is_finite() && max.is_finite() && min <= max { (*min, *max) } else { (0.0, 1.0) };
            let edit_id = egui::Id::unique(("plugin-slider-edit", key, id));
            let (mut v, dragging, old_rev) = ui.ctx().data_mut(|d| d.get_temp::<(f64, bool, u64)>(edit_id)).unwrap_or((*value, false, rev));
            if !dragging && old_rev != rev {
                v = *value;
            }
            let r = ui.push_id((key, id), |ui| ui.add(egui::Slider::new(&mut v, lo..=hi).text(label))).inner;
            let active = r.dragged() || r.is_pointer_button_down_on();
            ui.ctx().data_mut(|d| d.insert_temp(edit_id, (v, active, rev)));
            if r.drag_stopped() || (dragging && !active) || (r.changed() && !active) {
                out.push((id.clone(), json!(v)));
            }
        }
        Node::TextInput { id, label, value } => {
            let edit_id = egui::Id::unique(("plugin-text-edit", key, id));
            let (mut v, mut sent, old) = ui.ctx().data_mut(|d| d.get_temp::<(String, String, String)>(edit_id)).unwrap_or((value.clone(), value.clone(), value.clone()));
            // The plugin changed the value: take it, unless the user is mid-edit.
            let mut focused = false;
            ui.horizontal(|ui| {
                ui.label(label);
                let r = ui.push_id((key, id), |ui| ui.text_edit_singleline(&mut v)).inner;
                focused = r.has_focus();
                if (r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))) && v != sent {
                    out.push((id.clone(), json!(v)));
                    sent = v.clone();
                }
            });
            if !focused && old != *value {
                v = value.clone();
                sent = value.clone();
            }
            ui.ctx().data_mut(|d| d.insert_temp(edit_id, (v, sent, value.clone())));
        }
        Node::Select { id, label, options, selected } => {
            let mut sel = *selected;
            let shown = options.get(sel as usize).cloned().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(label);
                egui::ComboBox::from_id_salt(("plugin-select", key, id)).selected_text(shown).show_ui(ui, |ui| {
                    for (i, o) in options.iter().enumerate() {
                        ui.selectable_value(&mut sel, i as u32, o);
                    }
                });
            });
            if sel != *selected {
                out.push((id.clone(), json!(sel)));
            }
        }
        Node::Color { id, label, rgba: c } => {
            ui.horizontal(|ui| {
                ui.label(label);
                let mut v = c.map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 });
                if ui.push_id((key, id), |ui| ui.color_edit_button_rgba_unmultiplied(&mut v)).inner.changed() {
                    out.push((id.clone(), json!(v)));
                }
            });
        }
        Node::Image { width, height, rgba: px } => {
            let (w, h) = (*width as usize, *height as usize);
            if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(4)) != Some(px.len()) {
                return;
            }
            let img = egui::ColorImage::from_rgba_unmultiplied([w, h], px);
            let tex = ui.ctx().load_texture(format!("plugin-image-{key}-{rev}-{w}x{h}"), img, egui::TextureOptions::LINEAR);
            ui.image((tex.id(), vec2(*width as f32, *height as f32)));
        }
        Node::Canvas { id, label, width, height, ops } => {
            let size = vec2(width.clamp(1.0, 2048.0), height.clamp(1.0, 2048.0));
            let (r, p) = ui.allocate_painter(size, Sense::click_and_drag());
            let r = r.on_hover_text(label);
            let o = r.rect.min;
            let at = |q: [f32; 2]| pos2(o.x + q[0], o.y + q[1]);
            for op in ops.iter().take(craft_ext_core::ui::MAX_DRAW_OPS) {
                match op {
                    DrawOp::Line { from, to, width, rgba: c } => {
                        p.line_segment([at(*from), at(*to)], Stroke::new(width.clamp(0.0, 64.0), rgba(*c)));
                    }
                    DrawOp::Rect { min, max, fill, stroke } => {
                        let rect = Rect::from_two_pos(at(*min), at(*max));
                        p.rect_filled(rect, 0.0, rgba(*fill));
                        if let Some(s) = stroke {
                            p.rect_stroke(rect, 0.0, Stroke::new(1.0, rgba(*s)), StrokeKind::Inside);
                        }
                    }
                    DrawOp::Circle { center, radius, fill } => {
                        p.circle_filled(at(*center), radius.clamp(0.0, 4096.0), rgba(*fill));
                    }
                    DrawOp::Text { at: a, text, size, rgba: c } => {
                        p.text(at(*a), Align2::LEFT_TOP, text, egui::FontId::proportional(size.clamp(4.0, 96.0)), rgba(*c));
                    }
                }
            }
            let kind = if r.drag_started() {
                Some("down")
            } else if r.dragged() {
                Some("drag")
            } else if r.drag_stopped() || r.clicked() {
                Some("up")
            } else {
                None
            };
            if let (Some(kind), Some(pos)) = (kind, r.interact_pointer_pos()) {
                out.push((id.clone(), json!({"kind": kind, "at": [pos.x - o.x, pos.y - o.y]})));
            }
        }
    }
}

/// Form values remain typed; choice labels never become protocol values.
fn form_ui(ui: &mut Ui, surface: &str, rev: u64, form: &craft_ext_core::form::Form, values: &craft_ext_core::form::Values, out: &mut Vec<(String, Value)>) {
    use craft_ext_core::form::{ParamKind as K, Value as V, Unit};
    if form.validate_values(values).is_err() { return; }
    let mut effective = form.defaults();
    effective.extend(values.clone());
    ui.label(egui::RichText::new(&form.title.english).strong());
    for p in &form.params {
        if p.visible_when.as_ref().is_some_and(|c| !c.eval(&effective)) { continue; }
        let enabled = p.enabled_when.as_ref().is_none_or(|c| c.eval(&effective));
        if matches!(p.kind, K::Section) { ui.separator(); ui.add_enabled(enabled, egui::Label::new(egui::RichText::new(&p.label.english).heading())); continue; }
        let Some(source) = effective.get(&p.key) else { continue };
        let id = egui::Id::unique(("plugin-form-edit", surface, &form.key, &p.key));
        let (mut value, active, old_rev) = ui.ctx().data_mut(|d| d.get_temp::<(V, bool, u64)>(id)).unwrap_or((source.clone(), false, rev));
        if !active && old_rev != rev { value = source.clone(); }
        let before = value.clone();
        let mut editing = false;
        ui.push_id(id, |ui| ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let label = ui.label(&p.label.english);
                if let Some(help) = &p.help { label.on_hover_text(&help.english); }
                let response = match (&p.kind, &mut value) {
                    (K::Range { min, max, step, .. }, V::Number(v)) => {
                        let mut slider = egui::Slider::new(v, *min..=*max);
                        if let Some(step) = step { slider = slider.step_by(*step); }
                        Some(ui.add(slider))
                    }
                    (K::Int { min, max, .. }, V::Number(v)) => Some(ui.add(egui::DragValue::new(v).range(*min as f64..=*max as f64).speed(1.0).max_decimals(0))),
                    (K::Number { min, max, .. }, V::Number(v)) => Some(ui.add(egui::DragValue::new(v).range(min.unwrap_or(f64::NEG_INFINITY)..=max.unwrap_or(f64::INFINITY)))),
                    (K::Angle { .. }, V::Number(v)) => Some(ui.add(egui::DragValue::new(v).suffix("°"))),
                    (K::Bool { .. }, V::Bool(v)) => Some(ui.checkbox(v, "")),
                    (K::Text { multiline, .. }, V::String(v)) => Some(if *multiline { ui.text_edit_multiline(v) } else { ui.text_edit_singleline(v) }),
                    (K::Choice { options, .. }, V::String(v)) => {
                        egui::ComboBox::from_id_salt(id).selected_text(options.iter().find(|o| o.id == *v).map(|o| o.label.english.as_str()).unwrap_or("")).show_ui(ui, |ui| {
                            for o in options { ui.selectable_value(v, o.id.clone(), &o.label.english); }
                        }); None
                    }
                    (K::Radio { options, .. }, V::String(v)) => { for o in options { ui.radio_value(v, o.id.clone(), &o.label.english); } None }
                    (K::Color { alpha, .. }, V::Color(v)) => {
                        // Numeric channels preserve HDR and straight working-space values.
                        for channel in v.iter_mut().take(if *alpha {4} else {3}) {
                            let r = ui.add(egui::DragValue::new(channel).speed(0.01));
                            editing |= r.dragged() || r.has_focus();
                        } None
                    }
                    (K::Point { space, .. }, V::Point(v)) => {
                        for channel in v { let r = ui.add(egui::DragValue::new(channel)); editing |= r.dragged() || r.has_focus(); }
                        ui.label(match space { craft_ext_core::form::PointSpace::LayerPixels => "px", craft_ext_core::form::PointSpace::ImagePercent => "%" }); None
                    }
                    _ => None,
                };
                if let Some(r) = response { editing |= r.dragged() || r.has_focus(); if let Some(help) = &p.help { r.on_hover_text(&help.english); } }
                if let Some(unit) = &p.unit { ui.label(match unit { Unit::Pixels => "px", Unit::Percent => "%", Unit::Degrees => "°", Unit::Seconds => "s", Unit::Frames => "frames", Unit::Custom(s) => s }); }
            });
        }));
        ui.ctx().data_mut(|d| d.insert_temp(id, (value.clone(), editing, rev)));
        if enabled && value != before {
            let candidate = [(p.key.clone(), value.clone())].into_iter().collect();
            if form.validate_values(&candidate).is_ok() { out.push((p.key.clone(), json!(value))); }
        }
    }
}
