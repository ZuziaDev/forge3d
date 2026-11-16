# Changelog

All notable changes to forge3d are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.1] - 2026-11-16

### Added
- **forge3d by ZuziaDev branding**: new logo (`assets/logo.svg`, `logo.png`, multi-size `icon.ico`), used as the window icon, the Windows exe icon with embedded version information, the installer artwork and the README header.
- **Blender-style keymap** in the GUI: `G` / `R` / `S` modal move, rotate and scale with `X` / `Y` / `Z` axis lock (`Shift` + axis locks a plane), typed numeric values, `Enter` or left click to confirm, `Esc` or right click to cancel, one undo entry per operation.
- `Shift+A` add menu and `X` delete menu at the cursor, `Shift+D` duplicate and move, `Tab` edit mode, `1` / `2` / `3` vertex, edge and face mode, `A` / `Alt+A` / `Ctrl+I` select all, none and invert, `H` / `Alt+H` hide and show, `Z` wireframe, numpad `1` / `3` / `7` / `5` / `.` views (`Ctrl` for the opposite side).
- Modal mesh tools in edit mode: `E` extrude along the normals, `I` inset, `Ctrl+B` bevel, `Ctrl+R` loop cut, `P` separate, and `Ctrl+J` join objects.
- `F3` command search palette: fuzzy search over every action, with its shortcut shown.
- Native file dialogs (Windows and macOS) for open, save, import and export, with the typed-path window as fallback. `Ctrl+O` / `Ctrl+S` / `Ctrl+Shift+S` / `Ctrl+N`.
- Unsaved-changes confirmation on close, new and open; the window title shows `*` while the scene is dirty.
- Modifiers tab in the properties panel: add, remove, reorder, enable or disable, edit parameters and apply (mirror, array, subdivision, solidify, bevel, triangulate), plus per-object smooth shading with an auto-smooth angle.
- New mesh operations as shared commands, available in the Mesh menu and over MCP: solidify, bridge edge loops, fill holes, dissolve, triangulate, triangles to quads, poke faces, smooth vertices, separate and join objects, and bevel with multiple segments.
- Modifier commands (`add_modifier`, `remove_modifier`, `set_modifier`, `move_modifier`, `apply_modifier`) and `set_shading`, with MCP tools `modifier_edit`, `object_shading`, `object_join` and `mesh_measure`, so AI assistants use the same stack as the GUI.
- Release pipeline: CI workflow (format, clippy, tests), and a release workflow that builds the installer on every `v*` tag and publishes the setup exe, the standalone exe and SHA256 checksums with notes generated from this file.

### Changed
- Blender-like dark theme with an orange accent, rounder widgets and tooltips that show shortcuts.
- The Windows installer is branded (publisher "ZuziaDev", icon, welcome and header artwork, version resource).
- Starting the GUI by double-click no longer leaves a console window behind; `forge3d mcp` keeps its stdio and a GUI started from a terminal keeps that terminal.
- The viewport shortcut `1`-`4` for object, vertex, edge and face mode is now `Tab` and `1`-`3` (Blender layout); `W` / `E` / `F` / `O` no longer switch tools or views.

## [0.1.0] - 2026-11-16

### Added
- `forge3d-core`: half-edge mesh, primitives, mesh operations (extrude, inset, bevel, loop cut, Catmull-Clark subdivision, merge, delete, mirror, boolean, normals), scene graph, selection and the undoable, serializable `CommandSpec` command language with JSON Schema.
- `forge3d-io`: glTF / GLB, OBJ and STL import and export, and the `.f3d` project format.
- `forge3d-render`: wgpu viewport renderer, ID-buffer picking and a CPU software rasterizer for headless renders.
- `forge3d-api` and the `forge3d` CLI: MCP server over stdio and Streamable HTTP with 21 tools and resources, headless script runner and file conversion.
- `forge3d-app`: interactive modeler (winit, egui, wgpu) in which every edit is a command on the shared document, with a live MCP server so GUI and AI edits share one undo history.
- Windows installer built with NSIS.
