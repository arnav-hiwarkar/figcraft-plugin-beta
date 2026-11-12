# Remaining plugin migration work

- DOCUMENT-TRANSACTIONS: service Bridge requests on the document thread using FigCraft history;
  capture authoritative selection in begin, validate targets there, implement all listed ops,
  and roll back timed-out requests even when Finish cannot be queued. Block other editing while
  a transaction is open. Make a completed event one labelled undo step.
- SESSION-REGISTRY: load enabled contributions from Manager/GrantView, key workers by plugin/module,
  construct Invocation including declared permissions, and route command/init/dispose events.
- PANEL-UI: validate core UI trees, draw dockable egui panels and named floating windows, route
  events with tree revisions, and stage UI changes until event commit.
- TOOL-GESTURES: toolbar, pointer events, Escape cancellation, document-switch cancellation,
  overlays with bounded draw lists, and one transaction for the whole gesture.
- NATIVE: start Helper with plugin/version/executable/work/api/grants/log; own its lifecycle.
- PROJECTS: serde-default PluginRef list in the file manifest; check_project on open; desktop
  ProjectPrompt and CLI statuses; preserve opaque legacy widget/plugin state and previews.
- MANAGEMENT: shared ManagerView in menu/settings and add CLI run_plugin;
  `plugin contract` must render contract().to_markdown(). No plugin downloads on project open.
- RESOURCES: register fonts only for this app, activate craft-theme when available; investigate
  image-fill effect dispatch before advertising raster.filter.
- LEGACY-REMOVAL: complete. Removed the old crate, SDK/examples, runtime/widget commands,
  directory transport, UI management/panels/palette/settings and xtask hooks. Retired collaboration
  tags retain their postcard enum slots and JSON names, but refuse decoding with a clear removal
  error. Existing document plugin_data remains opaque and serializes unchanged; saved widget
  layers/previews remain ordinary layers. Unknown preferences round-trip through serde flatten.
  The shared adapter remains inactive. No automatic downloads or activation occur on open.
- SCOPE: replace conservative document grants for selected-target get/update/delete with
  host-captured selection authorization; guest JSON cannot determine permission scope.
- SAFE-MODE: apply FIGCRAFT_SAFE_MODE=1 to startup and discovery before activation.
- CONTRACT-DOCS: regenerate the shared contract markdown from the final adapter; replace empty
  operation examples with validated request/result schemas after operation dispatch is implemented.

## Removal validation

Scoped native cargo checks passed for collab, engine, collab-net, ui-egui, CLI, server,
desktop and xtask. Edited test modules and the preservation/refusal fixtures compile; tests
were not executed. Scoped wasm checks passed for collab, collab-net, engine, ui-egui and the
web app. For the standalone UI wasm check, enable `--features egui/web_file`, matching the
feature supplied by eframe in the web app (the repository's combined wasm check supplies it).
No application, test, build, clippy or xtask command was run. The legacy symbol audit is clean;
ordinary egui control helpers remain. The shared adapter was not changed or activated.
