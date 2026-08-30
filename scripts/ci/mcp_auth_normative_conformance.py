#!/usr/bin/env python3
"""Resolve and execute focused MCP 2026-07-28 authorization requirements."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "conformance/mcp-auth-normative.json"
COVERAGE = ROOT / "conformance/mcp-auth-coverage-manifest.json"
VENDOR = ROOT / "vendor/rmcp-3.1.0-labby/Cargo.toml"

PACKAGES = {
    "upstream::manager::url_tests::published_metadata_rejects_issuer_not_identical_to_selected_server": "labby-auth",
    "upstream::manager::url_tests::authorization_metadata_candidates_preserve_issuer_path_and_priority": "labby-auth",
    "upstream::store::tests::authorization_state_round_trips_issuer_requirement_and_requested_scopes": "labby-auth",
    "cimd::tests::accepts_the_real_chatgpt_connector_metadata_document": "labby-auth",
    "authorize::tests::dynamically_registered_client_requires_click_consent_showing_redirect_host": "labby-auth",
    "authorize::tests::authorize_validates_redirect_against_cimd_document_and_persists_reference": "labby-auth",
    "cimd::tests::rejects_malformed_or_incomplete_client_metadata_documents": "labby-auth",
    "authorize::tests::authorize_accepts_configured_protected_resource_scopes": "labby-auth",
    "middleware::tests::exact_resource_audience_including_port_and_path_is_enforced": "labby-auth",
    "middleware::tests::expired_access_token_receives_http_401": "labby-auth",
    "metadata::tests::authorization_server_metadata_exposes_lab_endpoints": "labby-auth",
    "metadata::tests::protected_resource_metadata_uses_canonical_mcp_resource_uri": "labby-auth",
    "token::tests::refresh_grant_rotates_local_token_on_success": "labby-auth",
    "middleware::tests::broader_admin_scope_satisfies_read_scope_hierarchy": "labby-auth",
    "middleware::tests::jwt_validation_path_accepts_signed_token_and_writes_context": "labby-auth",
    "metadata::tests::protected_resource_metadata_advertises_configured_scopes_and_resource_path": "labby-auth",
    "upstream::manager::url_tests::callback_issuer_errors_preserve_issuer_mismatch_kind": "labby-auth",
    "at_rest::tests::contextual_ciphertext_rejects_row_transplant": "labby-auth",
    "sqlite::tests::opening_with_a_key_encrypts_legacy_plaintext_provider_tokens": "labby-auth",
    "token::tests::refresh_grant_preserves_original_token_when_upstream_refresh_fails": "labby-auth",
    "token::tests::refresh_grant_replay_rejects_a_different_resource": "labby-auth",
    "authorize::tests::authorize_rejects_mismatched_resource_parameter": "labby-auth",
    "middleware::tests::missing_bearer_token_returns_401_with_www_authenticate": "labby-auth",
    "cimd::tests::rejects_document_whose_client_id_does_not_exactly_match_url": "labby-auth",
    "cimd::tests::absolute_metadata_deadline_includes_single_flight_wait": "labby-auth",
    "upstream::manager::url_tests::malformed_authorization_server_does_not_block_later_valid_entry": "labby-auth",
    "upstream::manager::url_tests::authorization_server_list_is_deduplicated_and_bounded": "labby-auth",
    "upstream::http_client::tests::bearer_scope_parser_rejects_parameter_name_substrings": "labby-gateway",
    "upstream::manager::url_tests::pkce_validation_accepts_advertised_s256": "labby-auth",
    "upstream::manager::url_tests::pkce_validation_rejects_advertised_non_s256_methods": "labby-auth",
    "upstream::manager::url_tests::pkce_validation_rejects_missing_method_metadata": "labby-auth",
    "authorize::tests::register_accepts_public_dcr_and_enforces_loopback_redirects": "labby-auth",
    "authorize::tests::register_rejects_native_callback_endpoint_smuggled_with_an_unsafe_redirect_uri": "labby-auth",
    "authorize::tests::wildcard_redirect_patterns_do_not_overmatch_similar_hosts": "labby-auth",
    "authorize::tests::https_redirects_still_require_the_allowlist": "labby-auth",
    "authorize::tests::callback_rejects_expired_or_mismatched_state": "labby-auth",
    "upstream::store::tests::replayed_state_is_rejected": "labby-auth",
    "upstream::store::tests::expired_state_is_rejected": "labby-auth",
    "upstream::http_client::tests::trusted_private_origin_is_allowed_and_pinned": "labby-auth",
    "upstream::http_client::tests::different_private_origin_is_blocked": "labby-auth",
    "upstream::http_client::tests::redirect_policy_stops_cross_origin": "labby-auth",
    "upstream::pool::connect::conformance_tests::configured_headers_cannot_transit_an_inbound_authorization_token": "labby-gateway",
    "remote::tests::cache_policy_is_case_insensitive_and_bounded": "labby-auth",
    "config::tests::oauth_mode_rejects_non_https_public_url_except_loopback": "labby-auth",
    "upstream::pool::connect_stdio::conformance_tests::stdio_named_environment_credential_reaches_child_environment_without_oauth": "labby-gateway",
    "upstream::pool::connect_stdio::conformance_tests::stdio_parent_environment_is_fail_closed_to_explicit_runtime_allowlist": "labby-gateway",
    "jwt::tests::expired_access_token_is_rejected": "labby-auth",
    "jwt::tests::minted_access_token_round_trips_and_contains_kid": "labby-auth",
    "jwt::tests::validate_with_issuer_rejects_wrong_issuer_via_validation_struct": "labby-auth",
    "middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge": "labby-auth",
    "token::tests::token_endpoint_errors_use_oauth_error_shape": "labby-auth",
    "authorize::tests::oauth_client_callback_succeeds_for_allowlisted_non_admin_email": "labby-auth",
    "authorize::tests::oauth_client_callback_redirects_with_access_denied_when_email_not_in_allowlist": "labby-auth",
    "authorize::tests::omitted_initial_scope_defaults_to_least_privilege_read_only_scope": "labby-auth",
    "authorize::tests::authorization_endpoint_requires_code_flow_and_pkce_s256": "labby-auth",
    "authorize::tests::authorize_rejects_nonidentical_registered_redirect_without_redirecting": "labby-auth",
    "authorize::tests::localhost_redirect_consent_warns_with_exact_loopback_host": "labby-auth",
    "authorize::response::tests::successful_authorization_response_uses_exact_metadata_issuer": "labby-auth",
    "authorize::response::tests::error_authorization_response_uses_exact_metadata_issuer": "labby-auth",
    "token::tests::token_endpoint_mints_lab_jwt_and_refresh_token": "labby-auth",
}
VENDOR_TESTS = {
    "transport::auth::tests::select_scopes_unions_challenge_with_previously_requested",
    "transport::auth::tests::dcr_registration_declares_application_type_and_authorization_code_refresh_grants",
    "transport::auth::tests::initial_scope_selection_prefers_challenge_then_resource_metadata_then_omission",
    "transport::auth::tests::authorization_metadata_issuer_comparison_is_exact",
    "transport::auth::tests::authorization_manager_can_attempt_scope_upgrade_respects_config",
    "transport::auth::tests::resolve_metadata_from_challenge_uses_challenge_pointer_and_scope",
    "transport::auth::tests::protected_resource_metadata_supports_custom_location_and_oidc_path_append",
    "transport::auth::tests::protected_resource_metadata_supports_authorization_server_path_insertion",
    "transport::auth::tests::authorization_metadata_accepts_oidc_path_inserted_issuer",
    "transport::auth::tests::path_bearing_authorization_server_discovery_uses_required_order_and_stops",
    "transport::auth::tests::authorization_metadata_accepts_oidc_path_appended_issuer",
    "transport::auth::tests::preregistered_client_takes_priority_over_cimd",
    "transport::auth::tests::preregistered_client_skips_registration_endpoint",
    "transport::auth::tests::initialize_from_store_clears_dcr_credentials_when_issuer_changes",
    "transport::auth::tests::issuer_change_surfaces_mismatch_then_reregisters_with_new_server",
    "transport::auth::tests::validate_authorization_response_issuer_rejects_invalid_cases",
    "transport::auth::tests::authorization_url_stores_expected_issuer_for_callback_validation",
    "transport::auth::tests::issuer_mismatch_is_rejected_before_any_token_endpoint_request",
    "transport::auth::tests::native_dcr_registration_sends_native_application_type",
    "transport::auth::tests::dcr_takes_priority_over_user_entered_client_information",
    "transport::auth::tests::user_entered_client_is_final_fallback_after_cimd_and_dcr_are_unavailable",
    "transport::auth::tests::authorization_url_uses_discovered_resource",
    "transport::auth::tests::authorization_url_uses_default_resource_without_protected_resource_document",
    "transport::auth::tests::refresh_token_includes_resource_parameter",
    "transport::auth::tests::authorized_http_client_uses_bearer_header_and_never_query_token",
    "transport::auth::tests::cimd_used_when_server_advertises_support",
    "transport::auth::tests::cimd_falls_back_to_dcr_when_server_lacks_support",
    "transport::auth::tests::configure_client_credentials_uses_request_body_auth_for_client_secret",
    "transport::auth::tests::select_scopes_deduplicates_challenge_already_requested",
    "transport::auth::tests::get_access_token_requires_reauth_when_expired_and_no_refresh_token",
    "transport::auth::tests::authorization_manager_tracks_scope_upgrade_attempts",
    "transport::auth::tests::scope_upgrade_attempt_counter_tracks_requests_and_stops_at_limit",
    "transport::auth::tests::dcr_recovers_unauthorized_state_after_registration_failure",
    "transport::auth::tests::resource_identifier_matching_allows_matching_host_or_parent_path",
    "transport::auth::tests::authorization_callback_rejects_untrusted_error_fields_without_echoing_them",
    "transport::auth::tests::initialize_from_store_rejects_token_without_current_issuer_binding",
    "transport::auth::tests::select_scopes_does_not_expand_user_request_to_entire_server_catalog",
    "transport::auth::tests::select_scopes_prefers_exact_challenge_over_resource_metadata_catalog",
    "transport::auth::tests::refresh_token_uses_discovered_protected_resource",
    "transport::auth::tests::extract_www_authenticate_params_insufficient_scope",
    "transport::auth::tests::scope_upgrade_adds_offline_access_when_as_supports_it",
}

NORMATIVE_ACTORS = {
    "HTTP MCP implementation",
    "STDIO MCP implementation",
    "alternative-transport MCP implementation",
    "MCP implementation",
    "MCP client",
    "MCP client and resource server",
    "MCP client using CIMD",
    "MCP client using DCR",
    "MCP client hosting a Client ID Metadata Document",
    "native MCP client using DCR",
    "web MCP client using DCR",
    "MCP server",
    "MCP gateway",
    "MCP authorization server",
    "authorization server",
    "authorization server accepting CIMD",
}


def rows() -> dict[str, dict]:
    data = json.loads(MATRIX.read_text())
    return {row["id"]: row for row in data["requirements"]}


def validate_manifest(matrix_rows: dict[str, dict]) -> None:
    """Fail closed when clause provenance or test resolution drifts.

    This validates the fixed denominator independently of executing tests: every
    row is actor-bound, every applicable disposition resolves to at least one
    exact test, every evidence path exists, and the coverage projection is an
    exact copy of the executable matrix rather than a second hand-maintained
    claim.
    """
    coverage = json.loads(COVERAGE.read_text())
    if coverage.get("protocol_version") != "2026-07-28":
        raise SystemExit("coverage manifest has the wrong protocol version")
    coverage_rows = {row["row_id"]: row for row in coverage["coverage"]}
    if set(coverage_rows) != set(matrix_rows):
        raise SystemExit("coverage manifest denominator differs from normative matrix")
    malformed_suffixes = (
        " and", " or", "**MUST**.", "**SHOULD** to", "e.g.", "* Clients",
        "* The `client_id` URL", " *",
    )
    known_tests = set(PACKAGES) | VENDOR_TESTS
    reuse = Counter(
        test_id
        for row in matrix_rows.values()
        for test_id in row_test_ids(row)
    )
    overused = {test_id: count for test_id, count in reuse.items() if count > 10}
    if overused:
        raise SystemExit(f"MCP assertion tests are reused too broadly: {overused}")
    def validate_subordinates(row_id: str, path: tuple[str, ...] = ()) -> None:
        if row_id in path:
            raise SystemExit(f"{row_id}: aggregate evidence cycle: {' -> '.join((*path, row_id))}")
        for subordinate_id in matrix_rows[row_id].get("subordinate_row_ids", []):
            if subordinate_id not in matrix_rows:
                raise SystemExit(f"{row_id}: unknown subordinate row {subordinate_id}")
            if subordinate_id == row_id:
                raise SystemExit(f"{row_id}: aggregate evidence cannot reference itself")
            validate_subordinates(subordinate_id, (*path, row_id))

    for row_id, row in matrix_rows.items():
        requirement = row.get("requirement", "").strip()
        if not requirement or requirement.endswith(malformed_suffixes):
            raise SystemExit(f"{row_id}: malformed or context-free requirement")
        if not row.get("actor"):
            raise SystemExit(f"{row_id}: missing normative actor")
        if row.get("actor") not in NORMATIVE_ACTORS:
            raise SystemExit(f"{row_id}: actor is not a closed normative role: {row.get('actor')!r}")
        expected_hash = hashlib.sha256(requirement.encode()).hexdigest()
        if row.get("source_requirement_sha256") != expected_hash:
            raise SystemExit(f"{row_id}: requirement provenance hash mismatch")
        if any(not (ROOT / path).exists() for path in row.get("evidence_paths", [])):
            raise SystemExit(f"{row_id}: evidence path does not exist")
        tests = row_test_ids(row)
        subordinate_ids = row.get("subordinate_row_ids", [])
        if len(subordinate_ids) != len(set(subordinate_ids)):
            raise SystemExit(f"{row_id}: duplicate subordinate row")
        if tests and subordinate_ids:
            raise SystemExit(f"{row_id}: direct and aggregate evidence cannot be mixed")
        validate_subordinates(row_id)
        evidence = row.get("assertion_evidence", [])
        if [entry.get("test_id") for entry in evidence] != tests:
            raise SystemExit(f"{row_id}: assertion evidence does not exactly match tests")
        if any(not entry.get("behavior", "").strip() for entry in evidence):
            raise SystemExit(f"{row_id}: assertion evidence lacks exact behavior")
        if any(entry.get("behavior", "").strip() == row.get("asserted_obligation", "").strip() for entry in evidence):
            raise SystemExit(f"{row_id}: assertion evidence merely repeats the obligation")
        if any("**MUST" in entry.get("behavior", "") or "**SHOULD" in entry.get("behavior", "") for entry in evidence):
            raise SystemExit(f"{row_id}: assertion evidence contains copied normative prose")
        if row["applicability"] == "applicable":
            if row.get("status") != "pass" or (not tests and not subordinate_ids):
                raise SystemExit(f"{row_id}: applicable row lacks direct or aggregate evidence")
            unresolved = set(tests) - known_tests
            if unresolved:
                raise SystemExit(f"{row_id}: unresolved tests: {sorted(unresolved)}")
            if subordinate_ids and any(
                matrix_rows[subordinate_id].get("status") not in {"pass", "not_applicable"}
                for subordinate_id in subordinate_ids
            ):
                raise SystemExit(f"{row_id}: aggregate evidence includes unresolved disposition")
        elif row.get("status") != "not_applicable" or tests:
            raise SystemExit(f"{row_id}: N/A row must have no executable pass claim")
        projected = coverage_rows[row_id]
        for key in (
            "source_url", "source_requirement_sha256", "actor",
            "asserted_obligation", "assertion_test_ids", "implementation",
            "assertion_evidence", "subordinate_row_ids", "evidence_paths", "applicability", "status",
        ):
            if projected.get(key) != row.get(key):
                raise SystemExit(f"{row_id}: coverage projection drift in {key}")


def run(command: list[str]) -> None:
    subprocess.run(command, cwd=ROOT, env=os.environ.copy(), check=True)


def run_one(test_id: str) -> None:
    if test_id in VENDOR_TESTS:
        run([
            "cargo", "test", "--manifest-path", str(VENDOR), "--lib",
            "--features", "auth,client", test_id, "--", "--exact",
        ])
        return
    package = PACKAGES[test_id]
    run(["cargo", "test", "-p", package, "--all-features", "--locked", "--lib", test_id, "--", "--exact"])


def row_test_ids(row: dict) -> list[str]:
    return row.get("assertion_test_ids", [row["test_id"]])


def resolved_test_ids(row_id: str, matrix_rows: dict[str, dict]) -> list[str]:
    row = matrix_rows[row_id]
    tests = list(row_test_ids(row))
    for subordinate_id in row.get("subordinate_row_ids", []):
        tests.extend(resolved_test_ids(subordinate_id, matrix_rows))
    return list(dict.fromkeys(tests))


def exact_expression(test_ids: list[str]) -> str:
    return " | ".join(f"test(={test_id})" for test_id in test_ids)


def run_all(matrix_rows: dict[str, dict]) -> None:
    validate_manifest(matrix_rows)
    unique = sorted({test_id for row in matrix_rows.values() for test_id in row_test_ids(row)})
    repo = [test_id for test_id in unique if test_id in PACKAGES]
    vendor = [test_id for test_id in unique if test_id in VENDOR_TESTS]
    unknown = set(unique) - set(repo) - set(vendor)
    if unknown:
        raise SystemExit(f"unresolved MCP auth tests: {sorted(unknown)}")
    run([
        "cargo", "nextest", "run", "-p", "labby-auth", "-p", "labby-gateway",
        "--all-features", "--locked", "--lib", "--test-threads", "1",
        "-E", exact_expression(repo),
    ])
    run([
        "cargo", "nextest", "run", "--manifest-path", str(VENDOR), "--lib",
        "--features", "auth,client", "-E", exact_expression(vendor),
    ])


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("row_id", nargs="?")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--resolve")
    parser.add_argument("--validate-only", action="store_true")
    args = parser.parse_args()
    matrix_rows = rows()
    if args.validate_only:
        validate_manifest(matrix_rows)
        print(f"validated {len(matrix_rows)} actor-bound MCP authorization clauses")
        return
    if args.list:
        print("\n".join(matrix_rows))
        return
    if args.resolve:
        print(json.dumps(resolved_test_ids(args.resolve, matrix_rows)))
        return
    if args.row_id:
        validate_manifest(matrix_rows)
        for test_id in resolved_test_ids(args.row_id, matrix_rows):
            run_one(test_id)
        return
    run_all(matrix_rows)


if __name__ == "__main__":
    main()
