# Shared Craft plugin migration — incomplete

The new `figcraft-plugins` crate is an adapter foundation, not an activated replacement.
The existing runtime remains active. Removing it before document/UI callers migrate would break
existing files and commands. No legacy plugin or widget data has been deleted or rewritten.

Implemented: shared workspace dependencies (beta sibling paths), declarative session contract,
bounded per-module WASM worker, document-thread request/reply bridge with five-second timeout
and cancellation, live shared-store grants that fail closed, and safe-mode checks for the new host.
The shared Session checks contribution declarations and grants and calls begin/finish per event.
The bridge itself does not implement FigCraft history: its receiver must do so before activation.

Named integration TODOs:

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
- MANAGEMENT: shared ManagerView in menu/settings and replace CLI publish with run_plugin;
  `plugin contract` must render contract().to_markdown(). No plugin downloads on project open.
- RESOURCES: register fonts only for this app, activate craft-theme when available; investigate
  image-fill effect dispatch before advertising raster.filter.
- LEGACY-REMOVAL: delete crates/plugin, top-level plugins, engine runtime/widgets/directory,
  server distribution routes, collab distribution messages, obsolete tests/docs and xtask hooks
  only after migrating every caller. Keep document-owned opaque data and placeholders.
- SCOPE: replace conservative document grants for selected-target get/update/delete with
  host-captured selection authorization; guest JSON cannot determine permission scope.
- SAFE-MODE: apply FIGCRAFT_SAFE_MODE=1 to startup, discovery and legacy runtime before activation.
- CONTRACT-DOCS: regenerate the shared contract markdown from the final adapter; replace empty
  operation examples with validated request/result schemas after operation dispatch is implemented.

Dependency note: the new store dependency has its `net` feature disabled and introduces no
ureq TLS features. The pre-existing workspace lockfile already contains ureq → webpki-roots
through other dependencies; resolving that existing graph remains pending.

Verification is limited to scoped cargo check commands. No runtime, tests, build, clippy or CI
was requested or run. This foundation must not be presented as a completed migration.
