# Plugin migration status

The shared adapter is active (`crates/plugins`, `figcraft_plugins::activate` from the desktop app's
startup, after startup files open). Done:

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
  slider/text edit state, tree revisions; stale revisions dropped); panel toggles in the Plugins menu.
  UI ops are staged and applied only on commit.
- TOOL-GESTURES: toolbar strip above the main pill; press/move/release → begin/update/commit as one
  transaction; Escape (consumed before shortcuts) → cancel + rollback; document switch, revocation
  or grant change cancels. `canvas.overlay.set` draw lists (page units, bounded/finite-checked) draw
  on the canvas and clear at gesture end.
- NATIVE: `native::Helper::start` with grants/log; `supervise()` every worker tick (100 ms).
- MANAGEMENT: Plugins window (`ManagerView`) from Plugins ▸ Manage plugins… and Preferences;
  catalogue/kill-list refresh at startup and every 6 h in the background (`refresh_in_background`,
  store `net` feature). CLI: `figcraft-cli plugin …` → `run_plugin`; `plugin contract` prints
  `contract().to_markdown()`; `plugin project FILE [--json]` prints statuses (no downloads).
- PROJECTS: `Document.plugins` (serde default, skipped when empty); a committed event records its
  plugin's PluginRef; `check_project` on document open → desktop ProjectPrompt.

Remaining (named TODOs):

- TODO(PLUGIN-CANVAS-NAV): while a plugin tool is selected the canvas skips its own input, so wheel
  zoom/pan need another tool; route navigation through.
- TODO(PLUGIN-PANEL-DOCK): panels are floating egui windows stacked at the canvas edge, not docked
  into the right sidebar.
- TODO(PLUGIN-FAILURE-RESET): the three-failure pause is per process; expose resume in the manager.
- RESOURCES: register fonts only for this app, activate craft-theme when available; investigate
  image-fill effect dispatch before advertising raster.filter.
- SCOPE: replace conservative document grants for selected-target get/update/delete with
  host-captured selection authorization (Begin already captures the selection); guest JSON cannot
  determine permission scope.
- CONTRACT-DOCS: regenerate the shared contract markdown; replace empty operation examples with
  validated request/result schemas.
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
