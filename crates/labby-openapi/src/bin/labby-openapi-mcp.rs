//! Independent stdio MCP server over Labby's guarded OpenAPI dispatcher.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use clap::{ArgGroup, Parser};
use labby_openapi::{
    OpenApiCredential, OpenApiProviderConfig, OpenApiRegistry, OpenApiSpecConfig, SpecSource,
    dispatch_openapi_call, http::build_dispatch_client,
};
use rmcp::{
    ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
        Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
        Tool, ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Map, Value, json};

const LABEL: &str = "mcp-upstream";

#[derive(Parser)]
#[command(
    version,
    about = "Serve selected OpenAPI operations as a standard stdio MCP server"
)]
#[command(group(ArgGroup::new("source").args(["spec_path", "spec_url"]).required(true)))]
struct Args {
    /// Local JSON OpenAPI document.
    #[arg(long)]
    spec_path: Option<PathBuf>,
    /// Remote HTTPS JSON OpenAPI document.
    #[arg(long)]
    spec_url: Option<String>,
    /// API origin and optional base path; never sourced from the spec.
    #[arg(long)]
    base_url: String,
    /// Exact operationId to expose. Repeat to permit additional operations.
    #[arg(long = "allow-operation", required = true)]
    allowed_operations: Vec<String>,
    /// Explicitly permit selected methods other than GET and HEAD.
    #[arg(long)]
    allow_write: bool,
    /// Read a server-scoped Bearer token from this environment variable.
    #[arg(long, conflicts_with = "api_key_env")]
    bearer_token_env: Option<String>,
    /// Read a server-scoped API key from this environment variable.
    #[arg(long)]
    api_key_env: Option<String>,
    /// Header used with --api-key-env (default X-API-Key).
    #[arg(long, requires = "api_key_env")]
    api_key_header: Option<String>,
}

fn credential(args: &Args) -> Result<Option<OpenApiCredential>, &'static str> {
    fn read_secret(env: &str) -> Result<String, &'static str> {
        let value = std::env::var(env).map_err(|_| "configured credential is missing")?;
        if value.trim().is_empty() || reqwest::header::HeaderValue::from_str(&value).is_err() {
            return Err("configured credential is empty or is not a valid header value");
        }
        Ok(value)
    }
    if let Some(env) = &args.bearer_token_env {
        return read_secret(env)
            .map(OpenApiCredential::BearerToken)
            .map(Some);
    }
    if let Some(env) = &args.api_key_env {
        let header = args.api_key_header.as_deref().unwrap_or("X-API-Key");
        let normalized = reqwest::header::HeaderName::from_bytes(header.as_bytes())
            .map_err(|_| "configured API key header name is invalid")?;
        if matches!(
            normalized.as_str(),
            "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "te"
                | "trailer"
                | "proxy-authorization"
                | "proxy-connection"
                | "upgrade"
                | "cookie"
                | "content-type"
        ) {
            return Err("configured API key header conflicts with transport or cookie handling");
        }
        return Ok(Some(OpenApiCredential::ApiKey {
            header: normalized.to_string(),
            value: read_secret(env)?,
        }));
    }
    Ok(None)
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

#[derive(Clone)]
struct OpenApiMcp {
    registry: OpenApiRegistry,
    client: reqwest::Client,
    tools: Arc<BTreeMap<String, Tool>>,
}

impl OpenApiMcp {
    async fn new(args: Args) -> Result<Self, String> {
        let credential = credential(&args)?;
        let base_url = url::Url::parse(&args.base_url).map_err(|_| "invalid API base URL")?;
        let source = match (args.spec_path, args.spec_url) {
            (Some(path), None) => SpecSource::Path(path),
            (None, Some(url)) => {
                SpecSource::Url(url::Url::parse(&url).map_err(|_| "invalid spec URL")?)
            }
            _ => return Err("must select exactly one spec source".into()),
        };
        if args.allowed_operations.iter().any(|n| !valid_tool_name(n)) {
            return Err("operation IDs must be valid MCP tool names (1–128 letters, digits, underscores, dots or hyphens)".into());
        }
        let registry = OpenApiRegistry::load(
            OpenApiProviderConfig {
                specs: vec![OpenApiSpecConfig {
                    label: LABEL.into(),
                    spec_source: source,
                    base_url,
                    allowed_operations: args.allowed_operations.clone(),
                    credential,
                    oauth_upstream: None,
                }],
            },
            Duration::from_secs(12),
        )
        .await;
        let mut tools = BTreeMap::new();
        for name in args.allowed_operations {
            let operation = registry.operation(LABEL, &name).map_err(|_| {
                format!("operation {name} is absent or the OpenAPI document could not be loaded")
            })?;
            let (description, schema) = registry.mcp_tool(LABEL, &name).ok_or_else(|| {
                format!("operation {name} has duplicate IDs or unsupported input metadata")
            })?;
            let read_only = matches!(operation.method.as_str(), "GET" | "HEAD");
            if !read_only && !args.allow_write {
                return Err(format!(
                    "operation {name} is not read-only; review it and pass --allow-write to expose it"
                ));
            }
            tools.insert(
                name.clone(),
                Tool::new_with_raw(
                    name,
                    Some(description.to_owned().into()),
                    Arc::new(schema.clone()),
                )
                .with_annotations(
                    ToolAnnotations::new()
                        .read_only(read_only)
                        .destructive(!read_only)
                        .open_world(true),
                ),
            );
        }
        let client = build_dispatch_client().map_err(|error| error.to_string())?;
        Ok(Self {
            registry,
            client,
            tools: Arc::new(tools),
        })
    }
}

fn validate_args(
    schema: &Map<String, Value>,
    args: &Map<String, Value>,
) -> Result<(), &'static str> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("invalid server schema")?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or("invalid server schema")?;
    if required
        .iter()
        .filter_map(Value::as_str)
        .any(|name| !args.contains_key(name))
    {
        return Err("a required operation argument is missing");
    }
    for (name, value) in args {
        let ty = properties
            .get(name)
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str);
        let good = match ty {
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("integer") => value.is_i64() || value.is_u64(),
            Some("number") => value.is_number(),
            Some(_) | None => false,
        };
        if !good {
            return Err("operation argument has an unexpected name or type");
        }
    }
    Ok(())
}

impl ServerHandler for OpenApiMcp {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        config.server_info = Implementation::new("labby-openapi-mcp", env!("CARGO_PKG_VERSION"));
        config
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            self.tools.values().cloned().collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.as_ref();
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ErrorData::invalid_params("operation is not exposed", None))?;
        let args = request.arguments.unwrap_or_default();
        validate_args(&tool.input_schema, &args)
            .map_err(|message| ErrorData::invalid_params(message, None))?;
        let outcome = match dispatch_openapi_call(
            &self.registry,
            &self.client,
            LABEL,
            name,
            Value::Object(args),
        )
        .await
        {
            Ok(value) => CallToolResult::structured(json!({"result": value})),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
        };
        Ok(outcome.into())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let server = match OpenApiMcp::new(Args::parse()).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("labby-openapi-mcp: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match server.serve(rmcp::transport::stdio_pipes()).await {
        Ok(running) => {
            if running.waiting().await.is_ok() {
                std::process::ExitCode::SUCCESS
            } else {
                eprintln!("labby-openapi-mcp: MCP transport stopped unexpectedly");
                std::process::ExitCode::FAILURE
            }
        }
        Err(_) => {
            eprintln!("labby-openapi-mcp: could not initialize MCP stdio");
            std::process::ExitCode::FAILURE
        }
    }
}
