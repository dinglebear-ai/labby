//! A programmatic stdio client can request a larger Code Mode response budget
//! through `tools/call` `_meta`; agents that send nothing keep the default.
//!
//! This drives `labby mcp` the way a dashboard does: a persistent stdio
//! session with a legacy initialize and fixed read-only scripts.
#![cfg(feature = "gateway")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const BUDGET_KEY: &str = "ai.dinglebear.labby/responseBudget";
const RESULT_CHARS: usize = 200_000;

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    _home: tempfile::TempDir,
}

impl Session {
    fn start(code_mode_toml: &str) -> Self {
        let home = tempfile::tempdir().expect("LABBY_HOME");
        std::fs::create_dir_all(home.path().join("tmp")).unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!("[code_mode]\nenabled = true\ntimeout_ms = 30000\n{code_mode_toml}"),
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
            .arg("mcp")
            .env_clear()
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path())
            .env("TMPDIR", home.path().join("tmp"))
            .env("LABBY_MCP_FORCE_STANDALONE", "1")
            .env("LABBY_CODE_MODE_RUNNER_BACKEND", "process")
            .env("LABBY_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn labby mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            _home: home,
        };
        let hello = session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "budget-test", "version": "1" }
            }),
        );
        assert!(hello.get("result").is_some(), "initialize failed: {hello}");
        session.notify("notifications/initialized");
        session
    }

    fn notify(&mut self, method: &str) {
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "method": method })
        )
        .unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        )
        .unwrap();
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read response");
            assert!(!line.is_empty(), "labby mcp exited before response {id}");
            let value: Value = serde_json::from_str(line.trim()).expect("JSON-RPC line");
            if value["id"] == id {
                return value;
            }
        }
    }

    /// Run a fixed read-only script, optionally requesting a response budget.
    fn run(&mut self, budget: Option<Value>) -> Value {
        let mut params = json!({
            "name": "codemode_read",
            "arguments": { "code": format!("async () => 'x'.repeat({RESULT_CHARS})") }
        });
        if let Some(budget) = budget {
            params["_meta"] = json!({ BUDGET_KEY: budget });
        }
        let response = self.request("tools/call", params);
        assert!(
            response.get("result").is_some(),
            "tools/call failed: {response}"
        );
        response["result"].clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn envelope(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(text).expect("JSON envelope")
}

#[test]
fn agent_default_budget_truncates_and_program_budget_returns_everything() {
    let mut session = Session::start("");

    // No request: the agent-facing 24 KiB envelope cap still applies.
    let agent = session.run(None);
    assert_ne!(agent["isError"], true, "{agent}");
    let agent_text = agent["content"][0]["text"].as_str().unwrap();
    assert!(agent_text.len() <= 24 * 1024, "{}", agent_text.len());
    assert_eq!(envelope(&agent)["result"]["truncated"], true);
    assert!(agent["_meta"].get(BUDGET_KEY).is_none());

    // A program asks for 1 MiB through request metadata and gets the full answer.
    let program = session.run(Some(json!({ "maxResponseBytes": 1_048_576 })));
    assert_ne!(program["isError"], true, "{program}");
    let full = envelope(&program);
    assert_eq!(
        full["result"].as_str().map(str::len),
        Some(RESULT_CHARS),
        "result was not returned in full"
    );
    assert_eq!(
        program["_meta"][BUDGET_KEY],
        json!({ "maxResponseBytes": 1_048_576 })
    );

    // A request above the gateway maximum fails clearly instead of truncating.
    let too_large = session.run(Some(json!({ "maxResponseBytes": 8 * 1024 * 1024 })));
    assert_eq!(too_large["isError"], true, "{too_large}");
    let error = too_large["content"][0]["text"].as_str().unwrap();
    assert!(error.contains("invalid_param"), "{error}");
    assert!(error.contains("1048576"), "{error}");

    // Malformed metadata is rejected rather than ignored.
    let malformed = session.run(Some(json!("big")));
    assert_eq!(malformed["isError"], true, "{malformed}");

    // A budget smaller than the result still truncates, at that budget.
    let small = session.run(Some(json!({ "maxResponseBytes": 4096 })));
    assert_ne!(small["isError"], true, "{small}");
    assert!(small["content"][0]["text"].as_str().unwrap().len() <= 4096);
    assert_eq!(envelope(&small)["result"]["truncated"], true);
}

#[test]
fn operator_can_disable_programmatic_budgets() {
    let mut session = Session::start("max_programmatic_response_bytes = 0\n");
    let refused = session.run(Some(json!({ "maxResponseBytes": 65_536 })));
    assert_eq!(refused["isError"], true, "{refused}");
    let error = refused["content"][0]["text"].as_str().unwrap();
    assert!(error.contains("disabled"), "{error}");

    let agent = session.run(None);
    assert_ne!(agent["isError"], true, "{agent}");
    assert_eq!(envelope(&agent)["result"]["truncated"], true);
}
