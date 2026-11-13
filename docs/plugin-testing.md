# Plugin beta testing

FigCraft is the session-first app: its plugins are panels, commands and pointer tools that
create ordinary layers. Use the local signed catalogue that
`../craft-extensions/scripts/e2e.sh` generates in `../test-catalogue/`, and the example plugin
`../craft-extensions/examples/figcraft-shapes` (a form panel, a command, a drag tool with a guide
overlay). Wait for `e2e.sh` to finish before refreshing; it republishes the catalogue while it runs.
All builds are debug builds and take the shared resource lock.

## 1. Build and install

From the FigCraft repository:

```sh
LOCK=/home/ash/projects/plugin-beta/.cargo.lock
flock $LOCK env CARGO_BUILD_JOBS=4 cargo build -p figcraft-cli
CE=../craft-extensions
# craft-ext (author tool); skip if $CE/target/debug/craft-ext exists
(cd $CE && flock $LOCK env CARGO_BUILD_JOBS=4 cargo build -p craft-ext-cli)
(cd $CE/examples/figcraft-shapes && flock $LOCK env CARGO_BUILD_JOBS=4 ../../target/debug/craft-ext build \
  && ../../target/debug/craft-ext check && ../../target/debug/craft-ext pack --out figcraft-shapes.craftplugin)

export CRAFT_EXT_DIR="$(mktemp -d /tmp/figcraft-plugin-beta.XXXXXX)"
export CRAFT_CATALOGUE_URL=file:///home/ash/projects/plugin-beta/test-catalogue/
export CRAFT_CATALOGUE_ALLOW_FILE=1
CLI=./target/debug/figcraft-cli
ID=io.github.arnav-hiwarkar.figcraft-shapes
$CLI plugin refresh
$CLI plugin search shapes
$CLI plugin install io.github.arnav-hiwarkar.color-notes   # refused: it targets EffectCraft only
$CLI plugin install io.github.arnav-hiwarkar.halftone      # refused: FigCraft offers no raster.filter
$CLI plugin install-file $CE/examples/figcraft-shapes/figcraft-shapes.craftplugin   # or: plugin install $ID
$CLI plugin permissions $ID
$CLI plugin setup $ID --all                     # refused: "Permission needed. Fix: plugin grant …"
$CLI plugin setup $ID --all --grant document.edit:document --grant app.events
```

Installing never enables anything or grants permissions; `app.events` is optional. The test catalogue
also lists `figcraft-shapes` (archive in `../test-catalogue/assets/`), so `plugin install $ID` works too.

## 2. Headless acceptance test (real wasm through the adapter)

```sh
FIGCRAFT_SHAPES_PACKAGE=$CE/examples/figcraft-shapes/figcraft-shapes.craftplugin \
  flock $LOCK env CARGO_BUILD_JOBS=4 cargo test -p figcraft-plugins --lib -- --include-ignored --nocapture
```

It installs the package into its own disposable store (your `CRAFT_EXT_DIR` is not touched) and
checks, in one session:

- init draws the Shapes panel: a column whose first child is a `Node::Form` (shape, size, count, colour);
- form edits reach the guest; **Create shapes** (command and panel button) makes the row as **one undo
  step** labelled “Create shapes · Shapes”, records the plugin reference, and undo/redo work;
- revoking `document.edit:document` mid-session: the running guest gets a recoverable denial and says so
  in its panel (no layers, no undo step); the next frame unloads the contributions; granting again
  recovers without a restart; a revoke **during a tool gesture** cancels it and rolls back every phase;
- Draw rectangle: begin/update/commit is one undo step, the guide overlay (4 draw ops) shows during the
  drag and clears at the end; cancel (Escape) rolls the gesture back;
- save with the plugin list → reopen → statuses Ready; uninstall → reopen keeps every layer and the
  project prompt reports the plugin as not ready.

## 3. CLI session path, project statuses and contract

```sh
echo "[{\"command\":\"plugin.$ID.create-shapes\",\"params\":{}}]" > /tmp/shapes.json
$CLI run /tmp/shapes.json --out /tmp/shapes.figcraft            # fails: unknown command (no --plugins)
$CLI run /tmp/shapes.json --plugins --out /tmp/shapes.figcraft  # 3 shapes, one undo step
$CLI info /tmp/shapes.figcraft
$CLI plugin project /tmp/shapes.figcraft      # "status": "ready"
$CLI plugin remove $ID
$CLI plugin project /tmp/shapes.figcraft      # "unavailable" (file install) or "downloadable" (catalogue install)
$CLI info /tmp/shapes.figcraft                # same layers: plugin layers are ordinary layers
$CLI plugin install-file $CE/examples/figcraft-shapes/figcraft-shapes.craftplugin
$CLI plugin setup $ID --all --grant document.edit:document --grant app.events
$CLI plugin contract > /tmp/figcraft-contract.md
diff -u $CE/docs/plugins/contracts/figcraft.md /tmp/figcraft-contract.md   # no output
```

The shared contract page is generated: regenerate it with
`$CLI plugin contract > $CE/docs/plugins/contracts/figcraft.md` after changing `crates/plugins/src/contract.rs`.

## 4. Desktop (manual GUI)

```sh
flock $LOCK env CARGO_BUILD_JOBS=4 cargo build -p figcraft
./target/debug/figcraft          # same shell, so it uses the store and catalogue above
```

1. **Plugins ▸ Manage plugins…** opens the Plugins window (Browse, Installed, Updates, Problems). In
   Browse search “shapes”, **Install 1.0.0**, then in **Set up Shapes** enable the three
   contributions and allow “Change the current document” (and optionally app events). Skip this if
   you used the CLI setup above.
2. New design file. **Plugins ▸ Shapes** (the panel toggle at the top of the menu) docks the Shapes
   panel beside the right sidebar as a tab. Drag its canvas-side edge to resize it; right-click the tab
   ▸ **Move to left sidebar**; quit and restart: it is still there, at that width. Click × to hide it. Change Shape, Size, Count and Colour, click **Create shapes**: a row of layers appears.
   **Edit ▸ Undo** once removes the whole row; Redo brings it back. With app events allowed, the panel
   shows “Last app event: selection.changed” when you click a layer.
3. **Plugins ▸ Shapes ▸ Create shapes** runs the same command from the menu (also in the Actions palette).
4. The plugin tool strip above the toolbar shows **D** (Draw rectangle). Click it, press-drag on the
   canvas: a live rectangle, a pink outline, cross-hair guides and a size label follow the pointer.
   Release to keep it (one undo step). Drag again and press **Escape** before releasing: the rectangle
   and guide disappear and History is unchanged. With the tool still selected: the wheel, a pinch,
   Cmd/Ctrl+wheel, middle-drag, Space-drag, Z-click and Shift+0/Shift+1/Cmd+= zoom and pan as with
   the Move tool and draw nothing; zoom with the wheel or Cmd+= in the middle of a drag: the rectangle
   follows the pointer's new page position.
5. With the panel open, in the Plugins window **Installed ▸ Shapes** revoke “Change the current
   document”: the Shapes panel, menu items and tool disappear on the next frame; a gesture in progress
   is cancelled with a “revoked” toast. Allow it again: they come back.
6. Disable Shapes in the Plugins window: its docked panel disappears and the canvas widens. Enable it
   again: the panel comes back on the same side. (Pausing needs a plugin that fails three times; the
   Plugins window then lists it under "Problems this session" with **Resume**. Headless:
   `figcraft-cli plugin resume <id>` explains that pauses last one session, and a `run --plugins` script
   can contain `{"command":"plugins.resume","params":{"id":"<id>"}}`.)
7. **File ▸ Save as…** a `.figcraft`, close it, remove Shapes in the Plugins window, reopen the file:
   the project prompt lists Shapes as missing; **Open without them** opens the file with every layer intact.

The renderer stays egui: `figcraft_ui_egui::plugins_ui::render_surface` is the single function to
swap for craft-framework's form renderer later.

## Verified run (2026-11-13, Linux x86_64, debug)

- `cargo test -p figcraft-plugins --lib -- --include-ignored`: 16 passed, including
  `shapes_example_end_to_end` with the real wasm package; `cargo test -p craft-ext-cli --lib`: 11 passed;
  figcraft-cli `files::` (13) and `args::` (9) tests passed.
- CLI refresh/search/install (both refusals as documented), install-file, setup (refused without the
  grant), `run --plugins` (3 and 6 shapes saved), `plugin project` ready → unavailable/downloadable,
  layers kept after uninstall; a catalogue install of `figcraft-shapes` from the regenerated test
  catalogue (serial 11); the contract matches the shared page byte for byte.
- The desktop binary was built once (10 min), `target/debug/figcraft`. No GUI was launched: the clicks in
  section 4 are for the beta tester.
