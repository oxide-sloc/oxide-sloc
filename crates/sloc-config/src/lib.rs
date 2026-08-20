// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MixedLinePolicy {
    #[default]
    CodeOnly,
    CodeAndComment,
    CommentOnly,
    SeparateMixedCategory,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BinaryFileBehavior {
    #[default]
    Skip,
    Fail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailureBehavior {
    #[default]
    WarnSkip,
    Fail,
}

/// IEEE 1045-1992: how backslash line continuations are handled for physical SLOC counting.
///
/// Physical SLOC (the default) counts each physical line. Logical mode collapses a
/// backslash-continued sequence into a single counted line, which is useful when measuring
/// logical statements (e.g., multi-line C preprocessor macros).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationLinePolicy {
    #[default]
    /// Count each physical line separately — the IEEE 1045-1992 default for physical SLOC.
    EachPhysicalLine,
    /// Collapse backslash-continued physical lines into a single logical line.
    CollapseToLogical,
}

/// IEEE 1045-1992: how blank lines that fall inside a block comment are classified.
///
/// The standard aligns with counting them as comment lines (they are part of the comment
/// body). The `CountAsBlank` variant preserves the legacy behaviour if required.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlankInBlockCommentPolicy {
    #[default]
    /// Blank lines inside /* */ (or equivalent) blocks count as comment lines — IEEE aligned.
    CountAsComment,
    /// Blank lines inside block comments count as blank lines.
    CountAsBlank,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default)]
    pub root_paths: Vec<PathBuf>,
    #[serde(default)]
    pub include_globs: Vec<String>,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
    #[serde(default = "default_excluded_directories")]
    pub excluded_directories: Vec<String>,
    #[serde(default = "default_true")]
    pub honor_ignore_files: bool,
    #[serde(default = "default_true")]
    pub ignore_hidden_files: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_max_file_size_bytes")]
    pub max_file_size_bytes: u64,
    #[serde(default)]
    pub parallelism_limit: Option<usize>,
    /// When true, detect .gitmodules and produce a per-submodule summary alongside the overall run.
    #[serde(default = "default_true")]
    pub submodule_breakdown: bool,
    #[serde(default)]
    pub allowed_scan_roots: Vec<PathBuf>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            root_paths: Vec::new(),
            include_globs: Vec::new(),
            exclude_globs: Vec::new(),
            excluded_directories: vec![".git".into(), "node_modules".into(), "target".into()],
            honor_ignore_files: true,
            ignore_hidden_files: true,
            follow_symlinks: false,
            max_file_size_bytes: 2 * 1024 * 1024,
            parallelism_limit: None,
            submodule_breakdown: true,
            allowed_scan_roots: Vec::new(),
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default)]
    pub enabled_languages: Vec<String>,
    #[serde(default)]
    pub extension_overrides: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub shebang_detection: bool,
    #[serde(default)]
    pub mixed_line_policy: MixedLinePolicy,
    #[serde(default = "default_true")]
    pub python_docstrings_as_comments: bool,
    #[serde(default = "default_true")]
    pub generated_file_detection: bool,
    #[serde(default = "default_true")]
    pub minified_file_detection: bool,
    #[serde(default = "default_true")]
    pub vendor_directory_detection: bool,
    #[serde(default)]
    pub include_lockfiles: bool,
    #[serde(default)]
    pub binary_file_behavior: BinaryFileBehavior,
    #[serde(default)]
    pub decode_failure_behavior: FailureBehavior,
    #[serde(default)]
    pub parse_failure_behavior: FailureBehavior,
    /// IEEE 1045-1992: how backslash line continuations (C macros, shell, Makefile) are counted.
    #[serde(default)]
    pub continuation_line_policy: ContinuationLinePolicy,
    /// IEEE 1045-1992: whether blank lines inside block comments count as comment lines.
    #[serde(default)]
    pub blank_in_block_comment_policy: BlankInBlockCommentPolicy,
    /// IEEE 1045-1992 §4.2: when false, preprocessor/compiler directives (#include, #define,
    /// etc.) are excluded from code SLOC and tracked separately in `compiler_directive_lines`.
    /// Applies to C, C++, and Objective-C. Default: true (directives count toward code SLOC).
    #[serde(default = "default_true")]
    pub count_compiler_directives: bool,
    /// Optional SLOC budget thresholds. When set, `--fail-on-budget` exits non-zero if
    /// any threshold is exceeded. Configured under `[analysis.budget]` in the TOML.
    #[serde(default)]
    pub budget: Option<BudgetConfig>,
    /// Path to a coverage report; format is auto-detected (LCOV `.info` from lcov/gcov/
    /// cargo-llvm-cov, Cobertura XML, `JaCoCo` XML, coverage.py JSON, or Istanbul/NYC JSON).
    /// When set, oxide-sloc attaches per-file line/function/branch coverage to each `FileRecord`.
    /// Can also be set via the `SLOC_COVERAGE_FILE` environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_file: Option<PathBuf>,
    /// Column-width threshold for style "N-col compliant" reporting (default 80).
    /// Supported values: 80, 100, 120 (others snap to the nearest bucket).
    /// Files where ≤ 5 % of lines exceed this limit count as compliant.
    #[serde(default = "default_style_col_threshold")]
    pub style_col_threshold: u16,
    /// When false, skip all style-guide heuristic analysis entirely (faster on very large repos).
    /// Default: true.
    #[serde(default = "default_true")]
    pub style_analysis_enabled: bool,
    /// Minimum dominant-guide adherence score (0-100) below which a file is flagged in the
    /// per-file style table. 0 = no threshold / all files shown without warning. Default: 0.
    #[serde(default)]
    pub style_score_threshold: u8,
    /// Language scope for style analysis. "all" = every supported language family (default).
    /// `"c_family"` = C / C++ / Objective-C only (fast, backwards-compatible).
    #[serde(default = "default_style_lang_scope")]
    pub style_lang_scope: String,
    /// Git activity window in days. **On by default (90)**: oxide-sloc runs a single
    /// `git log --since` pass and attaches per-file commit-count + last-change date to each
    /// `FileRecord`, powering the hotspots view. `Some(0)` (or `None`) disables it; on a
    /// non-git path the single `git log` attempt fails gracefully and no hotspots are produced.
    /// This is distinct from the scan-to-scan "churn rate" shown in the web UI's Compare page.
    #[serde(
        default = "default_activity_window_days",
        skip_serializing_if = "Option::is_none"
    )]
    pub activity_window_days: Option<u32>,
    /// Per-author code-ownership attribution. **Off by default** (opt-in): it adds a
    /// `git blame` pass over every analyzed file, which is markedly slower than the single
    /// `git log` activity pass. When enabled on a git repo, oxide-sloc attributes each
    /// physical line to the author who last touched it (blame honours the repo `.mailmap`),
    /// buckets it as code / comment / blank, and rolls the results up per contributor.
    /// No effect on non-git paths.
    #[serde(default)]
    pub attribution: bool,
}

const fn default_true() -> bool {
    true
}

// Serde `default = "..."` for the `Option<u32>` field must return the field type, so the
// `Option` wrapper is required here despite clippy::unnecessary_wraps flagging it under pedantic.
#[allow(clippy::unnecessary_wraps)]
const fn default_activity_window_days() -> Option<u32> {
    Some(90)
}

const fn default_style_col_threshold() -> u16 {
    80
}

fn default_style_lang_scope() -> String {
    "all".into()
}

fn default_excluded_directories() -> Vec<String> {
    vec![".git".into(), "node_modules".into(), "target".into()]
}

const fn default_max_file_size_bytes() -> u64 {
    2 * 1024 * 1024
}

fn default_report_title() -> String {
    "OxideSLOC Report".into()
}

fn default_output_formats() -> Vec<String> {
    vec!["cli".into(), "json".into(), "html".into()]
}

fn default_theme() -> String {
    "auto".into()
}

fn default_bind_address() -> String {
    "127.0.0.1:4317".into()
}

/// Validates that `s` is a CSS hex colour: `#RGB` or `#RRGGBB`.
///
/// # Errors
/// Returns an error if `s` does not start with `#` or is not a 3- or 6-digit hex colour.
pub fn validate_hex_color(s: &str) -> Result<()> {
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| anyhow::anyhow!("must start with '#'"))?;
    if !matches!(hex.len(), 3 | 6) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("must be a 3- or 6-digit hex colour (e.g. #3b82f6)");
    }
    Ok(())
}

/// Per-language and total SLOC thresholds. Used with `--fail-on-budget` in CI.
///
/// Keys in `per_language` are case-insensitive language display names
/// (e.g. `"rust"`, `"typescript"`). Zero means unlimited.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum total code lines across all languages (0 = unlimited).
    #[serde(default)]
    pub total_max: u64,
    /// Per-language code-line ceilings. Key is the language display name, lowercase.
    #[serde(default)]
    pub per_language: BTreeMap<String, u64>,
}

impl BudgetConfig {
    /// Returns `true` if no limits are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_max == 0 && self.per_language.is_empty()
    }

    /// # Errors
    ///
    /// Returns an error if any budget threshold is zero (which would always fail).
    pub fn validate(&self) -> Result<()> {
        for (lang, &limit) in &self.per_language {
            if limit == 0 {
                anyhow::bail!("per_language[\"{lang}\"] limit must be > 0");
            }
        }
        Ok(())
    }
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            enabled_languages: Vec::new(),
            extension_overrides: BTreeMap::new(),
            shebang_detection: true,
            mixed_line_policy: MixedLinePolicy::CodeOnly,
            python_docstrings_as_comments: true,
            generated_file_detection: true,
            minified_file_detection: true,
            vendor_directory_detection: true,
            include_lockfiles: false,
            binary_file_behavior: BinaryFileBehavior::Skip,
            decode_failure_behavior: FailureBehavior::WarnSkip,
            parse_failure_behavior: FailureBehavior::WarnSkip,
            continuation_line_policy: ContinuationLinePolicy::EachPhysicalLine,
            blank_in_block_comment_policy: BlankInBlockCommentPolicy::CountAsComment,
            count_compiler_directives: true,
            budget: None,
            coverage_file: None,
            style_col_threshold: 80,
            style_analysis_enabled: true,
            style_score_threshold: 0,
            style_lang_scope: "all".into(),
            activity_window_days: Some(90),
            attribution: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportingConfig {
    #[serde(default = "default_report_title")]
    pub report_title: String,
    #[serde(default = "default_output_formats")]
    pub output_formats: Vec<String>,
    #[serde(default = "default_true")]
    pub include_summary_charts: bool,
    #[serde(default = "default_true")]
    pub include_skipped_files_section: bool,
    #[serde(default = "default_true")]
    pub include_warnings_section: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Optional company or team name shown in the report header instead of "`OxideSLOC`".
    #[serde(default)]
    pub company_name: Option<String>,
    /// Path to a PNG/SVG logo file to embed in the report header.
    /// If unset, the default `OxideSLOC` logo is used.
    #[serde(default)]
    pub logo_path: Option<std::path::PathBuf>,
    /// CSS hex colour (e.g. `#3b82f6`) used as the primary accent throughout the report.
    /// Must start with `#` and be a valid 3- or 6-digit hex colour.
    #[serde(default)]
    pub accent_color: Option<String>,
    /// Text printed in a header and footer strip on every page of the HTML/PDF report.
    /// Use for company name, project identifier, or scanner identification.
    #[serde(default)]
    pub report_header_footer: Option<String>,
}

impl Default for ReportingConfig {
    fn default() -> Self {
        Self {
            report_title: "OxideSLOC Report".into(),
            output_formats: vec!["cli".into(), "json".into(), "html".into()],
            include_summary_charts: true,
            include_skipped_files_section: true,
            include_warnings_section: true,
            theme: "auto".into(),
            company_name: None,
            logo_path: None,
            accent_color: None,
            report_header_footer: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// When true the server binds to 0.0.0.0 by default, suppresses browser
    /// auto-open, and disables desktop-only routes (pick-directory, open-path).
    #[serde(default)]
    pub server_mode: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:4317".into(),
            server_mode: false,
        }
    }
}

/// Non-secret git integration settings. Per-host credentials are intentionally NOT here —
/// tokens/keys live only in the environment (`SLOC_GIT_CRED_<HOST>`, `SLOC_GIT_SSHKEY_<HOST>`)
/// or the `SLOC_GIT_CRED_FILE`, never in a committed TOML. This section only carries the
/// offline-import gate so operators who prefer a config file over env vars can set it there.
/// Values here are applied to the environment at startup **only if the corresponding env var
/// is unset**, so an explicit env var always wins (see [`AppConfig::apply_git_settings_to_env`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitConfig {
    /// Permit cloning from local/offline sources (git bundle, `file://`, local path).
    /// Maps to `SLOC_GIT_ALLOW_LOCAL`. Default false (network sources only).
    #[serde(default)]
    pub allow_local: bool,
    /// Directory that local/offline sources must resolve under (required when
    /// `allow_local` is on). Maps to `SLOC_GIT_LOCAL_ROOT`.
    #[serde(default)]
    pub local_root: Option<PathBuf>,
}

/// A named configuration profile.
///
/// All sub-config sections are optional; any present section *replaces* the
/// corresponding base config section in full. Commonly used to represent
/// different scanning contexts in the same repo
/// (e.g. `[profile.frontend]`, `[profile.backend]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileConfig {
    #[serde(default)]
    pub discovery: Option<DiscoveryConfig>,
    #[serde(default)]
    pub analysis: Option<AnalysisConfig>,
    #[serde(default)]
    pub reporting: Option<ReportingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub analysis: AnalysisConfig,
    #[serde(default)]
    pub reporting: ReportingConfig,
    #[serde(default)]
    pub web: WebConfig,
    /// Non-secret git integration settings (offline-import gate). See [`GitConfig`].
    #[serde(default)]
    pub git: GitConfig,
    /// Named profiles that override base config sections when selected via `--profile`.
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

impl AppConfig {
    /// Apply the named profile overrides on top of this config.
    ///
    /// # Errors
    ///
    /// Returns an error if no profile with that name exists or if the resulting
    /// config fails validation.
    pub fn apply_profile(&mut self, name: &str) -> Result<()> {
        let profile = self
            .profiles
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("profile '{name}' not found in config"))?
            .clone();
        if let Some(d) = profile.discovery {
            self.discovery = d;
        }
        if let Some(a) = profile.analysis {
            self.analysis = a;
        }
        if let Some(r) = profile.reporting {
            self.reporting = r;
        }
        self.validate()
    }
}

impl AppConfig {
    /// Apply the `[git]` offline-import settings to the process environment so `sloc-git`
    /// (which reads `SLOC_GIT_ALLOW_LOCAL` / `SLOC_GIT_LOCAL_ROOT` directly) picks them up.
    ///
    /// An explicit environment variable always wins: a config value is only applied when the
    /// corresponding env var is unset. Call this once, early at startup, before any git op and
    /// before spawning worker threads.
    pub fn apply_git_settings_to_env(&self) {
        if self.git.allow_local && std::env::var_os("SLOC_GIT_ALLOW_LOCAL").is_none() {
            // SAFETY: called once at single-threaded startup before any git op / task spawn.
            unsafe { std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1") };
        }
        if let Some(root) = &self.git.local_root
            && std::env::var_os("SLOC_GIT_LOCAL_ROOT").is_none()
            && !root.as_os_str().is_empty()
        {
            // SAFETY: called once at single-threaded startup before any git op / task spawn.
            unsafe { std::env::set_var("SLOC_GIT_LOCAL_ROOT", root) };
        }
    }

    /// # Errors
    ///
    /// Returns an error if the file cannot be read, the TOML cannot be parsed, or the
    /// resulting config fails validation.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse TOML config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// # Errors
    ///
    /// Returns an error if any configuration field contains an invalid value.
    pub fn validate(&self) -> Result<()> {
        if self.discovery.max_file_size_bytes == 0 {
            anyhow::bail!("discovery.max_file_size_bytes must be greater than zero");
        }

        if self.web.bind_address.trim().is_empty() {
            anyhow::bail!("web.bind_address must not be empty");
        }

        if let Some(color) = &self.reporting.accent_color {
            validate_hex_color(color)
                .with_context(|| format!("reporting.accent_color is invalid: {color}"))?;
        }

        if let Some(budget) = &self.analysis.budget {
            budget.validate().context("analysis.budget is invalid")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_hex_color ───────────────────────────────────────────────────

    #[test]
    fn hex_color_valid_six_digits() {
        assert!(validate_hex_color("#3b82f6").is_ok());
        assert!(validate_hex_color("#FFFFFF").is_ok());
        assert!(validate_hex_color("#000000").is_ok());
    }

    #[test]
    fn hex_color_valid_three_digits() {
        assert!(validate_hex_color("#abc").is_ok());
        assert!(validate_hex_color("#FFF").is_ok());
    }

    #[test]
    fn hex_color_missing_hash_fails() {
        assert!(validate_hex_color("3b82f6").is_err());
    }

    #[test]
    fn hex_color_wrong_length_fails() {
        assert!(validate_hex_color("#12345").is_err()); // 5 chars
        assert!(validate_hex_color("#1234567").is_err()); // 7 chars
    }

    #[test]
    fn hex_color_non_hex_chars_fails() {
        assert!(validate_hex_color("#xyz123").is_err());
        assert!(validate_hex_color("#gg0000").is_err());
    }

    #[test]
    fn hex_color_empty_fails() {
        assert!(validate_hex_color("").is_err());
        assert!(validate_hex_color("#").is_err());
    }

    // ── AppConfig::default() validates ──────────────────────────────────────

    #[test]
    fn app_config_default_validates() {
        let cfg = AppConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn activity_window_is_on_by_default() {
        // Default config and a TOML that omits the field both default to a 90-day window.
        assert_eq!(AnalysisConfig::default().activity_window_days, Some(90));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sloc.toml");
        std::fs::write(&path, "[analysis]\n").unwrap();
        let cfg = AppConfig::load_from_file(&path).unwrap();
        assert_eq!(cfg.analysis.activity_window_days, Some(90));
    }

    #[test]
    fn app_config_zero_max_file_size_fails() {
        let mut cfg = AppConfig::default();
        cfg.discovery.max_file_size_bytes = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn app_config_empty_bind_address_fails() {
        let mut cfg = AppConfig::default();
        cfg.web.bind_address = "   ".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn app_config_invalid_accent_color_fails() {
        let mut cfg = AppConfig::default();
        cfg.reporting.accent_color = Some("not-a-color".into());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn app_config_valid_accent_color_passes() {
        let mut cfg = AppConfig::default();
        cfg.reporting.accent_color = Some("#3b82f6".into());
        assert!(cfg.validate().is_ok());
    }

    // ── BudgetConfig ─────────────────────────────────────────────────────────

    #[test]
    fn budget_config_is_empty_when_all_zero() {
        let budget = BudgetConfig {
            total_max: 0,
            per_language: BTreeMap::new(),
        };
        assert!(budget.is_empty());
    }

    #[test]
    fn budget_config_not_empty_when_total_set() {
        let budget = BudgetConfig {
            total_max: 10_000,
            per_language: BTreeMap::new(),
        };
        assert!(!budget.is_empty());
    }

    #[test]
    fn budget_config_validate_passes_with_positive_per_lang() {
        let mut budget = BudgetConfig {
            total_max: 0,
            per_language: BTreeMap::new(),
        };
        budget.per_language.insert("rust".into(), 5_000);
        assert!(budget.validate().is_ok());
    }

    #[test]
    fn budget_config_validate_fails_zero_per_lang() {
        let mut budget = BudgetConfig {
            total_max: 0,
            per_language: BTreeMap::new(),
        };
        budget.per_language.insert("rust".into(), 0);
        assert!(budget.validate().is_err());
    }

    // ── load_from_file ────────────────────────────────────────────────────────

    #[test]
    fn load_from_file_minimal_toml_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sloc.toml");
        std::fs::write(&path, "[discovery]\n").unwrap();
        let cfg = AppConfig::load_from_file(&path).unwrap();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn load_from_file_missing_file_errors() {
        let result = AppConfig::load_from_file(std::path::Path::new("/nonexistent/sloc.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn load_from_file_invalid_toml_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not valid toml {{{{").unwrap();
        let result = AppConfig::load_from_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_from_file_full_config_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.toml");
        let toml = r#"
[discovery]
max_file_size_bytes = 5242880
honor_ignore_files = true

[analysis]
mixed_line_policy = "code_only"

[reporting]
report_title = "My Report"

[web]
bind_address = "127.0.0.1:4317"
"#;
        std::fs::write(&path, toml).unwrap();
        let cfg = AppConfig::load_from_file(&path).unwrap();
        assert_eq!(cfg.reporting.report_title, "My Report");
        assert_eq!(cfg.web.bind_address, "127.0.0.1:4317");
    }

    // ── Enum serde round-trips ────────────────────────────────────────────────

    #[test]
    fn mixed_line_policy_serde_roundtrip() {
        for variant in [
            MixedLinePolicy::CodeOnly,
            MixedLinePolicy::CodeAndComment,
            MixedLinePolicy::CommentOnly,
            MixedLinePolicy::SeparateMixedCategory,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: MixedLinePolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn binary_file_behavior_serde_roundtrip() {
        for variant in [BinaryFileBehavior::Skip, BinaryFileBehavior::Fail] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: BinaryFileBehavior = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn continuation_line_policy_serde_roundtrip() {
        for variant in [
            ContinuationLinePolicy::EachPhysicalLine,
            ContinuationLinePolicy::CollapseToLogical,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: ContinuationLinePolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn blank_in_block_comment_policy_serde_roundtrip() {
        for variant in [
            BlankInBlockCommentPolicy::CountAsComment,
            BlankInBlockCommentPolicy::CountAsBlank,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: BlankInBlockCommentPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn apply_profile_overrides_sections() {
        let mut cfg = AppConfig::default();
        let mut analysis = cfg.analysis.clone();
        analysis.count_compiler_directives = !analysis.count_compiler_directives;
        let mut reporting = cfg.reporting.clone();
        reporting.report_title = "Profiled".to_string();
        cfg.profiles.insert(
            "ci".to_string(),
            ProfileConfig {
                discovery: Some(cfg.discovery.clone()),
                analysis: Some(analysis.clone()),
                reporting: Some(reporting),
            },
        );
        cfg.apply_profile("ci").expect("profile should apply");
        assert_eq!(cfg.reporting.report_title, "Profiled");
        assert_eq!(
            cfg.analysis.count_compiler_directives,
            analysis.count_compiler_directives
        );
    }

    #[test]
    fn apply_profile_unknown_name_errors() {
        let mut cfg = AppConfig::default();
        assert!(cfg.apply_profile("does-not-exist").is_err());
    }
}
