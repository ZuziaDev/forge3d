<p align="center"><img src="assets/logo.png" width="120" alt="forge3d logo"></p>

<h1 align="center">forge3d</h1>
<p align="center"><b>by ZuziaDev</b> &middot; a Blender-style 3D modeler in Rust with a built-in MCP server</p>

forge3d is built around one principle: **every operation goes through a single
command API** (`CommandSpec`) shared by the GUI, the CLI and AI agents. You and
your assistant edit the same scene, and every change lands in one undo history.

## Features

- Polygon modeling: extrude, inset, bevel (segments), loop cut, bridge, fill, dissolve, solidify, subdivide, mirror, boolean, smooth, triangulate, separate and join.
- Non-destructive modifier stack (mirror, array, subdivision, solidify, bevel, triangulate) with per-object smooth shading and auto-smooth.
- Blender-style GUI: modal `G` / `R` / `S` with axis lock and typed values, edit modes, `Shift+A` add menu, `F3` command search, native file dialogs.
- Built-in MCP server (stdio and Streamable HTTP): Claude can model in the live window, and its edits appear as `AI: ...` history entries you can undo.
- glTF / GLB, OBJ and STL import and export, `.f3d` project files, headless script runner and renderer.

## Install

**Windows**: download `forge3d-<version>-setup.exe` from the [latest release](https://github.com/ZuziaDev/forge3d/releases/latest) (per-user install, no admin rights), or the standalone `forge3d-<version>-windows-x64.exe`. Builds are not code-signed, so SmartScreen may ask you to confirm.

**From source** (Rust stable): `cargo build --release`, binary at `target/release/forge3d`. Windows installer from Linux: `packaging/windows/build.sh` (needs mingw-w64 and nsis).

## Shortcuts

| key | action | key | action |
|-----|--------|-----|--------|
| `G` / `R` / `S` | move / rotate / scale | `X` `Y` `Z` | lock axis (`Shift` = plane) |
| digits, `-`, `.` | type a value | `Enter` / `LMB` | confirm |
| `Esc` / `RMB` | cancel | `Ctrl` while dragging | snap |
| `Shift+A` | add menu | `X` / `Delete` | delete menu / delete |
| `Shift+D` | duplicate and move | `Ctrl+J` | join objects |
| `Tab` | edit mode | `1` `2` `3` | vertex / edge / face |
| `E` | extrude | `I` | inset |
| `Ctrl+B` | bevel | `Ctrl+R` | loop cut |
| `P` | separate | `A` / `Alt+A` / `Ctrl+I` | select all / none / invert |
| `H` / `Alt+H` | hide / show | `Z` | wireframe |
| numpad `1` `3` `7` (+`Ctrl`) | front / right / top (back / left / bottom) | numpad `5` / `.` | ortho / frame selected |
| `F3` | command search | `Ctrl+Z` / `Ctrl+Shift+Z` | undo / redo |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` | new / open / save | `Ctrl+Shift+S` | save as |

## Workspace

| crate | purpose |
|-------|---------|
| `forge3d-core` | half-edge mesh, primitives, mesh operations, modifiers, scene graph, selection, undoable `CommandSpec` language with JSON Schema (no GPU/UI deps, no unsafe) |
| `forge3d-io` | glTF / GLB, OBJ, STL and `.f3d` project files |
| `forge3d-render` | wgpu viewport renderer, ID-buffer picking, CPU software rasterizer |
| `forge3d-app` | the winit + egui + wgpu GUI |
| `forge3d-api` | MCP server (tools, resources) |
| `forge3d-cli` | the `forge3d` binary: GUI, MCP server, runner, converter |

## Quick example

```rust
use forge3d_core::prelude::*;

let mut doc = Document::new();
doc.execute_json(r#"{"op":"create_primitive","primitive":{"type":"cube"}}"#, Origin::Ai)?;
doc.execute_json(r#"{"op":"set_select_mode","mode":"face"}"#, Origin::Ai)?;
doc.execute_json(r#"{"op":"select_by_direction","direction":[0,0,1]}"#, Origin::Ai)?;
let summary = doc.execute_json(r#"{"op":"extrude","distance":1.0}"#, Origin::Ai)?;
// history label: "AI: Extrude 1 face on 'Cube'"
doc.undo()?;
```

`CommandSpec::json_schema()` returns the JSON Schema of the full command
language; errors carry a stable `code()` and a corrective `hint()`.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

See [CHANGELOG.md](CHANGELOG.md) for what changed. Licensed under MIT OR Apache-2.0.

## Usage and MCP setup

```sh
cargo build --release                  # binary: target/release/forge3d
forge3d gui                            # window + live MCP server on 127.0.0.1:7878/mcp
forge3d gui --project a.f3d --workspace ~/models --token SECRET   # --no-mcp, --mcp-stdio, --mcp-http ADDR
forge3d mcp [--workspace DIR]          # headless MCP over stdio (Claude Desktop / Claude Code)
forge3d serve --addr 127.0.0.1:7878    # headless MCP over Streamable HTTP (--token / FORGE3D_TOKEN)
forge3d run examples/table.json --export out.glb --render out.png
forge3d tools                          # tool list with JSON schemas
forge3d export in.obj out.glb          # convert between f3d/glb/gltf/obj/stl
```

In `forge3d gui` the window owns the document; AI edits arrive over MCP, show up
immediately in the viewport and appear as `AI: ...` entries in the History panel
(undo/redo work on them). If the port is busy the GUI still starts and logs the error.
Build without the window: `cargo build -p forge3d-cli --no-default-features`.

Claude Desktop (`claude_desktop_config.json`), headless stdio server:

```json
{
  "mcpServers": {
    "forge3d": {
      "command": "/path/to/forge3d",
      "args": ["mcp", "--workspace", "/path/to/models"]
    }
  }
}
```

To drive a running GUI instead, add it as an HTTP server:
`claude mcp add --transport http forge3d http://127.0.0.1:7878/mcp`.

Tools: `scene_get`, `object_create`, `object_transform`, `object_delete`,
`object_duplicate`, `object_rename`, `object_parent`, `object_join`, `object_shading`,
`modifier_edit`, `mesh_edit`, `mesh_select`, `mesh_get`, `mesh_measure`, `boolean_op`,
`material_set`, `undo`, `redo`, `history_list`, `viewport_render`, `file_import`,
`file_export`, `project_save`, `project_load`, `batch`.
