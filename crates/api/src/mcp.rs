//! MCP server (official Rust SDK `rmcp`) exposing the forge3d tools and
//! resources over stdio or Streamable HTTP.
//!
//! Tool failures are returned as tool results with `isError: true` whose
//! content is the structured error JSON `{code, message, hint, details?}`,
//! so the model can read the hint and self-correct. Protocol-level errors
//! (unknown tool, unknown resource) use JSON-RPC errors.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

use crate::error::ApiError;
use crate::resources;
use crate::tools::{tool_defs, tool_names, Forge3dTools};

/// Instructions sent to the client at initialization.
pub const INSTRUCTIONS: &str = "forge3d is a 3D polygon modeler. You edit ONE shared document (the user may watch it live in the GUI).\n\
Workflow: 1) scene_get to see objects, ids and the selection. 2) Build with object_create (primitives: cube, plane, uv_sphere, ico_sphere, cylinder, cone, torus), object_transform, material_set, and refine meshes with mesh_select + mesh_edit (extrude, inset, bevel, loop_cut, subdivide, mirror, ...). 3) viewport_render to LOOK at the result and fix proportions. 4) file_export (.glb) or project_save (.f3d) when asked.\n\
Conventions: Z is up, units are meters, primitives are centered at their origin (a cube of size 1 at translation [0,0,0.5] sits on the ground). Rotations are XYZ Euler degrees.\n\
Objects can be referenced by id OR by unique name. Element indices change after topology edits: re-query with mesh_select/mesh_get. Use batch for multi-step changes that should be one atomic undo step. Every change is undoable (undo/redo/history_list) and labelled \"AI: ...\" in the history.\n\
Errors come back as {code, message, hint}: read the hint and retry. File paths are relative to the server's workspace directory.";

/// The MCP server handler.
#[derive(Clone)]
pub struct Forge3dMcpServer {
    tools: Forge3dTools,
    tool_list: Arc<Vec<Tool>>,
}

impl std::fmt::Debug for Forge3dMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forge3dMcpServer").finish_non_exhaustive()
    }
}

fn build_tools() -> Vec<Tool> {
    tool_defs()
        .into_iter()
        .map(|d| {
            let schema = match d.input_schema {
                Value::Object(m) => m,
                _ => serde_json::Map::new(),
            };
            Tool::new(d.name, d.description, Arc::new(schema))
                .with_title(d.title)
                .with_annotations(
                    ToolAnnotations::with_title(d.title)
                        .read_only(d.read_only)
                        .destructive(d.destructive)
                        .idempotent(d.read_only)
                        .open_world(false),
                )
        })
        .collect()
}

fn error_result(e: &ApiError) -> CallToolResult {
    let json = e.to_json();
    let mut r = CallToolResult::error(vec![ContentBlock::text(
        serde_json::to_string_pretty(&json).expect("serializes"),
    )]);
    r.structured_content = Some(json);
    r
}

impl Forge3dMcpServer {
    /// Wrap a tool set (create it with `Origin::Ai`).
    pub fn new(tools: Forge3dTools) -> Self {
        Forge3dMcpServer {
            tools,
            tool_list: Arc::new(build_tools()),
        }
    }

    /// The underlying tools.
    pub fn tools(&self) -> &Forge3dTools {
        &self.tools
    }

    /// Execute a tool call and convert the outcome to an MCP result.
    pub async fn call(&self, name: &str, args: Value) -> Result<CallToolResult, McpError> {
        if !tool_names().contains(&name) {
            let e = ApiError::unknown_tool(name, &tool_names());
            return Err(McpError::invalid_params(
                e.message.clone(),
                Some(e.to_json()),
            ));
        }
        tracing::info!(tool = name, "tool call");
        match self.tools.call_rich(name, args).await {
            Ok(out) => {
                let mut content = Vec::new();
                for img in &out.images {
                    content.push(ContentBlock::image(img.base64(), img.mime_type.clone()));
                }
                content.push(ContentBlock::text(
                    serde_json::to_string(&out.value).expect("serializes"),
                ));
                let mut r = CallToolResult::success(content);
                r.structured_content = Some(out.value);
                Ok(r)
            }
            Err(e) => {
                tracing::info!(tool = name, code = %e.code, "tool error");
                Ok(error_result(&e))
            }
        }
    }
}

impl ServerHandler for Forge3dMcpServer {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(
            Implementation::new("forge3d", env!("CARGO_PKG_VERSION"))
                .with_title("forge3d 3D modeler")
                .with_description("Polygon modeling: primitives, mesh editing, booleans, materials, rendering and glTF/OBJ/STL export."),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            self.tool_list.as_ref().clone(),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_list.iter().find(|t| t.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        self.call(&request.name, args).await.map(Into::into)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let list = resources::list_resources(self.tools.handle())
            .await
            .map_err(|e| McpError::internal_error(e.message.clone(), Some(e.to_json())))?;
        Ok(ListResourcesResult::with_all_items(
            list.into_iter()
                .map(|r| {
                    Resource::new(r.uri, r.name)
                        .with_description(r.description)
                        .with_mime_type(r.mime_type)
                })
                .collect(),
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new(resources::OBJECT_TEMPLATE, "object")
                .with_title("Scene object")
                .with_description(
                    "One object by id or unique name: transform, material, counts, bounds.",
                )
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri;
        match resources::read_resource(self.tools.handle(), &uri).await {
            Ok(v) => {
                let mime = if uri == resources::COMMAND_SCHEMA_URI {
                    "application/schema+json"
                } else {
                    "application/json"
                };
                let text = serde_json::to_string_pretty(&v).expect("serializes");
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, uri).with_mime_type(mime)
                ])
                .into())
            }
            Err(e) if e.code == "resource_not_found" || e.code == "object_not_found" => Err(
                McpError::resource_not_found(e.message.clone(), Some(e.to_json())),
            ),
            Err(e) => Err(McpError::internal_error(
                e.message.clone(),
                Some(e.to_json()),
            )),
        }
    }
}

/// Error type of the server entry points.
pub type ServeError = Box<dyn std::error::Error + Send + Sync>;

/// Serve MCP over stdin/stdout until the client disconnects. Nothing but
/// JSON-RPC is written to stdout; log with `tracing` to stderr.
pub async fn serve_stdio(tools: Forge3dTools) -> Result<(), ServeError> {
    let server = Forge3dMcpServer::new(tools);
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

/// Serve MCP over any byte stream pair (in-process tests, sockets, ...).
pub async fn serve_io<R, W>(tools: Forge3dTools, reader: R, writer: W) -> Result<(), ServeError>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let server = Forge3dMcpServer::new(tools);
    let running = server.serve((reader, writer)).await?;
    running.waiting().await?;
    Ok(())
}

/// Streamable HTTP server settings.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Listen address (default 127.0.0.1:7878; port 0 picks a free port).
    pub addr: SocketAddr,
    /// Endpoint path (default `/mcp`).
    pub path: String,
    /// Require `Authorization: Bearer <token>` when set.
    pub token: Option<String>,
    /// Extra allowed browser origins (e.g. `https://app.example.com:443`);
    /// localhost origins are always allowed, other `Origin`s get 403.
    pub allowed_origins: Vec<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            addr: SocketAddr::from(([127, 0, 0, 1], 7878)),
            path: "/mcp".into(),
            token: None,
            allowed_origins: Vec::new(),
        }
    }
}

fn unauthorized_response() -> Response {
    let body = serde_json::to_vec(&ApiError::unauthorized().to_json()).expect("serializes");
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = StatusCode::UNAUTHORIZED;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"forge3d\""),
    );
    resp
}

/// Constant-time bearer token check.
pub fn bearer_matches(header_value: Option<&str>, token: &str) -> bool {
    let Some(v) = header_value else { return false };
    let Some(presented) = v
        .strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))
    else {
        return false;
    };
    let a = presented.trim().as_bytes();
    let b = token.as_bytes();
    // `ct_eq` on slices of different length returns false (length is not secret).
    bool::from(a.ct_eq(b))
}

/// Build the axum router serving MCP at `config.path` (with bearer auth when
/// a token is configured). Cancel `ct` to close all sessions.
pub fn http_router(
    tools: Forge3dTools,
    config: &HttpConfig,
    ct: CancellationToken,
) -> Result<axum::Router, ServeError> {
    let loopback = config.addr.ip().is_loopback();
    if !loopback && config.token.is_none() {
        return Err(format!(
            "refusing to serve on non-loopback address {} without a bearer token; pass --token or FORGE3D_TOKEN",
            config.addr
        )
        .into());
    }
    let mut origins: Vec<String> = vec![
        "http://localhost:*".into(),
        "http://127.0.0.1:*".into(),
        "http://[::1]:*".into(),
    ];
    origins.extend(config.allowed_origins.iter().cloned());
    let mut sse_config = StreamableHttpServerConfig::default()
        .with_cancellation_token(ct)
        .with_allowed_origins(origins);
    if !loopback {
        // Reached by IP/hostname from other machines; access is protected
        // by the mandatory bearer token instead of Host validation.
        sse_config = sse_config.disable_allowed_hosts();
    }
    let server = Forge3dMcpServer::new(tools);
    let service: StreamableHttpService<Forge3dMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(LocalSessionManager::default()),
            sse_config,
        );
    let path = if config.path.starts_with('/') {
        config.path.clone()
    } else {
        format!("/{}", config.path)
    };
    let mut router = axum::Router::new().nest_service(&path, service);
    if let Some(token) = config.token.clone() {
        let token: Arc<str> = Arc::from(token);
        router = router.layer(axum::middleware::from_fn(
            move |req: Request, next: Next| {
                let token = token.clone();
                async move {
                    let auth = req
                        .headers()
                        .get(header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok());
                    if bearer_matches(auth, &token) {
                        next.run(req).await
                    } else {
                        tracing::warn!("rejected HTTP request without a valid bearer token");
                        unauthorized_response()
                    }
                }
            },
        ));
    }
    Ok(router)
}

/// A running Streamable HTTP server.
pub struct RunningHttpServer {
    /// The bound address (useful with port 0).
    pub local_addr: SocketAddr,
    /// Full endpoint URL, e.g. `http://127.0.0.1:7878/mcp`.
    pub url: String,
    ct: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl RunningHttpServer {
    /// Request a graceful shutdown.
    pub fn shutdown(&self) {
        self.ct.cancel();
    }

    /// Token that stops the server when cancelled.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.ct.clone()
    }

    /// Wait until the server has stopped.
    pub async fn wait(self) -> Result<(), ServeError> {
        self.task.await??;
        Ok(())
    }
}

/// Bind and start the Streamable HTTP server in the background.
pub async fn start_http(
    tools: Forge3dTools,
    config: HttpConfig,
) -> Result<RunningHttpServer, ServeError> {
    let ct = CancellationToken::new();
    let router = http_router(tools, &config, ct.child_token())?;
    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    let local_addr = listener.local_addr()?;
    let path = if config.path.starts_with('/') {
        config.path.clone()
    } else {
        format!("/{}", config.path)
    };
    let url = format!("http://{local_addr}{path}");
    let stop = ct.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled_owned().await })
            .await
    });
    Ok(RunningHttpServer {
        local_addr,
        url,
        ct,
        task,
    })
}
