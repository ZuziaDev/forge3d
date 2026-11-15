//! # forge3d-api
//!
//! The AI connection layer of forge3d. Every operation goes through the
//! core command API; this crate makes it reachable for AI agents (MCP), CLI
//! scripts and — via [`host::DocumentHandle`] — background work in the GUI.
//!
//! * [`host`] – single-writer document hosting: [`DocumentHandle`] submits
//!   jobs to the thread that owns the [`forge3d_core::document::Document`]
//!   (headless [`DocumentHost::spawn`] or embedded [`DocumentHandle::channel`]
//!   drained by the GUI each frame),
//! * [`tools`] – transport-independent tool registry [`Forge3dTools`]
//!   (`list` / `call(name, json)`), typed parameters with JSON Schema,
//! * [`mcp`] – MCP server on top of the official `rmcp` SDK: stdio and
//!   Streamable HTTP (bearer token, Origin/Host validation),
//! * [`script`] – JSON scripts (tool calls and raw CommandSpecs) for headless runs,
//! * [`resources`] – `forge3d://scene`, `forge3d://object/{id}`, `forge3d://history`,
//! * [`workspace`] – confinement of file paths to a workspace directory,
//! * [`error`] – [`ApiError`] `{code, message, hint, details}`.
//!
//! ```
//! use forge3d_api::{DocumentHost, Forge3dTools, Workspace};
//! use forge3d_core::prelude::*;
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let dir = std::env::temp_dir();
//! let tools = Forge3dTools::new(
//!     DocumentHost::spawn(Document::new()),
//!     Workspace::new(&dir).unwrap(),
//!     Origin::Ai,
//! );
//! let out = tools
//!     .call("object_create", json!({"primitive": {"type": "cube"}, "name": "Box"}))
//!     .await
//!     .unwrap();
//! assert_eq!(out["object"]["name"], "Box");
//! assert_eq!(out["history_entry"]["label"], "AI: Create cube 'Box'");
//! # });
//! ```

#![forbid(unsafe_code)]

pub mod error;
pub mod host;
pub mod info;
pub mod mcp;
pub mod resources;
pub mod script;
pub mod tools;
pub mod workspace;

pub use error::{ApiError, ApiResult};
pub use host::{DocumentHandle, DocumentHost, DocumentJob, DocumentJobs, Waker};
pub use mcp::{serve_io, serve_stdio, start_http, Forge3dMcpServer, HttpConfig, RunningHttpServer};
pub use tools::{tool_defs, Forge3dTools, ToolDef, ToolOutput};
pub use workspace::{PathAccess, Workspace};

/// The `rmcp` SDK version this crate is built against.
pub const RMCP_VERSION: &str = "3.10.0";
