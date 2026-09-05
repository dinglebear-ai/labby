//! Browser action metadata shared by CLI, MCP, API, and generated docs.

use labby_primitives::action::{ActionSpec, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "browser.status",
        description: "Show browser bridge health and live connection counts",
        destructive: false,
        requires_admin: false,
        returns: "BrowserBridgeStatus",
        params: &[],
    },
    ActionSpec {
        name: "browser.list",
        description: "List paired browsers and current connection state",
        destructive: false,
        requires_admin: true,
        returns: "BrowserList",
        params: &[],
    },
    ActionSpec {
        name: "browser.revoke",
        description: "Revoke one paired browser identity and close its active sessions",
        destructive: false,
        requires_admin: true,
        returns: "Browser",
        params: &[ParamSpec {
            name: "browser_id",
            ty: "string",
            required: true,
            description: "Paired browser id",
        }],
    },
    ActionSpec {
        name: "browser.pairing.list",
        description: "List pending browser-extension pairing requests",
        destructive: false,
        requires_admin: true,
        returns: "BrowserPairingList",
        params: &[],
    },
    ActionSpec {
        name: "browser.pairing.approve",
        description: "Approve one pending browser-extension pairing request",
        destructive: false,
        requires_admin: true,
        returns: "Browser",
        params: &[ParamSpec {
            name: "pairing_id",
            ty: "string",
            required: true,
            description: "Pending pairing request id",
        }],
    },
    ActionSpec {
        name: "browser.sessions",
        description: "List observed browser documents and sanitized WebMCP catalogs",
        destructive: false,
        requires_admin: true,
        returns: "BrowserSessionList",
        params: &[],
    },
    ActionSpec {
        name: "browser.session.enable",
        description: "Enable or disable WebMCP calls for one exact observed document",
        destructive: false,
        requires_admin: true,
        returns: "BrowserSession",
        params: &[
            ParamSpec {
                name: "session_id",
                ty: "string",
                required: true,
                description: "Observed document session id",
            },
            ParamSpec {
                name: "enabled",
                ty: "boolean",
                required: true,
                description: "Whether calls are allowed",
            },
        ],
    },
    ActionSpec {
        name: "browser.call",
        description: "Invoke one WebMCP tool on an exact browser document and catalog revision",
        destructive: false,
        requires_admin: true,
        returns: "PageToolResult",
        params: &[
            ParamSpec {
                name: "browser_id",
                ty: "string",
                required: true,
                description: "Paired browser id",
            },
            ParamSpec {
                name: "tab_id",
                ty: "integer",
                required: true,
                description: "Chrome tab id",
            },
            ParamSpec {
                name: "document_id",
                ty: "string",
                required: true,
                description: "Immutable Chrome document id",
            },
            ParamSpec {
                name: "catalog_revision",
                ty: "integer",
                required: true,
                description: "Observed catalog revision",
            },
            ParamSpec {
                name: "tool_name",
                ty: "string",
                required: true,
                description: "WebMCP tool name",
            },
            ParamSpec {
                name: "arguments",
                ty: "object",
                required: false,
                description: "Tool arguments",
            },
            ParamSpec {
                name: "timeout_ms",
                ty: "integer",
                required: false,
                description: "Bounded call deadline in milliseconds",
            },
        ],
    },
];
