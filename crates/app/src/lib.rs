//! # forge3d-app
//!
//! The interactive forge3d modeler. Every edit made in the GUI is a
//! [`forge3d_core::CommandSpec`] executed on the shared
//! [`forge3d_core::document::Document`], so GUI, CLI and AI edits land in one
//! undo history.
//!
//! * [`state`] – the winit/wgpu/egui independent state machine ([`state::AppState`]),
//! * [`gizmo`] – gizmo geometry, hit testing and drag math,
//! * [`camera_input`] – mouse/keyboard to orbit-camera mapping,
//! * [`actions`] – UI choices to command specs,
//! * [`target`] – what the gizmo edits and the commands a drag produces,
//! * [`ui`], [`gui`], [`app`] – egui panels, GPU glue and the event loop.
//!
//! Embedding: [`run`] with [`AppOptions`]; a `frame_hook` runs on the main
//! thread once per event-loop iteration and a [`Waker`] wakes the loop from
//! other threads.

pub mod actions;
pub mod app;
pub mod camera_input;
pub mod error;
pub mod gizmo;
pub mod gui;
pub mod script;
pub mod state;
pub mod target;
pub mod ui;

pub use app::{run, AppOptions, FrameHook, ScreenshotRequest, Waker};
pub use error::AppError;
