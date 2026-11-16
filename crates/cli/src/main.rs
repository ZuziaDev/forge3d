//! `forge3d` command line.
//!
//! ```text
//! forge3d mcp   [--workspace DIR] [--project FILE.f3d]          stdio MCP server
//! forge3d serve [--addr 127.0.0.1:7878] [--token T] [--workspace DIR] [--project FILE.f3d]
//! forge3d run   SCRIPT.json [--export OUT]... [--save OUT.f3d] [--render OUT.png]
//! forge3d gui   [--workspace DIR] [--project FILE.f3d] [--mcp-http ADDR] [--token T] [--no-mcp] [--mcp-stdio]
//! forge3d tools                                                  tool list + schemas (JSON)
//! forge3d export IN OUT                                          convert files
//! ```
//!
//! With no subcommand (and the `gui` feature) the GUI starts.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use forge3d_api::script::{run_script, Script};
use forge3d_api::{DocumentHost, Forge3dTools, HttpConfig, Workspace};
use forge3d_core::command::Origin;
use forge3d_core::document::Document;

#[derive(Parser, Debug)]
#[command(
    name = "forge3d",
    version,
    about = "forge3d 3D polygon modeler: MCP server, script runner and converter"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Open the modeler window; an MCP server runs alongside and edits the same live document.
    #[cfg(feature = "gui")]
    Gui(GuiArgs),
    /// Run the MCP server on stdin/stdout (for Claude Desktop / Claude Code).
    Mcp(ServerArgs),
    /// Run the MCP server over Streamable HTTP.
    Serve(ServeArgs),
    /// Execute a JSON script headlessly, then export / save / render.
    Run(RunArgs),
    /// Print the tool list with JSON input schemas.
    Tools,
    /// Convert a file: .f3d/.glb/.gltf/.obj/.stl in, any export format out.
    Export {
        /// Input file (.f3d project or an importable mesh file).
        input: PathBuf,
        /// Output file; the format follows the extension.
        output: PathBuf,
    },
}

#[derive(Args, Debug, Clone)]
struct ServerArgs {
    /// Directory file tools are confined to (default: current directory).
    #[arg(long, env = "FORGE3D_WORKSPACE")]
    workspace: Option<PathBuf>,
    /// Project (.f3d) to open at start.
    #[arg(long)]
    project: Option<PathBuf>,
}

#[cfg(feature = "gui")]
#[derive(Args, Debug, Clone)]
struct GuiArgs {
    #[command(flatten)]
    server: ServerArgs,
    /// Streamable HTTP MCP address of the running GUI.
    #[arg(long, default_value = "127.0.0.1:7878")]
    mcp_http: SocketAddr,
    /// Require `Authorization: Bearer <TOKEN>` on the HTTP MCP endpoint.
    #[arg(long, env = "FORGE3D_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Do not start any MCP server.
    #[arg(long)]
    no_mcp: bool,
    /// Also serve MCP on stdin/stdout (for clients that spawn the GUI).
    #[arg(long)]
    mcp_stdio: bool,
    /// Render the window to a PNG and exit (testing).
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Frames to draw before --screenshot.
    #[arg(long, default_value_t = 8)]
    after_frames: u32,
}

#[cfg(feature = "gui")]
impl GuiArgs {
    /// Arguments for a bare `forge3d` (same defaults as `forge3d gui`).
    fn default_args() -> Self {
        let cli = Cli::parse_from(["forge3d", "gui"]);
        match cli.command {
            Some(Command::Gui(a)) => a,
            _ => unreachable!(),
        }
    }
}

#[derive(Args, Debug, Clone)]
struct ServeArgs {
    #[command(flatten)]
    server: ServerArgs,
    /// Listen address.
    #[arg(long, default_value = "127.0.0.1:7878")]
    addr: SocketAddr,
    /// Require `Authorization: Bearer <TOKEN>` on every request.
    #[arg(long, env = "FORGE3D_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Endpoint path.
    #[arg(long, default_value = "/mcp")]
    path: String,
    /// Additional allowed browser Origin (repeatable), e.g. https://app.example.com:443.
    #[arg(long = "allow-origin")]
    allow_origins: Vec<String>,
}

#[derive(Args, Debug, Clone)]
struct RunArgs {
    /// Script file (see examples/ for the format).
    script: PathBuf,
    /// Export the final scene (repeatable; format by extension).
    #[arg(long)]
    export: Vec<PathBuf>,
    /// Save the final document as .f3d project.
    #[arg(long)]
    save: Option<PathBuf>,
    /// Render the final scene to a PNG.
    #[arg(long)]
    render: Option<PathBuf>,
    /// Size of --render as WIDTHxHEIGHT.
    #[arg(long, default_value = "800x600")]
    render_size: String,
    /// Directory file tools inside the script are confined to (default: current directory).
    #[arg(long, env = "FORGE3D_WORKSPACE")]
    workspace: Option<PathBuf>,
    /// Project (.f3d) to start from instead of an empty scene.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Print only failures and the summary.
    #[arg(long, short)]
    quiet: bool,
}

fn init_logging(default: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("FORGE3D_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    // Logs go to stderr: stdout is reserved for JSON-RPC (mcp) and results.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_target(false)
        .try_init();
}

fn open_document(project: Option<&Path>) -> Result<Document> {
    match project {
        Some(p) => forge3d_io::load_project(p)
            .map_err(|e| anyhow::anyhow!("{} ({})", e, e.hint()))
            .with_context(|| format!("opening project {}", p.display())),
        None => Ok(Document::new()),
    }
}

fn workspace(dir: Option<&Path>) -> Result<Workspace> {
    let ws = match dir {
        Some(d) => Workspace::new(d),
        None => Workspace::current_dir(),
    };
    ws.map_err(|e| anyhow::anyhow!("{}: {} ({})", e.code, e.message, e.hint))
}

fn server_tools(args: &ServerArgs) -> Result<Forge3dTools> {
    let ws = workspace(args.workspace.as_deref())?;
    let doc = open_document(args.project.as_deref())?;
    tracing::info!(workspace = %ws.root().display(), objects = doc.scene.len(), "document ready");
    Ok(Forge3dTools::new(DocumentHost::spawn(doc), ws, Origin::Ai))
}

async fn cmd_mcp(args: ServerArgs) -> Result<ExitCode> {
    init_logging("info");
    let tools = server_tools(&args)?;
    tracing::info!("forge3d MCP server on stdio");
    forge3d_api::serve_stdio(tools)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ExitCode::SUCCESS)
}

async fn cmd_serve(args: ServeArgs) -> Result<ExitCode> {
    init_logging("info");
    let tools = server_tools(&args.server)?;
    let token = args.token.filter(|t| !t.is_empty());
    let config = HttpConfig {
        addr: args.addr,
        path: args.path,
        token: token.clone(),
        allowed_origins: args.allow_origins,
    };
    let server = forge3d_api::start_http(tools, config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!(
        "forge3d MCP server listening on {} ({})",
        server.url,
        if token.is_some() {
            "bearer token required"
        } else {
            "no auth, loopback only"
        }
    );
    let ct = server.cancellation_token();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("shutting down");
            ct.cancel();
        }
    });
    server.wait().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ExitCode::SUCCESS)
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .context("--render-size must look like 800x600")?;
    Ok((w.trim().parse()?, h.trim().parse()?))
}

async fn cmd_run(args: RunArgs) -> Result<ExitCode> {
    init_logging("warn");
    let text = std::fs::read_to_string(&args.script)
        .with_context(|| format!("reading script {}", args.script.display()))?;
    let script = Script::parse(&text)
        .map_err(api_err)
        .with_context(|| format!("parsing script {}", args.script.display()))?;
    let ws = workspace(args.workspace.as_deref())?;
    let doc = open_document(args.project.as_deref())?;
    let tools = Forge3dTools::new(DocumentHost::spawn(doc), ws, Origin::Cli);

    let quiet = args.quiet;
    let reports = run_script(&tools, &script, |r| {
        if !quiet || r.result.is_err() {
            println!("{}", r.to_json());
        }
    })
    .await;
    let failed = reports.iter().filter(|r| r.result.is_err()).count();
    let total = if script.atomic { 1 } else { script.steps.len() };
    println!(
        "{}",
        serde_json::json!({"summary": {
            "steps": total,
            "executed": reports.len(),
            "failed": failed,
            "ok": failed == 0,
        }})
    );
    if failed > 0 {
        eprintln!("script failed; nothing was exported");
        return Ok(ExitCode::FAILURE);
    }

    let state = tools
        .handle()
        .run(|doc| doc.state())
        .await
        .map_err(api_err)?;
    let ensure_parent = |p: &Path| -> Result<()> {
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        Ok(())
    };
    for out in &args.export {
        ensure_parent(out)?;
        let written = forge3d_io::export_scene(&state.scene, out, &Default::default())
            .map_err(|e| anyhow::anyhow!("{} ({})", e, e.hint()))
            .with_context(|| format!("exporting {}", out.display()))?;
        for w in written {
            eprintln!("exported {}", w.display());
        }
    }
    if let Some(save) = &args.save {
        ensure_parent(save)?;
        forge3d_io::save_project(&Document::from_state(state.clone()), save)
            .map_err(|e| anyhow::anyhow!("{} ({})", e, e.hint()))
            .with_context(|| format!("saving {}", save.display()))?;
        eprintln!("saved {}", save.display());
    }
    if let Some(render) = &args.render {
        let (w, h) = parse_size(&args.render_size)?;
        let out = tools
            .call_rich(
                "viewport_render",
                serde_json::json!({"width": w, "height": h, "show_selection": false}),
            )
            .await
            .map_err(api_err)?;
        let img = out.images.first().context("renderer returned no image")?;
        ensure_parent(render)?;
        std::fs::write(render, &img.data)
            .with_context(|| format!("writing {}", render.display()))?;
        eprintln!(
            "rendered {} ({}x{})",
            render.display(),
            img.width,
            img.height
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn api_err(e: forge3d_api::ApiError) -> anyhow::Error {
    anyhow::anyhow!("{}: {} ({})", e.code, e.message, e.hint)
}

fn cmd_tools() -> Result<ExitCode> {
    use std::io::Write;
    let defs = forge3d_api::tool_defs();
    let text = serde_json::to_string_pretty(&defs)?;
    let mut out = std::io::stdout().lock();
    match writeln!(out, "{text}").and_then(|_| out.flush()) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        // `forge3d tools | head` closes the pipe early; that is not an error.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(ExitCode::SUCCESS),
        Err(e) => Err(e.into()),
    }
}

fn cmd_export(input: &Path, output: &Path) -> Result<ExitCode> {
    let is_project = input
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("f3d"));
    let doc = if is_project {
        open_document(Some(input))?
    } else {
        let mut doc = Document::new();
        forge3d_io::import_file_into_document(
            &mut doc,
            input,
            &Default::default(),
            None,
            Origin::Cli,
        )
        .map_err(|e| anyhow::anyhow!("{} ({})", e, e.hint()))
        .with_context(|| format!("importing {}", input.display()))?;
        doc
    };
    if doc.scene.is_empty() {
        bail!("{} contains no objects", input.display());
    }
    let written = forge3d_io::export_scene(&doc.scene, output, &Default::default())
        .map_err(|e| anyhow::anyhow!("{} ({})", e, e.hint()))
        .with_context(|| format!("exporting {}", output.display()))?;
    for w in written {
        eprintln!("wrote {}", w.display());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "gui")]
fn cmd_gui(args: GuiArgs) -> Result<ExitCode> {
    use std::sync::{Arc, OnceLock};
    init_logging("info");
    let ws = workspace(args.server.workspace.as_deref())?;
    let doc = open_document(args.server.project.as_deref())?;
    let waker_cell: Arc<OnceLock<forge3d_app::Waker>> = Arc::new(OnceLock::new());
    let cell = waker_cell.clone();
    let (handle, jobs) = forge3d_api::DocumentHandle::channel_with_waker(Arc::new(move || {
        if let Some(w) = cell.get() {
            w.wake();
        }
    }));
    if !args.no_mcp {
        let tools = Forge3dTools::new(handle, ws, Origin::Ai);
        let token = args.token.clone().filter(|t| !t.is_empty());
        let (addr, stdio) = (args.mcp_http, args.mcp_stdio);
        std::thread::Builder::new()
            .name("forge3d-mcp".into())
            .spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => return eprintln!("MCP server disabled: cannot start runtime: {e}"),
                };
                rt.block_on(async move {
                    if stdio {
                        let t = tools.clone();
                        tokio::spawn(async move {
                            if let Err(e) = forge3d_api::serve_stdio(t).await {
                                eprintln!("MCP stdio stopped: {e}");
                            }
                        });
                    }
                    let config = HttpConfig {
                        addr,
                        token: token.clone(),
                        ..Default::default()
                    };
                    match forge3d_api::start_http(tools, config).await {
                        Ok(server) => {
                            eprintln!(
                                "forge3d MCP (live GUI) listening on {} ({})",
                                server.url,
                                if token.is_some() {
                                    "bearer token required"
                                } else {
                                    "no auth, loopback only"
                                }
                            );
                            let _ = server.wait().await;
                        }
                        Err(e) => {
                            eprintln!("MCP HTTP server not started on {addr}: {e}");
                            if !stdio {
                                std::future::pending::<()>().await;
                            }
                        }
                    }
                    std::future::pending::<()>().await;
                });
            })
            .context("starting the MCP thread")?;
    }
    let options = forge3d_app::AppOptions {
        document: doc,
        project_path: args.server.project.clone(),
        frame_hook: Some(Box::new(move |doc| jobs.drain(doc) > 0)),
        on_waker: Some(Box::new(move |w| {
            let _ = waker_cell.set(w);
        })),
        screenshot: args.screenshot.map(|path| forge3d_app::ScreenshotRequest {
            path,
            after_frames: args.after_frames,
            headless: false,
        }),
        ..Default::default()
    };
    forge3d_app::run(options).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    #[cfg(feature = "gui")]
    {
        let gui_args = match &cli.command {
            None => Some(GuiArgs::default_args()),
            Some(Command::Gui(a)) => Some(a.clone()),
            _ => None,
        };
        if let Some(a) = gui_args {
            return match cmd_gui(a) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
    }
    let Some(command) = cli.command else {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        return ExitCode::FAILURE;
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(async move {
        match command {
            #[cfg(feature = "gui")]
            Command::Gui(_) => unreachable!("handled before the runtime starts"),
            Command::Mcp(a) => cmd_mcp(a).await,
            Command::Serve(a) => cmd_serve(a).await,
            Command::Run(a) => cmd_run(a).await,
            Command::Tools => cmd_tools(),
            Command::Export { input, output } => cmd_export(&input, &output),
        }
    });
    // A pending stdin read (stdio transport) cannot be cancelled; do not let
    // it keep the process alive after the server has finished.
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
