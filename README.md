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
