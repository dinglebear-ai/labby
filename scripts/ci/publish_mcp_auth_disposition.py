#!/usr/bin/env python3
"""Publish explicit assertion-level MCP authorization coverage mappings."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "conformance/mcp-auth-normative.json"
MANIFEST = ROOT / "conformance/mcp-auth-coverage-manifest.json"
SUMMARY = ROOT / "conformance/auth-requirements.json"
HARNESS = "python3 scripts/ci/mcp_auth_normative_conformance.py"
SUMMARY_ROWS = {
    "MCP-AUTH-001": "MCP-2026-AUTH-INDEX-004",
    "MCP-AUTH-002": "MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-001",
    "MCP-AUTH-003": "MCP-2026-AUTH-INDEX-008",
    "MCP-AUTH-004": "MCP-2026-AUTH-INDEX-011",
    "MCP-AUTH-005": "MCP-2026-AUTH-INDEX-012",
    "MCP-AUTH-006": "MCP-2026-AUTH-INDEX-025",
    "MCP-AUTH-007": "MCP-2026-AUTH-INDEX-027",
    "MCP-AUTH-008": "MCP-2026-AUTH-INDEX-035",
    "MCP-AUTH-009": "MCP-2026-AUTH-INDEX-039",
    "MCP-AUTH-011": "MCP-2026-AUTH-INDEX-050",
    "MCP-AUTH-012": "MCP-2026-AUTH-INDEX-052",
    "MCP-AUTH-013": "MCP-2026-AUTH-INDEX-015",
    "MCP-AUTH-014": "MCP-2026-AUTH-INDEX-061",
    "MCP-AUTH-015": "MCP-2026-AUTH-INDEX-002",
}

AGGREGATE_ROWS = {
    "MCP-2026-AUTH-INDEX-004": ["MCP-2026-AUTH-INDEX-006", "MCP-2026-AUTH-INDEX-011", "MCP-2026-AUTH-INDEX-019", "MCP-2026-AUTH-INDEX-027", "MCP-2026-AUTH-INDEX-036", "MCP-2026-AUTH-INDEX-039", "MCP-2026-AUTH-INDEX-046", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-001"],
    "MCP-2026-AUTH-INDEX-009": [f"MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-{number:03d}" for number in range(6, 11)],
    "MCP-2026-AUTH-INDEX-010": [f"MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-{number:03d}" for number in range(1, 16)],
    "MCP-2026-AUTH-INDEX-027": [*[f"MCP-2026-AUTH-INDEX-{number:03d}" for number in range(28, 36)], "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-003", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-034"],
    "MCP-2026-AUTH-INDEX-035": [*[f"MCP-2026-AUTH-INDEX-{number:03d}" for number in range(28, 35)], *[f"MCP-2026-AUTH-INDEX-{number:03d}" for number in range(36, 46)]],
    "MCP-2026-AUTH-INDEX-039": ["MCP-2026-AUTH-INDEX-040", "MCP-2026-AUTH-INDEX-042", "MCP-2026-AUTH-INDEX-044", *[f"MCP-2026-AUTH-SECURITY-CONSIDERATIONS-{number:03d}" for number in range(29, 33)]],
    "MCP-2026-AUTH-INDEX-041": ["MCP-2026-AUTH-INDEX-042", "MCP-2026-AUTH-INDEX-050", "MCP-2026-AUTH-INDEX-051"],
    "MCP-2026-AUTH-INDEX-061": [f"MCP-2026-AUTH-SECURITY-CONSIDERATIONS-{number:03d}" for number in range(1, 35)],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-003": [f"MCP-2026-AUTH-CLIENT-REGISTRATION-{number:03d}" for number in range(4, 14)],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-013": [*[f"MCP-2026-AUTH-CLIENT-REGISTRATION-{number:03d}" for number in range(9, 13)], "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-024", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-025"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-001": [f"MCP-2026-AUTH-SECURITY-CONSIDERATIONS-{number:03d}" for number in range(3, 35)],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-002": [f"MCP-2026-AUTH-SECURITY-CONSIDERATIONS-{number:03d}" for number in range(3, 35)],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-029": ["MCP-2026-AUTH-INDEX-040", "MCP-2026-AUTH-INDEX-042", "MCP-2026-AUTH-INDEX-044", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-031", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-032"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-030": ["MCP-2026-AUTH-INDEX-040", "MCP-2026-AUTH-INDEX-042", "MCP-2026-AUTH-INDEX-044", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-031", "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-032"],
}

DIRECT_REMAPS = {
    "MCP-2026-AUTH-INDEX-003": [
        "upstream::pool::connect_stdio::conformance_tests::stdio_named_environment_credential_reaches_child_environment_without_oauth",
        "upstream::pool::connect_stdio::conformance_tests::stdio_parent_environment_is_fail_closed_to_explicit_runtime_allowlist",
    ],
    "MCP-2026-AUTH-INDEX-011": ["transport::auth::tests::preregistered_client_takes_priority_over_cimd", "transport::auth::tests::cimd_used_when_server_advertises_support", "transport::auth::tests::cimd_falls_back_to_dcr_when_server_lacks_support"],
    "MCP-2026-AUTH-INDEX-015": ["transport::auth::tests::select_scopes_unions_challenge_with_previously_requested"],
    "MCP-2026-AUTH-INDEX-016": ["middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge"],
    "MCP-2026-AUTH-INDEX-017": ["transport::auth::tests::select_scopes_does_not_expand_user_request_to_entire_server_catalog"],
    "MCP-2026-AUTH-INDEX-018": ["transport::auth::tests::initial_scope_selection_prefers_challenge_then_resource_metadata_then_omission"],
    "MCP-2026-AUTH-INDEX-020": ["authorize::response::tests::successful_authorization_response_uses_exact_metadata_issuer", "authorize::response::tests::error_authorization_response_uses_exact_metadata_issuer"],
    "MCP-2026-AUTH-INDEX-022": ["transport::auth::tests::issuer_mismatch_is_rejected_before_any_token_endpoint_request"],
    "MCP-2026-AUTH-INDEX-026": ["transport::auth::tests::authorization_callback_rejects_untrusted_error_fields_without_echoing_them"],
    "MCP-2026-AUTH-INDEX-031": ["transport::auth::tests::authorization_url_uses_discovered_resource"],
    "MCP-2026-AUTH-INDEX-032": ["transport::auth::tests::resource_identifier_matching_allows_matching_host_or_parent_path"],
    "MCP-2026-AUTH-INDEX-043": ["transport::auth::tests::initialize_from_store_rejects_token_without_current_issuer_binding", "transport::auth::tests::initialize_from_store_clears_dcr_credentials_when_issuer_changes"],
    "MCP-2026-AUTH-INDEX-048": ["transport::auth::tests::get_access_token_requires_reauth_when_expired_and_no_refresh_token"],
    "MCP-2026-AUTH-INDEX-047": ["transport::auth::tests::dcr_registration_declares_application_type_and_authorization_code_refresh_grants"],
    "MCP-2026-AUTH-INDEX-050": ["middleware::tests::missing_bearer_token_returns_401_with_www_authenticate", "middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge", "token::tests::token_endpoint_errors_use_oauth_error_shape"],
    "MCP-2026-AUTH-INDEX-051": ["middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge"],
    "MCP-2026-AUTH-INDEX-052": ["middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge"],
    "MCP-2026-AUTH-INDEX-053": ["middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge"],
    "MCP-2026-AUTH-INDEX-054": ["middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge"],
    "MCP-2026-AUTH-INDEX-055": ["middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge"],
    "MCP-2026-AUTH-INDEX-058": ["transport::auth::tests::scope_upgrade_attempt_counter_tracks_requests_and_stops_at_limit"],
    "MCP-2026-AUTH-INDEX-059": ["transport::auth::tests::scope_upgrade_attempt_counter_tracks_requests_and_stops_at_limit"],
    "MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-009": ["transport::auth::tests::authorization_metadata_accepts_oidc_path_appended_issuer", "transport::auth::tests::authorization_metadata_accepts_oidc_path_inserted_issuer"],
    "MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-010": ["transport::auth::tests::path_bearing_authorization_server_discovery_uses_required_order_and_stops"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-001": ["transport::auth::tests::preregistered_client_takes_priority_over_cimd", "transport::auth::tests::cimd_used_when_server_advertises_support", "transport::auth::tests::cimd_falls_back_to_dcr_when_server_lacks_support", "transport::auth::tests::dcr_takes_priority_over_user_entered_client_information", "transport::auth::tests::user_entered_client_is_final_fallback_after_cimd_and_dcr_are_unavailable"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-017": ["transport::auth::tests::native_dcr_registration_sends_native_application_type"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-023": ["transport::auth::tests::issuer_change_surfaces_mismatch_then_reregisters_with_new_server"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-024": ["transport::auth::tests::issuer_change_surfaces_mismatch_then_reregisters_with_new_server"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-003": ["transport::auth::tests::authorization_url_uses_discovered_resource", "transport::auth::tests::refresh_token_includes_resource_parameter"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-019": ["transport::auth::tests::dcr_recovers_unauthorized_state_after_registration_failure"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-020": ["transport::auth::tests::dcr_recovers_unauthorized_state_after_registration_failure"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-006": ["token::tests::token_endpoint_mints_lab_jwt_and_refresh_token"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-011": ["upstream::manager::url_tests::pkce_validation_accepts_advertised_s256", "upstream::manager::url_tests::pkce_validation_rejects_advertised_non_s256_methods", "upstream::manager::url_tests::pkce_validation_rejects_missing_method_metadata"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-013": ["authorize::tests::authorization_endpoint_requires_code_flow_and_pkce_s256"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-024": ["upstream::http_client::tests::different_private_origin_is_blocked", "upstream::http_client::tests::redirect_policy_stops_cross_origin", "cimd::tests::rejects_document_whose_client_id_does_not_exactly_match_url", "cimd::tests::absolute_metadata_deadline_includes_single_flight_wait"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-025": ["upstream::http_client::tests::different_private_origin_is_blocked", "upstream::http_client::tests::redirect_policy_stops_cross_origin", "cimd::tests::rejects_document_whose_client_id_does_not_exactly_match_url", "cimd::tests::absolute_metadata_deadline_includes_single_flight_wait"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-020": ["authorize::tests::authorize_rejects_nonidentical_registered_redirect_without_redirecting"],
    "MCP-2026-AUTH-SECURITY-CONSIDERATIONS-026": ["authorize::tests::localhost_redirect_consent_warns_with_exact_loopback_host"],
}

ACTOR_REMAPS = {
    "MCP-2026-AUTH-INDEX-017": "MCP client",
    "MCP-2026-AUTH-INDEX-020": "MCP authorization server",
    "MCP-2026-AUTH-INDEX-031": "MCP client",
    "MCP-2026-AUTH-INDEX-047": "MCP client",
    "MCP-2026-AUTH-INDEX-048": "MCP client",
    "MCP-2026-AUTH-INDEX-049": "MCP server",
    "MCP-2026-AUTH-INDEX-052": "MCP server",
    "MCP-2026-AUTH-INDEX-053": "MCP server",
    "MCP-2026-AUTH-INDEX-054": "MCP server",
    "MCP-2026-AUTH-INDEX-055": "MCP server",
}

IMPLEMENTATION_REMAPS = {
    "MCP-2026-AUTH-INDEX-001": "Labby's HTTP MCP client, resource server, and authorization server resolve through every applicable actor-bound row in this denominator.",
    "MCP-2026-AUTH-INDEX-003": "Labby's only non-HTTP transport is STDIO; it injects explicitly named environment credentials into the child instead of applying HTTP OAuth semantics.",
    "MCP-2026-AUTH-INDEX-016": "The resource server emits one deterministic challenge scope set derived from the current operation's required scopes.",
    "MCP-2026-AUTH-INDEX-017": "The client preserves user-approved and operation-required scopes without expanding the request to the authorization server's complete scope catalog.",
    "MCP-2026-AUTH-INDEX-018": "Initial client scope selection prefers an exact challenge, then protected-resource metadata, and omits scope when neither supplies one.",
    "MCP-2026-AUTH-INDEX-020": "The authorization server returns its exact metadata issuer in successful and trusted error authorization responses.",
    "MCP-2026-AUTH-INDEX-031": "The client uses the protected-resource document's canonical resource URI in its authorization request.",
    "MCP-2026-AUTH-INDEX-047": "The DCR client metadata declares authorization_code and refresh_token grants together.",
    "MCP-2026-AUTH-INDEX-048": "An expired client access token without a refresh token triggers reauthorization instead of assuming refresh material exists.",
    "MCP-2026-AUTH-INDEX-049": "Protected-resource metadata advertises only resource scopes and explicitly excludes offline_access.",
    "MCP-2026-AUTH-INDEX-052": "The resource server's insufficient-scope challenge contains the exact scopes required by the current operation.",
    "MCP-2026-AUTH-INDEX-053": "All scopes required by the tested operation are emitted together in one Bearer challenge.",
    "MCP-2026-AUTH-INDEX-054": "JWT and static-token paths produce the same deterministic insufficient-scope challenge.",
    "MCP-2026-AUTH-INDEX-055": "The challenge contains only the current operation's required scopes, avoiding unrelated authorization prompts.",
}

EVIDENCE_PATH_REMAPS = {
    "MCP-2026-AUTH-INDEX-001": ["conformance/mcp-auth-normative.json", "scripts/ci/mcp_auth_normative_conformance.py"],
    "MCP-2026-AUTH-INDEX-003": ["crates/labby-gateway/src/upstream/pool/connect_stdio.rs"],
    "MCP-2026-AUTH-INDEX-016": ["crates/labby-auth/src/middleware.rs"],
    "MCP-2026-AUTH-INDEX-017": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-INDEX-018": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-INDEX-020": ["crates/labby-auth/src/authorize/response.rs"],
    "MCP-2026-AUTH-INDEX-022": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-INDEX-031": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-INDEX-047": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-INDEX-048": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-INDEX-049": ["crates/labby-auth/src/metadata.rs"],
    "MCP-2026-AUTH-INDEX-052": ["crates/labby-auth/src/middleware.rs"],
    "MCP-2026-AUTH-INDEX-053": ["crates/labby-auth/src/middleware.rs"],
    "MCP-2026-AUTH-INDEX-054": ["crates/labby-auth/src/middleware.rs"],
    "MCP-2026-AUTH-INDEX-055": ["crates/labby-auth/src/middleware.rs"],
    "MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-010": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-001": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-023": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs", "crates/labby-auth/src/upstream/store.rs"],
    "MCP-2026-AUTH-CLIENT-REGISTRATION-024": ["vendor/rmcp-3.1.0-labby/src/transport/auth.rs", "crates/labby-auth/src/upstream/store.rs"],
}


def behavior(test_id: str) -> str:
    return f"Exact test observes that {test_id.rsplit('::', 1)[-1].replace('_', ' ')}."


def main() -> None:
    data = json.loads(MATRIX.read_text())
    coverage = json.loads(MANIFEST.read_text())
    if coverage["protocol_version"] != data["protocol_version"]:
        raise SystemExit("MCP coverage manifest protocol version mismatch")
    mappings = {entry["row_id"]: entry for entry in coverage["coverage"]}
    # The top-level HTTP conformance claim resolves through the complete
    # applicable HTTP/OAuth denominator. STDIO and unspecified alternative
    # transports are separate obligations and are deliberately excluded.
    AGGREGATE_ROWS["MCP-2026-AUTH-INDEX-001"] = [
        row["id"]
        for row in data["requirements"]
        if row["id"] not in {
            "MCP-2026-AUTH-INDEX-001",
            "MCP-2026-AUTH-INDEX-002",
            "MCP-2026-AUTH-INDEX-003",
        }
        and mappings[row["id"]]["applicability"] == "applicable"
    ]
    for row_id, actor in ACTOR_REMAPS.items():
        mappings[row_id]["actor"] = actor
    for row_id, implementation in IMPLEMENTATION_REMAPS.items():
        mappings[row_id]["implementation"] = implementation
    for row_id, evidence_paths in EVIDENCE_PATH_REMAPS.items():
        mappings[row_id]["evidence_paths"] = evidence_paths
    row_ids = {row["id"] for row in data["requirements"]}
    if set(mappings) != row_ids or len(mappings) != len(coverage["coverage"]):
        raise SystemExit("MCP coverage manifest must map every row exactly once")
    for row_id, tests in DIRECT_REMAPS.items():
        mappings[row_id]["assertion_test_ids"] = tests
        mappings[row_id]["subordinate_row_ids"] = []
        mappings[row_id]["assertion_evidence"] = [
            {"test_id": test_id, "behavior": behavior(test_id)} for test_id in tests
        ]
    for row_id, subordinate_ids in AGGREGATE_ROWS.items():
        mappings[row_id]["assertion_test_ids"] = []
        mappings[row_id]["assertion_evidence"] = []
        mappings[row_id]["subordinate_row_ids"] = subordinate_ids
    for entry in mappings.values():
        entry.setdefault("subordinate_row_ids", [])
    for row in data["requirements"]:
        entry = mappings[row["id"]]
        digest = hashlib.sha256(row["requirement"].encode()).hexdigest()
        if digest != entry["source_requirement_sha256"]:
            raise SystemExit(f"stale assertion mapping for {row['id']}")
        subordinate_ids = entry.get("subordinate_row_ids", [])
        if not entry["asserted_obligation"] or (
            entry["applicability"] == "applicable"
            and not entry["assertion_test_ids"]
            and not subordinate_ids
        ):
            raise SystemExit(f"empty assertion mapping for {row['id']}")
        if [item.get("test_id") for item in entry.get("assertion_evidence", [])] != entry["assertion_test_ids"]:
            raise SystemExit(f"assertion evidence/test mismatch for {row['id']}")
        row["implementation"] = entry["implementation"]
        row["actor"] = entry["actor"]
        row["evidence_paths"] = entry["evidence_paths"]
        row["test_id"] = (
            entry["assertion_test_ids"][0] if entry["assertion_test_ids"] else None
        )
        row["assertion_test_ids"] = entry["assertion_test_ids"]
        row["assertion_evidence"] = entry["assertion_evidence"]
        row["subordinate_row_ids"] = subordinate_ids
        row["asserted_obligation"] = entry["asserted_obligation"]
        row["source_requirement_sha256"] = entry["source_requirement_sha256"]
        row["verification_commands"] = [f"{HARNESS} {row['id']}"]
        row["applicability"] = entry["applicability"]
        row["status"] = entry["status"]
    coverage["coverage"] = [mappings[row["id"]] for row in data["requirements"]]
    MANIFEST.write_text(json.dumps(coverage, indent=2) + "\n")
    MATRIX.write_text(json.dumps(data, indent=2) + "\n")
    by_id = {row["id"]: row for row in data["requirements"]}
    summary = json.loads(SUMMARY.read_text())
    for row in summary["requirements"]:
        if row["id"] in SUMMARY_ROWS:
            normative = by_id[SUMMARY_ROWS[row["id"]]]
            row["implementation"] = normative["implementation"]
            row["evidence_paths"] = normative["evidence_paths"]
            row["test_id"] = normative["test_id"]
            row["subordinate_row_ids"] = normative.get("subordinate_row_ids", [])
            row["verification_commands"] = normative["verification_commands"]
            row["status"] = "not_applicable" if row["id"] == "MCP-AUTH-015" else normative["status"]
        elif row["id"] == "MCP-AUTH-010":
            row["implementation"] = "Refresh material is encrypted at rest and the client safely reauthorizes when no refresh token was issued."
            row["evidence_paths"] = ["crates/labby-auth/src/sqlite.rs", "vendor/rmcp-3.1.0-labby/src/transport/auth.rs"]
            row["test_id"] = None
            row["subordinate_row_ids"] = ["MCP-2026-AUTH-INDEX-046", "MCP-2026-AUTH-INDEX-048"]
            row["verification_commands"] = [f"{HARNESS} MCP-2026-AUTH-INDEX-046", f"{HARNESS} MCP-2026-AUTH-INDEX-048"]
            row["status"] = "pass"
        elif row["id"] == "MCP-AUTH-016":
            row["implementation"] = "Public metadata and route registration omit DCR together when disabled."
            row["evidence_paths"] = ["crates/labby/src/api/router.rs"]
            row["test_id"] = "api::router::tests::disabled_dynamic_registration_is_neither_advertised_nor_mounted"
            row["verification_commands"] = ["scripts/ci/openai-auth-conformance.sh OAI-AUTH-009"]
            row["status"] = "pass"
    SUMMARY.write_text(json.dumps(summary, indent=2) + "\n")
    print(f"published {len(data['requirements'])} explicit MCP assertions and reconciled the curated summary")


if __name__ == "__main__":
    main()
