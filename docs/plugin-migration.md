# Plugin migration status

The shared adapter is active (`crates/plugins`, `figcraft_plugins::activate` from the desktop app's
startup, after startup files open). Current implementation notes and verification limits:

- STARTUP / SAFE-MODE: `activate` attaches one process runtime; `FIGCRAFT_SAFE_MODE=1` attaches it
  without loading or running plugin code (Bridge, Worker and LiveGrants also refuse).
- SESSION-REGISTRY: enabled contributions from GrantView; one worker per `plugin@version/module`
  (`native` for native packages); Invocation carries declared + optional permissions; init,
  command, ui, tool, app and (worker-exit) events routed.
- DOCUMENT-TRANSACTIONS: requests are served only on the document thread (`Runtime::serve`, from
  `pump`/`deliver`). Begin validates the dispatch-time document uid, release digest, enablement,
  grant generation and stop list; every op re-runs that check plus `authorize()` immediately
  before executing. An event is one history step `<label> · <plugin>` through
  `Session::begin_group/finish_group` (engine `ext.rs`); journal and events are buffered and
  dropped on rollback (failure, timeout, revocation, cancellation). Other commands are refused
  while a tool gesture is open.
- COMMANDS: `plugin.<id>.<contribution>` through `Session::execute`; menu Plugins ▸ <plugin> ▸
  <label> (and the Actions/automation paths that use `execute`).
- PANEL-UI: panels and `ui.window.open` windows drawn by `ui-egui/src/plugins_ui.rs` (per-widget
  slider/text edit state, tree revisions; stale revisions dropped). Panels dock (PLUGIN-PANEL-DOCK,
  below); windows stay floating. UI ops are staged and applied only on commit.
- TOOL-GESTURES: toolbar strip above the main pill; primary press/move/release → begin (at the press
  point)/update/commit as one transaction; Escape (consumed before shortcuts) → cancel + rollback;
  document switch, revocation or grant change cancels. `canvas.overlay.set` draw lists (page units,
  bounded/finite-checked) draw on the canvas and clear at gesture end. Navigation is the canvas's own
  (PLUGIN-CANVAS-NAV, below).
- NATIVE: `native::Helper::start` with grants/log; `supervise()` every worker tick (100 ms).
- MANAGEMENT: Plugins window (`ManagerView`) from Plugins ▸ Manage plugins… and Preferences;
  catalogue/kill-list refresh at startup and every 6 h in the background (`refresh_in_background`,
  store `net` feature). CLI: `figcraft-cli plugin …` → `run_plugin`; `plugin contract` prints
  `contract().to_markdown()`; `plugin project FILE [--json]` prints statuses (no downloads).
- PROJECTS: `Document.plugins` (serde default, skipped when empty); a committed event records its
  plugin's PluginRef; `check_project` on document open → desktop ProjectPrompt.

- PLUGIN-PANEL-DOCK (done): plugin panels dock in the main window beside the sidebars
  (`ui-egui/src/plugin_dock.rs`, hosted from `app.rs`): one dock per side, between the built-in panel
  and the canvas, with a tab per shown panel (click shows it, × or middle-click hides it, right-click
  moves it to the other sidebar or hides it). Each dock resizes from its canvas edge like the built-in
  panels (double-click: default width), keeps the canvas its minimum width, and hides with the
  sidebars (Hide UI, minimized, narrow window). Plugins ▸ <panel title> shows or hides a panel. The
  layout (side, tab order, shown, active tab per side, widths) is the `plugin_panels` preference and
  decides each panel's open flag in the runtime; it keeps entries for panels whose plugin is disabled
  or removed (at most 256), so a re-enabled panel comes back on its side, in its place, shown if it was.
- PLUGIN-CANVAS-NAV (done): with a plugin tool selected the canvas runs its own navigation first
  (`canvas/input.rs` `navigate`: the same wheel/pinch zoom and scroll, Z zoom tool, middle/side-button
  and Space pans as the built-in tools); a press that navigation takes stays navigation until every
  button is up and never reaches the plugin; middle and side buttons never start a gesture. Gesture
  coordinates are page units from the view of the frame they are sent in, so a zoom or pan mid-gesture
  sends an `update` with the pointer's new page point; a release during a pan commits at the last point
  the plugin saw. Viewport commands (`ext::NAVIGATION_COMMANDS`: zoom in/out/100/50/200/fit/selection,
  zoomAt, pan, view.set/get) run while a gesture is open (edits are still refused), and a rolled-back
  gesture keeps the view it ended with (`Session::finish_group`).
- PLUGIN-FAILURE-RESET (done): the runtime records each plugin's last failure; `PluginRuntime::paused`
  lists plugins paused after `PAUSE_AFTER` (3) failures and `resume` clears the count, replaces each of
  the plugin's workers with a fresh instance of its stored program (in-memory state is lost) and sends
  `init` again. Session commands `plugins.paused` and `plugins.resume {id}` reach it from the UI,
  scripts (`figcraft-cli run --plugins`), automation and MCP. The Plugins window shows "Problems this
  session" (name, id, last error, **Resume**) inside the shared manager Problems tab. Pauses are per session and
  never saved; `figcraft-cli plugin resume <id>` says so and prints the script step. A store change
  (enable, disable, grant, update) also starts plugins unpaused, as before.
- CANVAS PATHS (verified): canvas-widget pointer events reach the guest as `ui` events with the
  widget id, `{kind: down|drag|up, at: [x, y]}` in the widget's own units and the tree revision they
  were drawn from; an event for an older revision or a closed panel is dropped. Fixed: a click sent only
  `up` and a drag began with `down` at the drag-start point (after egui's drag threshold) and a `drag`
  every frame; now a press sends `down` at the press point, moves send `drag`, release sends `up`.
  `canvas.overlay.set` lists replace the plugin's previous list, draw over the canvas mapped through
  the current view, clear at gesture end and are refused when out of range.

Remaining (named TODOs):

- RESOURCES: register fonts only for this app, activate craft-theme when available; investigate
  image-fill effect dispatch before advertising raster.filter.
- SCOPE: replace conservative document grants for selected-target get/update/delete with
  host-captured selection authorization (Begin already captures the selection); guest JSON cannot
  determine permission scope.
- CONTRACT-DOCS (done): the shared markdown is generated from the session 1.1 contract,
  including canonical engine argument descriptions, permission rules and adapter limits.
- LEGACY-REMOVAL: complete (old crate, SDK/examples, runtime/widget commands, directory transport,
  UI management/panels/palette/settings, xtask hooks). Retired collaboration tags keep their slots
  and refuse decoding; plugin_data stays opaque; unknown preferences round-trip.

## Activation validation

Scoped `cargo check` passed for figcraft-plugins, figcraft-cli, figcraft-ui-egui, figcraft (native)
and figcraft-ui-egui for wasm32 (`--features egui/web_file`). `cargo test -p figcraft-plugins` passed (1 test:
`plugin contract` output). No app build/run, workspace tests, clippy or xtask was run. xtask layers now lists
`plugins` at L6 with ui-egui → plugins allowed.

## Removal validation

Scoped native cargo checks passed for collab, engine, collab-net, ui-egui, CLI, server,
desktop and xtask. Edited test modules and the preservation/refusal fixtures compile; tests
were not executed. Scoped wasm checks passed for collab, collab-net, engine, ui-egui and the
web app. For the standalone UI wasm check, enable `--features egui/web_file`, matching the
feature supplied by eframe in the web app (the repository's combined wasm check supplies it).
No application, test, build, clippy or xtask command was run. The legacy symbol audit is clean;
ordinary egui control helpers remain. The shared adapter was not changed or activated.

## Runtime integrity follow-up

Plugin groups now hold live synchronization across every nested event and the whole tool gesture:
commit publishes one batch/history step; rollback publishes nothing. Commit revalidates the original
document, grant generation, contribution and release. Explicit completion results reach the dispatcher,
so a guest returning zero cannot hide a cancelled or rolled-back phase. Invalid or denied individual
operations remain recoverable; revocation, cancellation, stop and document changes abort the group.

Authorization reads fresh store snapshots on the document thread (including immediately after local
management writes), fails closed on unreadable state, and compares the saved setup generation.
Store-generation changes reconcile commands, panels and tools and replace their workers, including
release updates and rollbacks. Native helper supervision remains every 100 ms while idle. Catalogue
refresh repeats every six hours, with failed attempts retried after 60 seconds and exponential backoff
capped at 15 minutes.

Init can run on the home screen; document operations return a recoverable `no document` error.
Opening a document sends init again and the authorized `document.opened` event. Init failures do not
advance the three-failure pause. Read-only events retain document identity and history; edits record
missing plugin references once, carrying enabled contribution IDs and their state versions.
`Document.plugins` participates in collaboration diffs, snapshots, apply and server persistence;
`figcraft-cli plugin project FILE` already loads those references directly from the FigCraft file.

Follow-up validation: `cargo test -p figcraft-plugins` passed all 13 tests, including live group
publication/rollback, gesture completion and original-generation checks, recoverable invalid/denied
operations, no-document init, init failure accounting, store reconciliation, clean read-only history,
and collaboration/server persistence of references. Scoped native `cargo check` passed for
`figcraft-plugins`, `figcraft-cli` and `figcraft-ui-egui`; the UI check also passed for
`wasm32-unknown-unknown --features egui/web_file` (covering doc, collab and engine dependencies).
No workspace tests, application build/run, clippy or commits were performed; craft-extensions was
read-only. Existing unused-patch, missing optional fonts and wasm fontdb dead-code warnings remain.

## Form schema v1

Panels and windows render `Node::Form` using validated typed values merged with defaults.
All parameter kinds, sections, units, help, choice IDs, and host-evaluated visibility/enabled
conditions are supported. Edit state is scoped by surface, form and parameter; events carry
parameter keys and the surface tree revision. Unknown components render their fallback.
Color channels are numeric working-space RGBA controls, preserving HDR values.

`ui.panel.set` and `ui.window.open` accept `{panel|id, patch}` instead of `tree` for a root
form. Patches use the current surface tree revision as `base_rev`; `rev` must increase.
Validation uses `Form::apply_patch` on private staged state; invalid/stale patches publish
nothing. Hide/show and disable/enable preserve original conditions. Multiple staged patches
see preceding staged revisions. Full tree replacement clears patch flags.
FigCraft has no raster.filter dispatch: contribution effect forms and optional
craft_describe/craft_update exports remain unused until image-fill effects are integrated.

Form migration validation: scoped native checks passed for figcraft-plugins and
figcraft-ui-egui; wasm32 UI check passed with egui/web_file. The plugin test suite
passed all 14 tests, including atomic/stale form patches and hide/show restoration.
No application run, workspace tests, clippy, or commit was performed; visual behavior
has not been inspected in a running application.

## Runtime verification (real package)

The `figcraft-shapes` example (`craft-extensions/examples/figcraft-shapes`) was built to wasm, packed,
installed and run through this adapter by the ignored acceptance test `shapes_example_end_to_end`
(`FIGCRAFT_SHAPES_PACKAGE=…`; see [plugin-testing.md](plugin-testing.md)): panel form, one-step command,
recoverable revocation and mid-gesture rollback, tool commit/cancel with overlay, save/reopen statuses and
uninstall keeping layers. Fixed on the way: `selection.set` now maps to the engine's `select.set` (it
named a command that does not exist); `plugin project` prints the shared status (`ready`, `reason`, …)
instead of Rust debug text; the contract carries real summaries, arguments and results and the shared
page is regenerated from it. `figcraft-cli run --plugins` runs plugin commands headlessly.
Panels start closed; they open from their title in the Plugins menu.

## Dock, navigation and resume (PLUGIN-PANEL-DOCK, PLUGIN-CANVAS-NAV, PLUGIN-FAILURE-RESET)

Tests: `figcraft-plugins` (resume with a real wasm instance from `wat`, paused listing, CLI resume
text, canvas-widget events and stale revisions, overlay set/replace/clear/refuse), `figcraft-engine`
`ext_tests` (viewport commands mid-gesture, edits refused, view kept on rollback, session commands),
`figcraft-ui-egui` `plugin_host_tests` (window harness with a recording runtime: wheel zoom and
scroll, middle and Space drags, zoom shortcut never reach the plugin; gesture begins at the press
point and follows a zoom or pan mid-gesture; docking, disable/re-enable restore, menu toggle; layout
rules; canvas widget down/drag/up). Not verified in a running GUI (no display here): the dock's look,
the tab context menu and the Resume button.

## Shared renderer and capabilities (session 1.1)

Panels, docked panels, windows and forms use `craft_ext_ui::plugin_ui::PluginUi`;
canvas overlays use its `paint_ops` with the current page-to-screen transform.
The saved host preference `plugin_ui_engine` chooses `egui` (default) or `framework`
(currently falls back to egui). Plugins cannot select a renderer. UI 1.0 trees still work.
The adapter validates UI 1.1 trees and draw lists with the shared validators.

`ManagerView::show_with_app_problems` places paused plugins and Resume in Problems.
Only busy plugin gestures schedule periodic frames; installed commands and panels do not
keep an idle document repainting. Store changes reconcile on the next app frame.
The retired `plugins.runLast` key binding was removed along with its stale parity expectation.

The session contract now exposes an explicit allowlist of engine operations for pages,
components/instances, styles/variables, auto layout tracks, inline images, vector creation,
text and comments. `node.children` reads direct children; `export.svg` returns SVG text.
All document writes require document edit permission; reads require document read permission.
Engine argument validation is retained, with additional 256 KiB argument, depth 32 and
4096-element array limits. SVG exports accept 1–256 existing layers and return at most 4 MiB.
Image placement/replacement only accepts inline base64 data; filesystem paths are refused.
No file, library, network, shell or process command is forwarded.

`figcraft-grid` and `figcraft-placeholder` are FigCraft-only UI 1.1 examples. The grid
uses a typed form and layout tracks; placeholder copy uses a multiline editor and
updates selected text layers while preserving other layer types. Each Apply is one undo step.

### Capability audit

| Design feature | Plugin surface |
| --- | --- |
| Pages | list/add/rename/delete/duplicate/move/background/select |
| Frames and shapes | node.create/update/get/children/find/reparent/reorder, grouping, resize-to-fit, booleans |
| Text | text.create, node.update characters and text properties, text-on-path |
| Components and instances | create/set/variants, instance create/swap/properties, detach/reset |
| Styles and variables | list/create/apply/update/delete, collections/modes, bindings |
| Images | inline image.place/replace and image.adjust; no OS paths |
| Auto layout | node.update layout properties, layout.add/remove/setTrack |
| Vector paths | vector.create SVG path/points, node.update, flatten, outline stroke, booleans |
| Selection | selection.get/set, node.find; scope remains host-controlled |
| Viewport | viewport.get/set; no document permission required |
| Export | export.svg returns a bounded string; disk export remains unavailable |
| Comments | list/add/reply/resolve; engine text and target validation applies |
| Events | document.opened/changed, page.changed, selection.changed; app.events required |

The new `document.changed` notification is read-only to prevent edit-feedback loops. Existing
app-event behavior is preserved. Events are coalesced at the next pump;
`document.changed` covers document revision changes, including undo/redo. They do not carry
raw document data. The existing wasm host enforces permissions, per-event fuel/deadlines,
reply limits, rollback and grant-generation checks for the new operations too.
