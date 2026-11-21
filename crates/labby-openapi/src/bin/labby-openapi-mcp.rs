//! Independent stdio MCP server over Labby's guarded OpenAPI dispatcher.

use std::{collections::BTreeMap, future::Future, path::PathBuf, sync::Arc, time::Duration};

use clap::{ArgGroup, Parser};
use labby_openapi::{
    OpenApiCredential, OpenApiError, OpenApiProviderConfig, OpenApiRegistry, OpenApiSpecConfig,
    SpecSource, dispatch_openapi_call, http::build_dispatch_client,
    registry::validate_mcp_operation_ids,
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
use tokio::sync::Semaphore;

const LABEL: &str = "mcp-upstream";
const MAX_IN_FLIGHT: usize = 2;

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

struct OpenApiMcp {
    registry: OpenApiRegistry,
    client: reqwest::Client,
    tools: BTreeMap<String, Tool>,
    slots: Semaphore,
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
        validate_mcp_operation_ids(&args.allowed_operations).map_err(|error| error.to_string())?;
        let registry = OpenApiRegistry::load_mcp(
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
            let selected = registry
                .selected_mcp_tool(LABEL, &name, args.allow_write)
                .map_err(|error| error.to_string())?;
            tools.insert(
                name.clone(),
                Tool::new_with_raw(
                    name,
                    Some(selected.description.to_owned().into()),
                    Arc::new(selected.schema.clone()),
                )
                .with_annotations(
                    ToolAnnotations::new()
                        .read_only(selected.read_only)
                        .destructive(!selected.read_only)
                        .open_world(true),
                ),
            );
        }
        let client = build_dispatch_client().map_err(|error| error.to_string())?;
        Ok(Self {
            registry,
            client,
            tools,
            slots: Semaphore::new(MAX_IN_FLIGHT),
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

async fn dispatch_bounded<T>(
    slots: &Semaphore,
    cancelled: impl Future<Output = ()>,
    dispatch: impl Future<Output = T>,
) -> Result<T, ErrorData> {
    // Bound concurrency inside the standalone process. A saturated server does
    // not queue request futures; retry is the caller's decision.
    let _permit = slots
        .try_acquire()
        .map_err(|_| ErrorData::internal_error("OpenAPI MCP is busy", None))?;
    tokio::select! {
        biased;
        () = cancelled => {
            Err(ErrorData::internal_error("OpenAPI MCP call cancelled", None))
        }
        result = dispatch => Ok(result),
    }
}

fn tool_response(outcome: Result<Value, OpenApiError>) -> CallToolResponse {
    match outcome {
        Ok(value) => CallToolResult::structured(json!({"result": value})),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
    .into()
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.as_ref();
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ErrorData::invalid_params("operation is not exposed", None))?;
        let args = request.arguments.unwrap_or_default();
        validate_args(&tool.input_schema, &args)
            .map_err(|message| ErrorData::invalid_params(message, None))?;
        let outcome = dispatch_bounded(
            &self.slots,
            context.ct.cancelled(),
            dispatch_openapi_call(
                &self.registry,
                &self.client,
                LABEL,
                name,
                Value::Object(args),
            ),
        )
        .await?;
        Ok(tool_response(outcome))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, future};
    use tokio::sync::Notify;

    #[tokio::test]
    async fn saturated_server_rejects_without_dispatching() {
        let slots = Semaphore::new(MAX_IN_FLIGHT);
        let _held = slots.acquire_many(MAX_IN_FLIGHT as u32).await.unwrap();
        let polled = Cell::new(false);
        let error = dispatch_bounded(&slots, future::pending(), async {
            polled.set(true);
        })
        .await
        .unwrap_err();
        assert_eq!(error.message, "OpenAPI MCP is busy");
        assert!(!polled.get());
    }

    #[tokio::test]
    async fn cancellation_drops_dispatch_and_releases_permit() {
        struct DispatchDrop<'a>(&'a Cell<bool>);
        impl Drop for DispatchDrop<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let slots = Semaphore::new(MAX_IN_FLIGHT);
        let started = Notify::new();
        let cancel = Notify::new();
        let dropped = Cell::new(false);
        let call = dispatch_bounded(&slots, cancel.notified(), async {
            let _drop = DispatchDrop(&dropped);
            started.notify_one();
            future::pending::<()>().await;
        });
        let controller = async {
            started.notified().await;
            assert_eq!(slots.available_permits(), MAX_IN_FLIGHT - 1);
            cancel.notify_one();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(call, controller)
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().message, "OpenAPI MCP call cancelled");
        assert!(dropped.get());
        assert_eq!(slots.available_permits(), MAX_IN_FLIGHT);
        assert_eq!(
            dispatch_bounded(&slots, future::pending(), future::ready(7))
                .await
                .unwrap(),
            7,
        );
    }

    #[tokio::test]
    async fn already_cancelled_call_does_not_start_dispatch() {
        let slots = Semaphore::new(MAX_IN_FLIGHT);
        let polled = Cell::new(false);
        let err = dispatch_bounded(&slots, future::ready(()), async {
            polled.set(true);
        })
        .await
        .unwrap_err();
        assert_eq!(err.message, "OpenAPI MCP call cancelled");
        assert!(!polled.get());
    }

    #[test]
    fn maps_completed_dispatch_to_structured_mcp_success() {
        let CallToolResponse::Complete(response) = tool_response(Ok(json!({"id": "fixture-1"})))
        else {
            panic!("expected a completed tool response");
        };
        assert_eq!(
            response.structured_content,
            Some(json!({"result": {"id": "fixture-1"}}))
        );
        assert_ne!(response.is_error, Some(true));
    }
}
