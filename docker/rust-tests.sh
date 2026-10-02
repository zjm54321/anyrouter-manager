#!/bin/sh
set -eu
# Listing compiles the whole test target, but is NOT test execution evidence.
cargo test --locked --bin anyrouter-manager-backend -- --list > /tmp/rust-test-list.txt
cat /tmp/rust-test-list.txt
total=$(grep -c ': test$' /tmp/rust-test-list.txt)
executed=0
for name in \
    config::tests::login_diagnostics_is_explicit_and_backward_compatible \
    config::tests::container_origins_and_executable_are_explicit \
    config::tests::insecure_lan_http_is_explicit_and_backward_compatible \
    config::tests::insecure_lan_http_requires_all_switches_and_exact_ipv4_range \
    config::tests::insecure_lan_opt_in_preserves_https_and_loopback_policy \
    diagnostics::tests::exact_schema_roundtrip_and_strict_bounds \
    error::tests::source_metadata_is_finite_and_absent_by_default \
    app::runtime_tests::secure_cookie_has_same_creation_deletion_policy \
    helper::container_command_is_explicit_and_native_command_keeps_namespace \
    tests::cookie_structure_requires_finite_expiry_but_not_live_session \
    tests::cookie_selection_obeys_domain_secure_expiry_path_and_precedence \
    tests::insecure_lan_http_login_session_logout_preserves_security_contract \
    upstream_body::tests::gzip_is_bounded_and_requires_complete_valid_stream \
    log_settings::tests::strict_defaults_thresholds_and_fixed_errors \
    log_settings::tests::update_commit_hot_watch_failure_keeps_old_and_reopen \
    helper::tests::session_payload_prunes_expired_and_canonicalizes_session_expiry \
    helper::tests::session_payload_expired_is_not_malformed_but_unsafe_fields_still_are
do
    # Exact matching and an asserted count prevent renamed tests passing as zero tests.
    cargo test --locked --bin anyrouter-manager-backend "$name" -- --exact > /tmp/rust-test-result.txt
    cat /tmp/rust-test-result.txt
    grep -F 'test result: ok. 1 passed; 0 failed; 0 ignored;' /tmp/rust-test-result.txt
    executed=$((executed + 1))
done
printf 'Rust pure-unit gate: %s executed; %s listed binary tests deferred, NOT passed.\n' "$executed" "$((total - executed))"
printf '%s\n' 'Integration tests are NOT executed in this build. Production container supervisor/browser probes are a separate mandatory runtime gate, not evidence of the full Rust regression suite. Native namespace and parent-monitor fault tests remain separate host validation.'
