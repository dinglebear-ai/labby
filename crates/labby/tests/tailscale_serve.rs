#![cfg(all(feature = "gateway", feature = "proxy-testkit"))]

use labby::proxy::config::ProxyPortPreference;
use labby::proxy::tailscale::{
    ServeStatus, TailscaleStatus, build_public_url, select_port_from_candidates,
};
#[cfg(unix)]
use labby::proxy::tailscale::{
    TailscaleClaimError, TailscaleServe, TailscaleServeOptions, TailscaleServePlan,
};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

const STATUS: &str = r#"{
  "BackendState": "Running",
  "Self": {"Online": true, "DNSName": "devhost.example.ts.net."}
}"#;

const SERVE_STATUS: &str = r#"{
  "TCP": {"52177": {"HTTPS": true}},
  "Web": {
    "devhost.example.ts.net:53147": {
      "Handlers": {"/": {"Proxy": "http://127.0.0.1:38417"}}
    }
  }
}"#;

#[test]
fn status_requires_running_online_node_with_dns_name() {
    let status = TailscaleStatus::parse(STATUS).unwrap();
    let identity = status.require_online().unwrap();
    assert_eq!(identity.dns_name, "devhost.example.ts.net.");

    for invalid in [
        r#"{"BackendState":"Stopped","Self":{"Online":true,"DNSName":"node.ts.net."}}"#,
        r#"{"BackendState":"Running","Self":{"Online":false,"DNSName":"node.ts.net."}}"#,
        r#"{"BackendState":"Running","Self":{"Online":true,"DNSName":""}}"#,
    ] {
        assert!(
            TailscaleStatus::parse(invalid)
                .unwrap()
                .require_online()
                .is_err()
        );
    }
}

#[test]
fn public_url_trims_only_the_dns_trailing_dot() {
    let url = build_public_url("devhost.example.ts.net.", 53_147, "/mcp").unwrap();
    assert_eq!(url.as_str(), "https://devhost.example.ts.net:53147/mcp");
}

#[test]
fn serve_status_finds_exact_mapping_and_ports_from_both_maps() {
    let status = ServeStatus::parse(SERVE_STATUS).unwrap();
    assert_eq!(
        status.backend_for("devhost.example.ts.net", 53_147),
        Some("http://127.0.0.1:38417")
    );
    assert!(status.occupied_ports().contains(&52_177));
    assert!(status.occupied_ports().contains(&53_147));
}

#[test]
fn serve_status_finds_foreground_mapping_and_marks_its_port_occupied() {
    let status = ServeStatus::parse(
        r#"{
          "Foreground": {
            "session-id": {
              "TCP": {"49287": {"HTTPS": true}},
              "Web": {
                "devhost.example.ts.net:49287": {
                  "Handlers": {"/": {"Proxy": "http://127.0.0.1:49001"}}
                }
              }
            }
          }
        }"#,
    )
    .unwrap();
    assert_eq!(
        status.backend_for("devhost.example.ts.net", 49_287),
        Some("http://127.0.0.1:49001")
    );
    assert!(status.occupied_ports().contains(&49_287));
}

#[test]
fn fixed_and_random_selection_respect_tcp_and_web_collisions() {
    let status = ServeStatus::parse(SERVE_STATUS).unwrap();
    assert!(
        select_port_from_candidates(
            ProxyPortPreference::Fixed(52_177),
            49_152,
            65_535,
            &status,
            [],
            8,
        )
        .is_err()
    );
    assert_eq!(
        select_port_from_candidates(
            ProxyPortPreference::default(),
            49_152,
            65_535,
            &status,
            [52_177, 53_147, 54_000],
            8,
        )
        .unwrap(),
        54_000
    );
}

#[cfg(unix)]
struct FakeTailscale {
    _temp: tempfile::TempDir,
    executable: PathBuf,
    root: PathBuf,
}

#[cfg(unix)]
impl FakeTailscale {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let executable = root.join("tailscale");
        let script = format!(
            r#"#!/usr/bin/env bash
set -u
root='{root}'
printf '%s\n' "$*" >> "$root/invocations"
mapping="$root/mapping"
if [[ "${{1:-}} ${{2:-}}" == "status --json" ]]; then
  printf '%s\n' '{{"BackendState":"Running","Self":{{"Online":true,"DNSName":"devhost.example.ts.net."}}}}'
  exit 0
fi
if [[ "${{1:-}}" == "version" ]]; then printf '%s\n' '1.98.10'; exit 0; fi
if [[ "${{1:-}} ${{2:-}} ${{3:-}}" == "serve status --json" ]]; then
  if [[ -f "$mapping" ]]; then
    IFS='|' read -r port backend < "$mapping"
    if [[ -f "$root/drift_backend" ]]; then backend=$(<"$root/drift_backend"); fi
    printf '{{"TCP":{{"443":{{"HTTPS":true}}}},"Web":{{"devhost.example.ts.net:443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:8765"}}}}}},"devhost.example.ts.net:%s":{{"Handlers":{{"/":{{"Proxy":"%s"}}}}}}}}}}\n' "$port" "$backend"
  else
    printf '%s\n' '{{"TCP":{{"443":{{"HTTPS":true}}}},"Web":{{"devhost.example.ts.net:443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:8765"}}}}}}}}}}'
  fi
  exit 0
fi
if [[ "${{1:-}}" == "serve" ]]; then
  port="${{3#--https=}}"
  if [[ "${{4:-}}" == "off" ]]; then rm -f "$mapping"; exit 0; fi
  backend="${{4:-}}"
  if [[ -f "$root/collision_ports" ]] && grep -qx "$port" "$root/collision_ports"; then
    printf '%s\n' 'port already configured' >&2
    exit 1
  fi
  if [[ -f "$root/exit_early" ]]; then exit 23; fi
  printf '%s|%s\n' "$port" "$backend" > "$mapping"
  trap 'if [[ ! -f "$root/sticky" ]]; then rm -f "$mapping"; fi; exit 0' TERM INT
  while :; do printf 'serve stdout\n'; printf 'serve stderr\n' >&2; sleep 0.02; done
fi
exit 2
"#,
            root = root.display()
        );
        let staged_executable = root.join("tailscale.staged");
        fs::write(&staged_executable, script).unwrap();
        fs::set_permissions(&staged_executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&staged_executable, &executable).unwrap();
        Self {
            _temp: temp,
            executable,
            root,
        }
    }

    fn options(&self, candidates: Vec<u16>) -> TailscaleServeOptions {
        TailscaleServeOptions {
            executable: self.executable.clone(),
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 38_417),
            path: "/mcp".to_string(),
            port: ProxyPortPreference::default(),
            port_range_start: 49_152,
            port_range_end: 65_535,
            candidate_ports: candidates,
            max_attempts: 4,
            poll_interval: Duration::from_millis(10),
            readiness_timeout: Duration::from_secs(2),
        }
    }

    fn touch(&self, name: &str) {
        fs::write(self.root.join(name), "1\n").unwrap();
    }

    fn invocations(&self) -> String {
        fs::read_to_string(self.root.join("invocations")).unwrap_or_default()
    }
}

#[cfg(unix)]
fn mapping(root: &Path) -> Option<String> {
    fs::read_to_string(root.join("mapping")).ok()
}

#[cfg(unix)]
#[tokio::test]
async fn foreground_serve_is_ready_only_after_exact_mapping_and_cleans_normally() {
    let fake = FakeTailscale::new();
    let serve = TailscaleServe::start(fake.options(vec![54_000]))
        .await
        .unwrap();
    assert_eq!(serve.external_port(), 54_000);
    assert_eq!(
        serve.public_url().as_str(),
        "https://devhost.example.ts.net:54000/mcp"
    );
    assert_eq!(
        mapping(&fake.root).as_deref(),
        Some("54000|http://127.0.0.1:38417\n")
    );

    serve.shutdown().await.unwrap();
    assert!(mapping(&fake.root).is_none());
    let calls = fake.invocations();
    assert!(calls.lines().any(|call| call == "version"));
    assert!(calls.contains("serve --yes --https=54000 http://127.0.0.1:38417"));
    assert!(!calls.contains(" off"));
    assert!(!calls.contains("reset"));
    let status = ServeStatus::parse(
        &std::process::Command::new(&fake.executable)
            .args(["serve", "status", "--json"])
            .output()
            .unwrap()
            .stdout
            .into_iter()
            .map(char::from)
            .collect::<String>(),
    )
    .unwrap();
    assert_eq!(
        status.backend_for("devhost.example.ts.net", 443),
        Some("http://127.0.0.1:8765")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn plan_exposes_exact_url_before_foreground_claim() {
    let fake = FakeTailscale::new();
    let options = fake.options(vec![54_000]);
    let plan = TailscaleServePlan::prepare(options).await.unwrap();

    assert_eq!(plan.external_port(), 54_000);
    assert_eq!(
        plan.public_url().as_str(),
        "https://devhost.example.ts.net:54000/mcp"
    );
    assert!(
        mapping(&fake.root).is_none(),
        "planning must not claim Serve"
    );

    let serve = plan.claim().await.unwrap();
    assert_eq!(
        mapping(&fake.root).as_deref(),
        Some("54000|http://127.0.0.1:38417\n")
    );
    serve.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn planned_claim_reports_a_real_collision_as_typed_error() {
    let fake = FakeTailscale::new();
    fs::write(fake.root.join("collision_ports"), "54000\n").unwrap();
    let plan = TailscaleServePlan::prepare(fake.options(vec![54_000]))
        .await
        .unwrap();
    assert!(matches!(
        plan.claim_typed().await.unwrap_err(),
        TailscaleClaimError::Collision(_)
    ));
    assert!(mapping(&fake.root).is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn real_serve_collision_retries_a_different_candidate() {
    let fake = FakeTailscale::new();
    fs::write(fake.root.join("collision_ports"), "54000\n").unwrap();
    let serve = TailscaleServe::start(fake.options(vec![54_000, 54_001]))
        .await
        .unwrap();
    assert_eq!(serve.external_port(), 54_001);
    serve.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn foreground_exit_before_verified_mapping_fails_startup() {
    let fake = FakeTailscale::new();
    fake.touch("exit_early");
    let error = TailscaleServe::start(fake.options(vec![54_000]))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exited"));
}

#[cfg(unix)]
#[tokio::test]
async fn sticky_owned_mapping_uses_exact_port_off_fallback() {
    let fake = FakeTailscale::new();
    fake.touch("sticky");
    let serve = TailscaleServe::start(fake.options(vec![54_000]))
        .await
        .unwrap();
    serve.shutdown().await.unwrap();
    assert!(mapping(&fake.root).is_none());
    let calls = fake.invocations();
    assert!(calls.contains("serve --yes --https=54000 off"));
    assert!(!calls.contains("reset"));
}

#[cfg(unix)]
#[tokio::test]
async fn cleanup_refuses_mapping_whose_backend_changed_ownership() {
    let fake = FakeTailscale::new();
    fake.touch("sticky");
    let serve = TailscaleServe::start(fake.options(vec![54_000]))
        .await
        .unwrap();
    fs::write(fake.root.join("drift_backend"), "http://127.0.0.1:49999\n").unwrap();
    let error = serve.shutdown().await.unwrap_err();
    assert!(error.to_string().contains("ownership"));
    assert!(mapping(&fake.root).is_some());
    let calls = fake.invocations();
    assert!(!calls.contains(" off"));
    assert!(!calls.contains("reset"));
}
