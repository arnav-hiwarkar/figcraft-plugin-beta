//! Plugin panels docked in the main window's sidebars (native only). Each sidebar side has one
//! plugin dock beside the built-in panel (between it and the canvas): its open plugin panels are
//! tabs, it sizes from its canvas edge like the built-in panels, and it hides with them (Hide UI,
//! minimized, a narrow window). The layout — which side each panel docks on, its tab order, whether
//! it is shown, each side's active tab and width — is a preference (`plugin_panels`), kept for
//! panels whose plugin is disabled or removed, so a panel comes back where it was when its plugin
//! is enabled again. The layout decides each panel's open flag in the plugin runtime.
//!
//! Only hosting lives here: the panel contents are drawn by [`crate::plugins_ui::render_surface`].

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use egui::{CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use figcraft_engine::ext::PluginSurface;
use serde::{Deserialize, Serialize};

use crate::FigApp;
use crate::i18n::tr;
use crate::theme::{Tokens, semibold};

/// The preference holding the [`Layout`] (in `Prefs::opaque`, so older builds keep it untouched).
pub const PREF_KEY: &str = "plugin_panels";
/// A dock's width limits and default (points).
pub const DOCK_WIDTH: RangeInclusive<f32> = 200.0..=560.0;
pub const DOCK_DEFAULT: f32 = 260.0;
/// Panels remembered at most (the layout of panels from plugins long gone is dropped past this).
const MAX_REMEMBERED: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    #[default]
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
    /// The name of this side's resize edge (in the app's list of panel edges).
    pub fn edge_name(self) -> &'static str {
        match self {
            Side::Left => "plugin_left",
            Side::Right => "plugin_right",
        }
    }
    pub fn from_edge_name(name: &str) -> Option<Side> {
        [Side::Left, Side::Right].into_iter().find(|s| s.edge_name() == name)
    }
}

/// Where one panel docks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub side: Side,
    /// Tab order within its side (ascending).
    pub order: u32,
    /// Shown (a tab in its dock) or hidden (only its menu item).
    pub visible: bool,
}

/// The saved dock layout, keyed by panel (`<plugin id>/<panel id>`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub panels: BTreeMap<String, Place>,
    pub active: BTreeMap<Side, String>,
    pub widths: BTreeMap<Side, f32>,
}


impl Layout {
    /// The saved layout; anything unreadable reads as the default (no panel shown).
    pub fn read(prefs: &figcraft_engine::Prefs) -> Layout {
        prefs.opaque.get(PREF_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    /// Saves the layout into `prefs`; true when it changed.
    pub fn write(&self, prefs: &mut figcraft_engine::Prefs) -> bool {
        let Ok(v) = serde_json::to_value(self) else { return false };
        if prefs.opaque.get(PREF_KEY) == Some(&v) {
            return false;
        }
        prefs.opaque.insert(PREF_KEY.into(), v);
        true
    }

    fn next_order(&self, side: Side) -> u32 {
        self.panels.values().filter(|p| p.side == side).map(|p| p.order.saturating_add(1)).max().unwrap_or(0)
    }

    /// Gives a panel seen for the first time a place: the right side, last, hidden (panels start
    /// closed; they open from the Plugins menu). False when the layout is full.
    pub fn register(&mut self, key: &str) -> bool {
        if self.panels.contains_key(key) {
            return true;
        }
        if self.panels.len() >= MAX_REMEMBERED {
            return false;
        }
        let order = self.next_order(Side::Right);
        self.panels.insert(key.into(), Place { side: Side::Right, order, visible: false });
        true
    }

    pub fn visible(&self, key: &str) -> bool {
        self.panels.get(key).is_some_and(|p| p.visible)
    }

    /// Shows (and brings to the front of its dock) or hides a panel.
    pub fn set_visible(&mut self, key: &str, on: bool) {
        if !self.register(key) {
            return;
        }
        let Some(place) = self.panels.get_mut(key) else { return };
        place.visible = on;
        let side = place.side;
        if on {
            self.active.insert(side, key.into());
        }
    }

    pub fn toggle(&mut self, key: &str) {
        let on = !self.visible(key);
        self.set_visible(key, on);
    }

    /// Docks a panel on `side`, as its last tab.
    pub fn move_to(&mut self, key: &str, side: Side) {
        if !self.register(key) {
            return;
        }
        let order = self.next_order(side);
        let Some(place) = self.panels.get_mut(key) else { return };
        if place.side == side {
            return;
        }
        place.side = side;
        place.order = order;
        if place.visible {
            self.active.insert(side, key.into());
        }
    }

    pub fn select(&mut self, side: Side, key: &str) {
        self.active.insert(side, key.into());
    }

    /// The tabs `side` shows: its visible panels that are `present` (their plugin enabled), in order.
    pub fn tabs(&self, side: Side, present: &[String]) -> Vec<String> {
        let mut tabs: Vec<(u32, &String)> =
            present.iter().filter_map(|k| self.panels.get(k).filter(|p| p.visible && p.side == side).map(|p| (p.order, k))).collect();
        tabs.sort();
        tabs.dedup();
        tabs.into_iter().map(|(_, k)| k.clone()).collect()
    }

    /// The tab `side` shows: the one last chosen if it is still a tab, else the first.
    pub fn active_tab(&self, side: Side, present: &[String]) -> Option<String> {
        let tabs = self.tabs(side, present);
        self.active.get(&side).filter(|k| tabs.contains(k)).cloned().or_else(|| tabs.into_iter().next())
    }

    pub fn width(&self, side: Side) -> f32 {
        let w = self.widths.get(&side).copied().unwrap_or(DOCK_DEFAULT);
        if w.is_finite() { w.clamp(*DOCK_WIDTH.start(), *DOCK_WIDTH.end()) } else { DOCK_DEFAULT }
    }

    pub fn set_width(&mut self, side: Side, w: f32) {
        let w = if w.is_finite() { w.clamp(*DOCK_WIDTH.start(), *DOCK_WIDTH.end()) } else { DOCK_DEFAULT };
        self.widths.insert(side, w);
    }
}

/// The plugin panels (not `ui.window.open` windows) the runtime has now.
fn panels(app: &FigApp) -> Vec<PluginSurface> {
    app.session.plugins.as_ref().map(|rt| rt.surfaces().into_iter().filter(|s| !s.window).collect()).unwrap_or_default()
}

/// Saves `layout` (when it changed) and sets each present panel's open flag from it.
fn commit(app: &mut FigApp, layout: &Layout, present: &[PluginSurface]) {
    if layout.write(&mut app.session.prefs) {
        app.session.prefs_unsaved = true;
    }
    if let Some(rt) = app.session.plugins.clone() {
        for p in present {
            let want = layout.visible(&p.key);
            if p.open != want {
                rt.set_open(&p.key, want);
            }
        }
    }
}

/// Every frame, after the plugins are pumped: new panels get a place, and each panel's open flag
/// follows the layout (a panel whose plugin comes back reopens if it was shown).
pub fn sync(app: &mut FigApp) {
    let present = panels(app);
    if present.is_empty() {
        return;
    }
    let mut layout = Layout::read(&app.session.prefs);
    for p in &present {
        layout.register(&p.key);
    }
    commit(app, &layout, &present);
}

/// The Plugins menu's panel item: shows or hides the panel.
pub fn toggle(app: &mut FigApp, key: &str) {
    let present = panels(app);
    let mut layout = Layout::read(&app.session.prefs);
    layout.toggle(key);
    commit(app, &layout, &present);
}

/// The width the dock on `side` is drawn at, given the `room` left beside the canvas's minimum;
/// `None` when it has no tab or there is no room for it.
pub fn width(app: &FigApp, side: Side, room: f32) -> Option<f32> {
    let keys: Vec<String> = panels(app).into_iter().map(|p| p.key).collect();
    let layout = Layout::read(&app.session.prefs);
    if layout.tabs(side, &keys).is_empty() || room.is_nan() || room < *DOCK_WIDTH.start() {
        return None;
    }
    Some(layout.width(side).min(room))
}

/// A dragged dock edge (`panel_edge`) asked for width `w`; saved once the drag is over.
pub fn set_width(app: &mut FigApp, side: Side, w: f32) {
    let mut layout = Layout::read(&app.session.prefs);
    layout.set_width(side, w);
    layout.write(&mut app.session.prefs);
}

/// One dock: its tab strip (click to show, middle-click or × to hide, right-click to move to the
/// other sidebar or hide) and the active panel's contents.
pub fn show(app: &mut FigApp, ui: &mut Ui, side: Side) {
    let Some(rt) = app.session.plugins.clone() else { return };
    let t = Tokens::get(ui.ctx());
    let present = panels(app);
    let keys: Vec<String> = present.iter().map(|p| p.key.clone()).collect();
    let mut layout = Layout::read(&app.session.prefs);
    let tabs = layout.tabs(side, &keys);
    let Some(active) = layout.active_tab(side, &keys) else { return };
    // The edge facing the canvas, as the built-in panels draw theirs.
    let edge = ui.max_rect();
    let x = if (side == Side::Left) != app.ui.mirrored { edge.max.x } else { edge.min.x };
    ui.painter().vline(x, edge.y_range(), Stroke::new(1.0, t.border));
    let title = |key: &str| present.iter().find(|p| p.key == key).map_or_else(|| key.to_string(), |p| p.title.clone());
    let mut hide: Option<String> = None;
    let mut select: Option<String> = None;
    let mut move_to: Option<String> = None;
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.add_space(6.0);
        for key in &tabs {
            let name = title(key);
            let selected = *key == active;
            let galley = ui.painter().layout_no_wrap(name.clone(), semibold(11.0), t.text);
            let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0 + 16.0, 24.0), Sense::click());
            crate::a11y::tab(&resp, &name, selected);
            if selected || resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(5), t.hover);
            }
            ui.painter().galley(pos2(r.min.x + 8.0, r.center().y - galley.size().y / 2.0), galley, if selected { t.text } else { t.text_secondary });
            // The close mark at the tab's end.
            let close = Rect::from_center_size(pos2(r.max.x - 10.0, r.center().y), vec2(14.0, 14.0));
            let close_resp = ui.interact(close, ui.scope_id().with(("plugin-dock-close", key)), Sense::click()).on_hover_text(tr("Hide panel"));
            let c = close.center();
            let ink = if close_resp.hovered() { t.text } else { t.text_secondary };
            ui.painter().line_segment([c + vec2(-3.5, -3.5), c + vec2(3.5, 3.5)], Stroke::new(1.2, ink));
            ui.painter().line_segment([c + vec2(-3.5, 3.5), c + vec2(3.5, -3.5)], Stroke::new(1.2, ink));
            if close_resp.clicked() || resp.middle_clicked() {
                hide = Some(key.clone());
            } else if resp.clicked() {
                select = Some(key.clone());
            }
            resp.context_menu(|ui| {
                let label = if side == Side::Left { tr("Move to right sidebar") } else { tr("Move to left sidebar") };
                if ui.button(label).clicked() {
                    move_to = Some(key.clone());
                    ui.close();
                }
                if ui.button(tr("Hide panel")).clicked() {
                    hide = Some(key.clone());
                    ui.close();
                }
            });
        }
    });
    ui.add_space(4.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([line.left_center(), line.right_center()], Stroke::new(1.0, t.border));
    let mut events = Vec::new();
    if let Some(sf) = present.iter().find(|p| p.key == active) {
        egui::ScrollArea::vertical().id_salt(("plugin-dock", side)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(10.0).show(ui, |ui| match sf.tree.as_ref().and_then(|t| serde_json::from_value(t.clone()).ok()) {
                Some(node) => events = crate::plugins_ui::render_surface(ui, &sf.key, sf.rev, &node),
                None => {
                    ui.label(tr("The plugin has not drawn this panel yet."));
                }
            });
        });
        let (key, rev) = (sf.key.clone(), sf.rev);
        for (widget, value) in events {
            rt.ui_event(&mut app.session, &key, rev, &widget, value);
        }
    }
    if let Some(k) = select {
        layout.select(side, &k);
    }
    if let Some(k) = move_to {
        layout.move_to(&k, side.other());
    }
    if let Some(k) = hide {
        layout.set_visible(&k, false);
    }
    commit(app, &layout, &present);
    if let Some(m) = app.session.message.take() {
        app.toast(m);
    }
}
