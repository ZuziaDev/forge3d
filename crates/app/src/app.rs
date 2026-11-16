//! The winit event loop around [`Gui`].

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge3d_core::document::Document;
use forge3d_render::wgpu;
use forge3d_render::ViewPreset;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Icon, Window, WindowAttributes, WindowId};

use crate::error::AppError;
use crate::gizmo::GizmoMode;
use crate::gui::{save_png, Gui, OffscreenTarget};
use crate::keymap;
use crate::script::{run_script, DEMO_SCRIPT};

/// Hook called once per event-loop iteration, before drawing, on the main
/// thread. It may run commands on the document; return `true` if it changed
/// anything so the viewport is redrawn.
pub type FrameHook = Box<dyn FnMut(&mut Document) -> bool>;

/// A thread-safe handle that wakes the event loop (and so runs the frame
/// hook and redraws) from any thread.
#[derive(Clone)]
pub struct Waker {
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Waker {
    /// Wrap an arbitrary wake function.
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Waker {
        Waker { wake: Arc::new(f) }
    }

    /// Wake the GUI event loop. Cheap, never blocks, harmless after the GUI
    /// has exited.
    pub fn wake(&self) {
        (self.wake)();
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

/// Render the first window frames to a PNG and exit.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenshotRequest {
    pub path: std::path::PathBuf,
    /// Number of UI frames to draw before capturing (layout and animations settle).
    pub after_frames: u32,
    /// Render without any window or display server.
    pub headless: bool,
}

/// Everything [`run`] needs.
pub struct AppOptions {
    /// The document to edit. The GUI becomes its only writer (besides `frame_hook`).
    pub document: Document,
    /// Window title.
    pub title: String,
    /// See [`FrameHook`].
    pub frame_hook: Option<FrameHook>,
    /// Receives a [`Waker`] before the window is created.
    pub on_waker: Option<Box<dyn FnOnce(Waker)>>,
    /// Inner window size in logical points.
    pub window_size: (u32, u32),
    /// Project file the document belongs to (target of Save).
    pub project_path: Option<std::path::PathBuf>,
    /// Startup script (see [`crate::script`]) executed on the document before the window opens.
    pub script: Option<String>,
    /// Add the demo scene before the script.
    pub demo: bool,
    /// Initial view preset.
    pub view: Option<ViewPreset>,
    /// Initial gizmo tool.
    pub tool: GizmoMode,
    /// Start with an orthographic camera.
    pub orthographic: bool,
    /// Frame the selection instead of the whole scene at startup.
    pub frame_selected: bool,
    /// Capture a screenshot and exit.
    pub screenshot: Option<ScreenshotRequest>,
    /// Window size / HiDPI scale of headless screenshots (physical pixels, scale).
    pub headless_size: (u32, u32),
    pub pixels_per_point: f32,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            document: Document::new(),
            title: "forge3d".into(),
            frame_hook: None,
            on_waker: None,
            window_size: (1400, 900),
            project_path: None,
            script: None,
            demo: false,
            view: None,
            tool: GizmoMode::Translate,
            orthographic: false,
            frame_selected: false,
            screenshot: None,
            headless_size: (1400, 900),
            pixels_per_point: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum UserEvent {
    Wake,
}

/// Run the application until the window is closed (or the screenshot is taken).
pub fn run(mut options: AppOptions) -> Result<(), AppError> {
    // Build the startup scene before any window exists.
    if options.demo {
        run_script(&mut options.document, DEMO_SCRIPT)?;
    }
    if let Some(script) = options.script.take() {
        run_script(&mut options.document, &script)?;
    }
    if options.screenshot.as_ref().is_some_and(|s| s.headless) {
        return run_headless(options);
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| AppError::Window(e.to_string()))?;
    if let Some(cb) = options.on_waker.take() {
        let proxy: Arc<Mutex<EventLoopProxy<UserEvent>>> =
            Arc::new(Mutex::new(event_loop.create_proxy()));
        cb(Waker::new(move || {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(UserEvent::Wake);
            }
        }));
    }
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new(options);
    event_loop
        .run_app(&mut app)
        .map_err(|e| AppError::Window(e.to_string()))?;
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn pick_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, AppError> {
    let mut last = String::new();
    for fallback in [false, true] {
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface: surface,
            ..Default::default()
        })) {
            Ok(a) => return Ok(a),
            Err(e) => last = e.to_string(),
        }
    }
    Err(AppError::Graphics(format!(
        "no usable GPU adapter ({last})"
    )))
}

fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), AppError> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("forge3d device"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .map_err(|e| AppError::Graphics(e.to_string()))
}

fn describe(adapter: &wgpu::Adapter) -> String {
    let i = adapter.get_info();
    format!("{} ({:?}, {:?})", i.name, i.device_type, i.backend)
}

fn apply_startup(gui: &mut Gui, options: &AppOptions) {
    let st = &mut gui.state;
    st.tool = options.tool;
    st.path = options.project_path.clone();
    st.camera.orthographic = options.orthographic;
    st.pending_frame_selected = options.frame_selected;
    if let Some(v) = options.view {
        crate::camera_input::apply_preset(&mut st.camera, v);
    }
}

fn run_headless(mut options: AppOptions) -> Result<(), AppError> {
    let Some(shot) = options.screenshot.clone() else {
        return Ok(());
    };
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pick_adapter(&instance, None)?;
    let (device, queue) = request_device(&adapter)?;
    let info = describe(&adapter);
    let size = [options.headless_size.0, options.headless_size.1];
    let doc = std::mem::take(&mut options.document);
    let mut gui = Gui::new(device, queue, wgpu::TextureFormat::Rgba8Unorm, doc, info)?;
    apply_startup(&mut gui, &options);
    gui.state.viewport = (size[0].max(2) / 2, size[1].max(2) / 2);
    let rgba = gui.render_offscreen(size, options.pixels_per_point, shot.after_frames)?;
    save_png(&shot.path, &rgba, size)
}

struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

struct App {
    options: AppOptions,
    hook: Option<FrameHook>,
    window: Option<Arc<Window>>,
    surface: Option<Surface>,
    gui: Option<Gui>,
    egui_state: Option<egui_winit::State>,
    instance: Option<wgpu::Instance>,
    next_redraw: Option<Instant>,
    frames: u32,
    title: String,
    error: Option<AppError>,
    modifiers: ModifiersState,
}

/// The forge3d logo (512 px PNG) shrunk to a 64 px window icon.
pub fn window_icon() -> Option<Icon> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(include_bytes!(
        "../../../assets/logo.png"
    )));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.color_type.samples();
    if channels != 4 || info.bit_depth != png::BitDepth::Eight || w % 64 != 0 || h != w {
        return None;
    }
    let k = w / 64;
    let mut out = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64 {
        for x in 0..64 {
            let mut acc = [0u32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let i = ((y * k + dy) * w + x * k + dx) * 4;
                    for c in 0..4 {
                        acc[c] += buf[i + c] as u32;
                    }
                }
            }
            out.extend(acc.iter().map(|v| (v / (k * k) as u32) as u8));
        }
    }
    Icon::from_rgba(out, 64, 64).ok()
}

impl App {
    fn new(mut options: AppOptions) -> App {
        let hook = options.frame_hook.take();
        App {
            title: options.title.clone(),
            options,
            hook,
            window: None,
            surface: None,
            gui: None,
            egui_state: None,
            instance: None,
            next_redraw: None,
            frames: 0,
            error: None,
            modifiers: ModifiersState::empty(),
        }
    }

    fn init(&mut self, el: &ActiveEventLoop) -> Result<(), AppError> {
        let attrs = WindowAttributes::default()
            .with_title(self.options.title.clone())
            .with_window_icon(window_icon())
            .with_inner_size(LogicalSize::new(
                self.options.window_size.0 as f64,
                self.options.window_size.1 as f64,
            ));
        let window = Arc::new(
            el.create_window(attrs)
                .map_err(|e| AppError::Window(e.to_string()))?,
        );
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
                Box::new(el.owned_display_handle()),
            ));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| AppError::Graphics(e.to_string()))?;
        let adapter = pick_adapter(&instance, Some(&surface))?;
        let (device, queue) = request_device(&adapter)?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| AppError::Graphics("surface supports no formats".into()))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| AppError::Graphics("surface is not supported by the adapter".into()))?;
        config.format = format;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        config.present_mode = wgpu::PresentMode::Fifo;
        config.view_formats = vec![];
        surface.configure(&device, &config);

        let max_side = device.limits().max_texture_dimension_2d as usize;
        let info = describe(&adapter);
        let doc = std::mem::take(&mut self.options.document);
        let mut gui = Gui::new(device, queue, format, doc, info)?;
        apply_startup(&mut gui, &self.options);
        let egui_state = egui_winit::State::new(
            gui.ctx().clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(max_side),
        );
        self.window = Some(window);
        self.surface = Some(Surface { surface, config });
        self.gui = Some(gui);
        self.egui_state = Some(egui_state);
        self.instance = Some(instance);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
        Ok(())
    }

    fn fail(&mut self, el: &ActiveEventLoop, e: AppError) {
        self.error = Some(e);
        el.exit();
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let (Some(s), Some(g)) = (self.surface.as_mut(), self.gui.as_ref()) {
            s.config.width = width;
            s.config.height = height;
            s.surface.configure(g.device(), &s.config);
        }
    }

    fn redraw(&mut self, el: &ActiveEventLoop) {
        let (Some(window), Some(surf), Some(gui), Some(egui_state)) = (
            self.window.clone(),
            self.surface.as_ref(),
            self.gui.as_mut(),
            self.egui_state.as_mut(),
        ) else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let size_px = [size.width, size.height];
        let shot_due = self
            .options
            .screenshot
            .as_ref()
            .is_some_and(|s| self.frames + 1 >= s.after_frames);

        let frame = if shot_due {
            None
        } else {
            match surf.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f)
                | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    surf.surface.configure(gui.device(), &surf.config);
                    window.request_redraw();
                    return;
                }
                wgpu::CurrentSurfaceTexture::Timeout
                | wgpu::CurrentSurfaceTexture::Occluded
                | wgpu::CurrentSurfaceTexture::Validation => return,
            }
        };

        let input = egui_state.take_egui_input(&window);
        let out;
        if let Some(frame) = frame {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            out = gui.frame(input, &view, size_px);
            window.pre_present_notify();
            gui.queue().present(frame);
        } else {
            // Screenshot: draw the frame into a readable texture instead.
            let target = OffscreenTarget::new(gui.device(), surf.config.format, size_px);
            out = gui.frame(input, &target.view, size_px);
            let result = target
                .read_rgba(gui.device(), gui.queue())
                .and_then(|rgba| {
                    let path = self
                        .options
                        .screenshot
                        .as_ref()
                        .map(|s| s.path.clone())
                        .unwrap_or_default();
                    save_png(&path, &rgba, size_px)
                });
            if let Err(e) = result {
                self.error = Some(e);
            }
            el.exit();
        }
        egui_state.handle_platform_output(&window, out.platform_output);
        self.frames += 1;

        // Window title follows the document.
        let name = gui
            .state
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(
                || "untitled".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
        let title = format!(
            "{} \u{2014} {}{}",
            self.options.title,
            name,
            if gui.state.is_dirty() { " *" } else { "" }
        );
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }

        if gui.state.should_exit {
            el.exit();
            return;
        }
        self.next_redraw = None;
        if self.options.screenshot.is_some() || out.repaint_after.is_zero() {
            window.request_redraw();
        } else if out.repaint_after != Duration::MAX {
            self.next_redraw = Instant::now().checked_add(out.repaint_after);
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(e) = self.init(el) {
                self.fail(el, e);
            }
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.next_redraw = None;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _event: UserEvent) {
        // The wake itself is the point: this iteration runs the frame hook
        // in `about_to_wait`.
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(window), Some(egui_state)) = (self.window.clone(), self.egui_state.as_mut())
        else {
            return;
        };
        if let WindowEvent::ModifiersChanged(m) = &event {
            self.modifiers = m.state();
        }
        // Keypad keys are view shortcuts; egui cannot tell them from the digit
        // row, so they are taken here (unless a modal operator wants digits).
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = &event
        {
            if let (PhysicalKey::Code(code), Some(gui)) =
                (key_event.physical_key, self.gui.as_mut())
            {
                let name = match code {
                    KeyCode::Numpad1 => Some("1"),
                    KeyCode::Numpad3 => Some("3"),
                    KeyCode::Numpad5 => Some("5"),
                    KeyCode::Numpad7 => Some("7"),
                    KeyCode::NumpadDecimal => Some("."),
                    _ => None,
                };
                if let Some(name) = name {
                    if !gui.state.is_modal() && !gui.ctx().egui_wants_keyboard_input() {
                        if key_event.state == ElementState::Pressed && !key_event.repeat {
                            if let Some(a) = keymap::numpad(name, self.modifiers.control_key()) {
                                gui.state.perform(a);
                            }
                            window.request_redraw();
                        }
                        return;
                    }
                }
            }
        }
        let response = egui_state.on_window_event(&window, &event);
        if response.repaint {
            window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                if let Some(gui) = self.gui.as_mut() {
                    gui.state.perform(crate::state::Action::Quit);
                    if gui.state.should_exit {
                        el.exit();
                    }
                    window.request_redraw();
                } else {
                    el.exit();
                }
            }
            WindowEvent::Resized(size) => {
                self.resize(size.width, size.height);
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => window.request_redraw(),
            WindowEvent::RedrawRequested => self.redraw(el),
            WindowEvent::DroppedFile(path) => {
                if let Some(gui) = self.gui.as_mut() {
                    let is_project = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("f3d"));
                    if is_project {
                        gui.state.open_project(&path);
                    } else {
                        gui.state.import_file(&path);
                    }
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let (Some(hook), Some(gui)) = (self.hook.as_mut(), self.gui.as_mut()) {
            if hook(&mut gui.state.doc) {
                gui.state.sync_external();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
        }
        el.set_control_flow(match self.next_redraw {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_embedded_logo_becomes_a_window_icon() {
        assert!(super::window_icon().is_some());
    }
}
