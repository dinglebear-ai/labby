#!/usr/bin/env bash
set -euo pipefail

readonly IDS=(
  OAI-AUTH-001 OAI-AUTH-002 OAI-AUTH-003 OAI-AUTH-004 OAI-AUTH-005
  OAI-AUTH-006 OAI-AUTH-007 OAI-AUTH-008 OAI-AUTH-009 OAI-AUTH-010
)

run_test() {
  local package=$1
  local filter=$2
  cargo test -p "$package" --all-features --locked --lib "$filter" -- --exact
}

run_requirement() {
  case "$1" in
    OAI-AUTH-001)
      run_test labby-auth metadata::tests::protected_resource_metadata_uses_canonical_mcp_resource_uri
      ;;
    OAI-AUTH-002)
      run_test labby mcp::permanent_tools::tests::builtin_service_tool_advertises_top_level_security_schemes
      run_test labby mcp::permanent_tools::tests::protected_boundary_rebinds_upstream_descriptor_security_policy
      run_test labby mcp::handlers_tools::tests::raw_mode_preserves_upstream_annotations_verbatim_on_both_listing_paths
      ;;
    OAI-AUTH-003)
      run_test labby mcp::result_format::tests::auth_errors_carry_mcp_reauthentication_metadata
      run_test labby mcp::result_format::tests::forbidden_and_non_auth_errors_publish_only_applicable_challenges
      ;;
    OAI-AUTH-004)
      run_test labby mcp::permanent_tools::tests::builtin_service_tool_mirrors_security_schemes_in_legacy_meta
      ;;
    OAI-AUTH-005)
      run_test labby-auth metadata::tests::authorization_server_metadata_exposes_lab_endpoints
      run_test labby-auth metadata::tests::authorization_server_metadata_advertises_private_key_jwt_without_machine_clients
      run_test labby-auth metadata::tests::authorization_server_metadata_keeps_issuer_binding_in_compatibility_mode
      run_test labby-auth authorize::response::tests::successful_authorization_response_uses_exact_metadata_issuer
      run_test labby-auth authorize::response::tests::error_authorization_response_uses_exact_metadata_issuer
      run_test labby-auth cimd::tests::honours_every_auth_method_the_client_publishes
      run_test labby-auth cimd::tests::rejects_a_published_auth_method_we_do_not_implement
      ;;
    OAI-AUTH-006)
      run_test labby-auth jwt::tests::minted_access_token_round_trips_and_contains_kid
      run_test labby-auth jwt::tests::wrong_audience_is_rejected
      run_test labby-auth jwt::tests::expired_access_token_is_rejected
      run_test labby-auth jwt::tests::not_yet_valid_access_token_is_rejected
      run_test labby-auth jwt::tests::validate_with_issuer_rejects_wrong_issuer_via_validation_struct
      run_test labby-auth middleware::tests::jwt_validation_path_accepts_signed_token_and_writes_context
      run_test labby-auth middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge
      run_test labby-auth authorize::tests::authorize_rejects_mismatched_resource_parameter
      run_test labby-auth token::tests::token_endpoint_rejects_mismatched_resource_parameter
      ;;
    OAI-AUTH-007)
      run_test labby api::router::tests::protected_mcp_route_metadata_uses_host_and_path_resource
      ;;
    OAI-AUTH-008)
      run_test labby api::router::tests::oauth_mode_missing_token_returns_www_authenticate_metadata_hint
      run_test labby api::router::tests::protected_mcp_route_unauthorized_header_points_to_route_metadata
      run_test labby api::router::tests::protected_mcp_route_insufficient_scope_returns_rfc_9728_challenge
      ;;
    OAI-AUTH-009)
      run_test labby api::router::tests::disabled_dynamic_registration_is_neither_advertised_nor_mounted
      ;;
    OAI-AUTH-010)
      run_test labby-auth token::tests::revocation_endpoint_invalidates_refresh_token_and_is_idempotent
      run_test labby-auth token::tests::refresh_grant_does_not_elevate_legacy_scope
      run_test labby-auth token::tests::refresh_grant_replay_rejects_a_revoked_replacement_token
      python3 scripts/ci/auth_backup_restore_drill.py
      ;;
    *)
      echo "unknown OpenAI auth requirement: $1" >&2
      return 2
      ;;
  esac
}

if [[ ${1:-} == --list ]]; then
  printf '%s\n' "${IDS[@]}"
  exit 0
fi

if [[ $# -eq 1 ]]; then
  run_requirement "$1"
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "usage: $0 [--list|OAI-AUTH-NNN]" >&2
  exit 2
fi

# CI runs one exact nextest expression so the product/auth graph is built once.
# Individual requirement IDs above remain independently reproducible.
readonly EXACT_TESTS='test(=metadata::tests::protected_resource_metadata_uses_canonical_mcp_resource_uri)
| test(=metadata::tests::authorization_server_metadata_exposes_lab_endpoints)
| test(=metadata::tests::authorization_server_metadata_advertises_private_key_jwt_without_machine_clients)
| test(=metadata::tests::authorization_server_metadata_keeps_issuer_binding_in_compatibility_mode)
| test(=authorize::response::tests::successful_authorization_response_uses_exact_metadata_issuer)
| test(=authorize::response::tests::error_authorization_response_uses_exact_metadata_issuer)
| test(=cimd::tests::honours_every_auth_method_the_client_publishes)
| test(=cimd::tests::rejects_a_published_auth_method_we_do_not_implement)
| test(=jwt::tests::minted_access_token_round_trips_and_contains_kid)
| test(=jwt::tests::wrong_audience_is_rejected)
| test(=jwt::tests::expired_access_token_is_rejected)
| test(=jwt::tests::not_yet_valid_access_token_is_rejected)
| test(=jwt::tests::validate_with_issuer_rejects_wrong_issuer_via_validation_struct)
| test(=middleware::tests::jwt_validation_path_accepts_signed_token_and_writes_context)
| test(=middleware::tests::insufficient_jwt_and_static_scopes_return_403_challenge)
| test(=authorize::tests::authorize_rejects_mismatched_resource_parameter)
| test(=token::tests::token_endpoint_rejects_mismatched_resource_parameter)
| test(=token::tests::revocation_endpoint_invalidates_refresh_token_and_is_idempotent)
| test(=token::tests::refresh_grant_does_not_elevate_legacy_scope)
| test(=token::tests::refresh_grant_replay_rejects_a_revoked_replacement_token)
| test(=mcp::permanent_tools::tests::builtin_service_tool_advertises_top_level_security_schemes)
| test(=mcp::permanent_tools::tests::protected_boundary_rebinds_upstream_descriptor_security_policy)
| test(=mcp::handlers_tools::tests::raw_mode_preserves_upstream_annotations_verbatim_on_both_listing_paths)
| test(=mcp::permanent_tools::tests::builtin_service_tool_mirrors_security_schemes_in_legacy_meta)
| test(=mcp::result_format::tests::auth_errors_carry_mcp_reauthentication_metadata)
| test(=mcp::result_format::tests::forbidden_and_non_auth_errors_publish_only_applicable_challenges)
| test(=api::router::tests::protected_mcp_route_metadata_uses_host_and_path_resource)
| test(=api::router::tests::oauth_mode_missing_token_returns_www_authenticate_metadata_hint)
| test(=api::router::tests::protected_mcp_route_unauthorized_header_points_to_route_metadata)
| test(=api::router::tests::protected_mcp_route_insufficient_scope_returns_rfc_9728_challenge)
| test(=api::router::tests::disabled_dynamic_registration_is_neither_advertised_nor_mounted)'
cargo nextest run -p labby-auth -p labby --all-features --locked --lib -E "$EXACT_TESTS"
python3 scripts/ci/auth_backup_restore_drill.py
