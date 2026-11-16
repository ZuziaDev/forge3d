# forge3d

A simple, Blender-like 3D polygon modeler written in Rust, built around one
principle: **every operation goes through a single command API** that is
shared by the GUI, the CLI and AI agents (via an MCP server).

## Workspace

| crate | status | purpose |
|-------|--------|---------|
| `crates/core` (`forge3d-core`) | stage 1 | half-edge mesh, primitives, mesh operations (extrude, inset, bevel, loop cut, Catmull–Clark, merge, delete, mirror, boolean, normals), scene graph, selection, undoable command system and the serializable `CommandSpec` language with JSON Schema |
| io, render, app, api, cli | planned | file formats, rendering, GUI, MCP server, command line |

`forge3d-core` is platform independent (no GPU/UI dependencies, no unsafe code).

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
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Licensed under MIT OR Apache-2.0.

## Usage

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
`object_duplicate`, `object_rename`, `object_parent`, `mesh_edit`, `mesh_select`,
`mesh_get`, `boolean_op`, `material_set`, `undo`, `redo`, `history_list`,
`viewport_render`, `file_import`, `file_export`, `project_save`, `project_load`, `batch`.
