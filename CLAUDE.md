# forge3d — agent rules

## Token budget (user is on a limited plan)
- Replies: shortest possible. No narration, no recaps, no option lists. "Fixed X. Pushed." style.
- Subagents: spawn only when asked or for large independent work. Pass these rules on. Final reports ≤15 lines.
- Read only the files/lines needed (Grep first, offset/limit). Never re-read known files.
- Filter command output (`| tail`, `| grep -E "test result|error"`). No full logs.
- Build/test only affected crates (`-p <crate>`); full workspace run once before commit.
- Prefer Sonnet for routine work; Opus only for hard algorithmic tasks.

## Project
- Workspace: crates/{core,io,render,app,api,cli}. All mutations go through `forge3d_core::CommandSpec` on `Document`.
- core: no unsafe, no GPU/UI deps. anyhow only in binaries.
- Gate: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`.
- No TODOs/placeholders. Errors carry `{code, message, hint}`.
- Windows installer: `packaging/windows/build.sh`.
