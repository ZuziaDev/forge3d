//! `forge3d-gui` command line.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use forge3d_app::gizmo::GizmoMode;
use forge3d_app::{run, AppOptions, ScreenshotRequest};
use forge3d_core::document::Document;
use forge3d_render::ViewPreset;

const USAGE: &str = "\
forge3d-gui [FILE] [options]

  FILE                    .f3d project to open, or a .gltf/.glb/.obj/.stl file to import
  --demo                  start with a small demo scene
  --script FILE           run a script of CommandSpec JSON lines before the window opens
  --view NAME             front|back|left|right|top|bottom|iso
  --tool NAME             translate|rotate|scale
  --ortho                 start with an orthographic camera
  --frame-selected        frame the selection (instead of the whole scene) at startup
  --size WxH              window size in points (default 1400x900)
  --screenshot OUT.png    render the UI to a PNG and exit
  --after-frames N        frames to draw before the screenshot (default 8)
  --headless              with --screenshot: render without a window / display server
  --scale F               with --headless: HiDPI scale factor (default 1)
  -h, --help              this text";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut options = AppOptions::default();
    let mut file: Option<PathBuf> = None;
    let mut shot: Option<PathBuf> = None;
    let mut after_frames = 8u32;
    let mut headless = false;
    let mut size = (1400u32, 900u32);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--demo" => options.demo = true,
            "--script" => {
                let p = value("--script")?;
                options.script = Some(
                    std::fs::read_to_string(&p).with_context(|| format!("reading script {p}"))?,
                );
            }
            "--view" => {
                options.view = Some(match value("--view")?.as_str() {
                    "front" => ViewPreset::Front,
                    "back" => ViewPreset::Back,
                    "left" => ViewPreset::Left,
                    "right" => ViewPreset::Right,
                    "top" => ViewPreset::Top,
                    "bottom" => ViewPreset::Bottom,
                    "iso" => ViewPreset::Iso,
                    other => bail!("unknown view '{other}'"),
                })
            }
            "--tool" => {
                options.tool = match value("--tool")?.as_str() {
                    "translate" | "move" => GizmoMode::Translate,
                    "rotate" => GizmoMode::Rotate,
                    "scale" => GizmoMode::Scale,
                    other => bail!("unknown tool '{other}'"),
                }
            }
            "--ortho" => options.orthographic = true,
            "--frame-selected" => options.frame_selected = true,
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').context("--size expects WxH")?;
                size = (w.parse()?, h.parse()?);
            }
            "--screenshot" => shot = Some(PathBuf::from(value("--screenshot")?)),
            "--after-frames" => after_frames = value("--after-frames")?.parse()?,
            "--headless" => headless = true,
            "--scale" => options.pixels_per_point = value("--scale")?.parse()?,
            s if s.starts_with('-') => bail!("unknown option '{s}'\n\n{USAGE}"),
            _ => file = Some(PathBuf::from(a)),
        }
    }
    options.window_size = size;
    options.headless_size = (
        (size.0 as f32 * options.pixels_per_point) as u32,
        (size.1 as f32 * options.pixels_per_point) as u32,
    );
    if let Some(path) = shot {
        options.screenshot = Some(ScreenshotRequest {
            path,
            after_frames,
            headless,
        });
    }
    if let Some(f) = file {
        let is_project = f
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("f3d"));
        if is_project {
            options.document = forge3d_io::load_project(&f)
                .with_context(|| format!("opening {}", f.display()))?;
            options.project_path = Some(f);
        } else {
            let mut doc = Document::new();
            forge3d_io::import_file_into_document(
                &mut doc,
                &f,
                &forge3d_io::ImportOptions::default(),
                None,
                forge3d_core::command::Origin::Gui,
            )
            .with_context(|| format!("importing {}", f.display()))?;
            options.document = doc;
        }
    }
    run(options)?;
    Ok(())
}
