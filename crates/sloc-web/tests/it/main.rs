//! Single integration-test binary for the sloc-web tests that do NOT mutate
//! process-global state. Each former top-level `tests/*.rs` is now a module under
//! `tests/it/` so they link into one binary instead of one per file (fewer link
//! steps, far less target/ debug bloat).
//!
//! Deliberately NOT merged here (kept as their own binaries for process isolation,
//! because they set/remove env vars and would race under a shared process):
//! `audit_chain_write`, `consent_gate`, `coverage_gaps`, `tls_config`.
//! The `tests/fixtures/` tree is unaffected (data, not a module).
mod api_functions;
mod audit_verify;
mod auth_readonly;
mod bundle_run;
mod coverage_boost;
mod coverage_extra2;
mod coverage_gaps2;
mod extra_coverage;
mod git_browser_error;
mod handler_success_paths;
mod integration;
mod render_branches;
mod server_mode_routes;
mod targeted_coverage;
