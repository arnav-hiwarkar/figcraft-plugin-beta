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
