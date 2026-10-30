//! Single integration-test binary for sloc-report. Each former top-level `tests/*.rs`
//! is now a module under `tests/it/` so the whole suite links into one binary
//! instead of one per file (fewer link steps, far less target/ debug bloat).
mod coverage_gaps;
mod exports;
mod render;
