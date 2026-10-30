//! Single integration-test binary for sloc-languages. Each former top-level
//! `tests/*.rs` is now a module under `tests/it/` so the whole suite links into
//! one binary instead of one per file (fewer link steps, far less target/ debug
//! bloat). The `tests/corpus/` fixture tree is unaffected (data, not a module).
mod classify_lines;
mod coverage_boost;
mod coverage_gaps;
mod golden;
mod style_branches;
