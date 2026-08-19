// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
#![allow(clippy::multiple_crate_versions)]

use std::fmt::Write as FmtWrite;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{MultiPart, SinglePart, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use tracing_subscriber::EnvFilter;

use sloc_config::{AppConfig, BlankInBlockCommentPolicy, ContinuationLinePolicy, MixedLinePolicy};
use sloc_core::{
    AnalysisRun, BaselineEntry, BaselineStore, ScanComparison, ScanRegistry, analyze,
    check_against_baseline, compute_delta, execute_run_prune, plan_run_prune, read_json,
    resolve_baselines_path, resolve_output_root, resolve_registry_path, rotated_log_paths,
    write_json,
};
use sloc_git::{clone_or_fetch, create_worktree, destroy_worktree, get_sha};
use sloc_report::{
    render_html, write_csv, write_diff_csv, write_html, write_html_with_pdf_link,
    write_pdf_from_run, write_xlsx,
};

mod atlassian;
use atlassian::{AtlassianTier, atlassian_auth, atlassian_ssrf_check, detect_tier};

// ── ANSI color helpers ────────────────────────────────────────────────────────

fn color_enabled() -> bool {
    std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

macro_rules! paint {
    ($enabled:expr_2021, $code:expr_2021, $val:expr_2021) => {
        if $enabled {
            format!("\x1b[{}m{}\x1b[0m", $code, $val)
        } else {
            $val.to_string()
        }
    };
}

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(name = "oxide-sloc", version)]
#[command(about = "Cross-platform source line analysis tool")]
#[command(
    long_about = "Cross-platform source line analysis tool.\n\nRun without arguments to start the web UI on http://127.0.0.1:4317."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Scan one or more directories and count source lines
    Analyze(Box<AnalyzeArgs>),
    /// Re-render a report from a saved JSON result (no re-scan)
    Report(ReportArgs),
    /// Compare two saved JSON results and show the delta
    Diff(DiffArgs),
    /// Start the web UI (default when no subcommand given).
    /// Use --server for multi-user LAN access; without it, CORS restricts API calls to localhost only.
    Serve(ServeArgs),
    /// Generate a starter .oxide-sloc.toml config file
    Init(InitArgs),
    /// Validate a config file: parse TOML, check paths/globs, logo, accent colour, and profiles
    Validate(ValidateArgs),
    /// Deliver a saved report via SMTP or webhook
    Send(Box<SendArgs>),
    /// Materialize a saved JSON result into the local web-UI run layout under an
    /// output directory, registering it in registry.json so the web UI's Compare /
    /// "Scan Delta" page can pick it up. Ideal for dropping CI results into a user's
    /// local `out/web/`.
    Bundle(BundleArgs),
    /// Clone a repository and scan it at a specific branch, tag, or commit SHA
    GitScan(GitScanArgs),
    /// Scan two git refs and emit a comparison (diff) report
    GitCompare(GitCompareArgs),
    /// Poll a repository branch for changes and scan on every new commit
    Watch(WatchArgs),
    /// Reclaim disk: delete old scan artifacts and rotate/remove log files.
    /// Runs as the local user against the output tree — no server or login needed.
    /// Dry-run by default; pass --yes to actually delete.
    Prune(PruneArgs),
    /// Post an SLOC diff comment to a pull request on GitHub, GitLab, or Bitbucket.
    /// Designed to be called from Jenkins post-build steps and CI pipelines.
    #[command(name = "pr-comment")]
    PrComment(PrCommentArgs),
    /// Post an SLOC summary to a Jira issue (comment, remote link, or custom field).
    /// Targets on-premises Jira Server/Data Center (built-in REST, no marketplace app);
    /// Atlassian Cloud is auto-detected as a fallback. Designed for CI pipelines.
    Jira(JiraArgs),
    /// Post an SLOC build status to a Bitbucket commit.
    /// Targets on-premises Bitbucket Server/Data Center (built-in REST); Bitbucket
    /// Cloud is auto-detected as a fallback. Designed for CI pipelines.
    #[command(name = "bitbucket-status")]
    BitbucketStatus(BitbucketStatusArgs),
    /// Verify the integrity of a hash-chained audit log.
    /// Recomputes each record's keyed MAC and checks the chain links; reports the
    /// first altered/removed record. Requires the same key used when writing
    /// (`SLOC_AUDIT_HMAC_KEY`, or pass `--key`).
    #[command(name = "verify-audit")]
    VerifyAudit(VerifyAuditArgs),
    /// Probe a running oxide-sloc server's health endpoint and exit 0 (healthy)
    /// or 1 (unreachable / non-2xx). Intended for container and orchestrator
    /// health checks (e.g. Docker HEALTHCHECK).
    Healthz(HealthzArgs),
    /// Print shell completion script to stdout.
    /// Source the output to enable tab-completion for the current shell session.
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

// ── healthz ─────────────────────────────────────────────────────────────────--

#[derive(Debug, Args)]
struct HealthzArgs {
    /// URL to probe. Defaults to `http://<SLOC_BIND host:port or 127.0.0.1:4317>/healthz`.
    #[arg(long, value_name = "URL")]
    url: Option<String>,

    /// Request timeout in seconds.
    #[arg(long, default_value_t = 5, value_name = "SECS")]
    timeout: u64,
}

// ── verify-audit ──────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct VerifyAuditArgs {
    /// Path to the audit log to verify. Defaults to `$SLOC_AUDIT_LOG`.
    #[arg(value_name = "PATH")]
    log: Option<PathBuf>,

    /// Integrity key. Defaults to `$SLOC_AUDIT_HMAC_KEY`.
    #[arg(long, value_name = "KEY")]
    key: Option<String>,
}

// ── analyze ───────────────────────────────────────────────────────────────────

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Args)]
struct AnalyzeArgs {
    /// One or more directories to scan
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Load configuration from a TOML file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Write JSON result to this path
    #[arg(long, short = 'j', value_name = "PATH")]
    json_out: Option<PathBuf>,

    /// Write HTML report to this path
    #[arg(long, short = 'H', value_name = "PATH")]
    html_out: Option<PathBuf>,

    /// Write PDF report to this path (pure Rust, no browser required)
    #[arg(long, value_name = "PATH")]
    pdf_out: Option<PathBuf>,

    /// Override the git branch recorded in the report (useful in CI where HEAD
    /// is detached and automatic detection yields nothing)
    #[arg(long, value_name = "BRANCH")]
    git_branch: Option<String>,

    /// Write CSV summary to this path
    #[arg(long, short = 'c', value_name = "PATH")]
    csv_out: Option<PathBuf>,

    /// Write Excel (.xlsx) workbook to this path
    #[arg(long, short = 'x', value_name = "PATH")]
    xlsx_out: Option<PathBuf>,

    /// Open the generated HTML report in the default browser
    #[arg(long)]
    open: bool,

    /// Suppress all output except errors
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Exit with code 2 if any warnings are emitted
    #[arg(long)]
    fail_on_warnings: bool,

    /// Exit with code 3 if code lines fall below this threshold
    #[arg(long, value_name = "N")]
    fail_below: Option<u64>,

    /// Override mixed-line counting policy
    #[arg(long)]
    mixed_line_policy: Option<MixedLinePolicy>,

    /// Count Python docstrings as code rather than comments
    #[arg(long)]
    python_docstrings_as_code: bool,

    /// IEEE 1045-1992: override continuation-line counting policy
    #[arg(long)]
    continuation_line_policy: Option<ContinuationLinePolicy>,

    /// IEEE 1045-1992: override blank-line classification inside block comments
    #[arg(long)]
    blank_in_block_comment_policy: Option<BlankInBlockCommentPolicy>,

    /// IEEE 1045-1992 §4.2: exclude compiler directives (#include, #define, etc.) from
    /// code SLOC; they are tracked in raw counts but not added to effective code lines
    #[arg(long)]
    no_count_compiler_directives: bool,

    /// Ignore .gitignore / .ignore files
    #[arg(long)]
    no_ignore_files: bool,

    /// Follow symbolic links during discovery
    #[arg(long)]
    follow_symlinks: bool,

    /// Include only files matching this glob (repeatable)
    #[arg(long, value_name = "PATTERN")]
    include_glob: Vec<String>,

    /// Exclude files matching this glob (repeatable)
    #[arg(long, value_name = "PATTERN")]
    exclude_glob: Vec<String>,

    /// Restrict analysis to these languages (repeatable)
    #[arg(long, value_name = "LANG")]
    enabled_language: Vec<String>,

    /// Title shown in HTML / PDF / XLSX reports
    #[arg(long, value_name = "TITLE")]
    report_title: Option<String>,

    /// Include per-file breakdown in terminal output
    #[arg(long)]
    per_file: bool,

    /// Machine-readable key=value terminal output
    #[arg(long)]
    plain: bool,

    /// Detect git submodules and emit per-submodule breakdown
    #[arg(long)]
    submodule_breakdown: bool,

    /// Apply a named profile from the config file (e.g. --profile frontend).
    /// Profile sections override the base [discovery], [analysis], and [reporting]
    /// sections in their entirety; define them as [profile.NAME] in the TOML.
    #[arg(long, value_name = "NAME")]
    profile: Option<String>,

    /// Exit with code 4 if any SLOC budget threshold is exceeded.
    /// Thresholds are defined under [analysis.budget] in the config file or
    /// passed via --config.
    #[arg(long)]
    fail_on_budget: bool,

    /// Save this scan as a named baseline snapshot (stored in out/baselines.json).
    /// Use --fail-above-baseline to enforce growth limits against this snapshot later.
    #[arg(long, value_name = "NAME")]
    set_baseline: Option<String>,

    /// Exit with code 5 if code lines grew more than `MAX_DELTA_PCT` % vs. the named
    /// baseline. Omit --max-delta-pct to fail on any growth.
    #[arg(long, value_name = "NAME")]
    fail_above_baseline: Option<String>,

    /// Maximum allowed code-line growth percentage when used with --fail-above-baseline.
    #[arg(long, value_name = "PCT")]
    max_delta_pct: Option<f64>,

    /// Path to a coverage report to attach per-file line/function/branch coverage to the
    /// analysis output. Format is auto-detected: LCOV .info (lcov, gcov, cargo-llvm-cov),
    /// Cobertura XML, `JaCoCo` XML, coverage.py JSON, or Istanbul/NYC JSON.
    /// Can also be set via the `SLOC_COVERAGE_FILE` environment variable.
    #[arg(long, value_name = "FILE")]
    coverage_file: Option<PathBuf>,

    /// Column-width threshold for style N-col compliance reporting (default: 80).
    /// Supported values: 80, 100, 120 — controls the "N-Col Compliant" chip in reports.
    #[arg(long, value_name = "N")]
    style_col_threshold: Option<u16>,

    /// Git activity window in days for the Hotspots ranking (code lines × recent commits),
    /// computed from one `git log` pass. On by default (90); pass 0 to disable. Needs a git repo.
    #[arg(long, value_name = "DAYS")]
    activity_window: Option<u32>,

    /// Write scan configuration JSON to this path (records the effective settings used
    /// for this run — identical to the scan-config_*.json produced by the web UI).
    #[arg(long, value_name = "PATH")]
    scan_config_out: Option<PathBuf>,

    /// When --submodule-breakdown is set, write per-submodule HTML reports into
    /// this directory as sub_<name>.html (mirrors the web UI artifact layout).
    #[arg(long, value_name = "DIR")]
    sub_html_out_dir: Option<PathBuf>,

    /// Exclude duplicate files (identical content) from SLOC totals.
    /// Duplicate groups are always reported regardless of this flag.
    #[arg(long)]
    no_duplicates: bool,

    /// Show COCOMO I cost estimate in terminal output (person-months, schedule, team size).
    #[arg(long)]
    cocomo: bool,

    /// Show Logical SLOC (statement count) alongside physical line counts.
    #[arg(long)]
    lsloc: bool,

    /// Exit with code 6 if any file's cyclomatic complexity exceeds this threshold.
    #[arg(long, value_name = "N")]
    max_complexity: Option<u32>,
}

// ── report ────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct ReportArgs {
    /// Path to a prior JSON result produced by `analyze --json-out`
    #[arg(value_name = "RESULT_JSON")]
    input: PathBuf,

    /// Write HTML report
    #[arg(long, short = 'H', value_name = "PATH")]
    html_out: Option<PathBuf>,

    /// Write PDF report
    #[arg(long, value_name = "PATH")]
    pdf_out: Option<PathBuf>,

    /// Write CSV summary
    #[arg(long, short = 'c', value_name = "PATH")]
    csv_out: Option<PathBuf>,

    /// Write Excel (.xlsx) workbook
    #[arg(long, short = 'x', value_name = "PATH")]
    xlsx_out: Option<PathBuf>,

    /// Open the generated HTML in the default browser
    #[arg(long)]
    open: bool,
}

// ── diff ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct DiffArgs {
    /// Baseline JSON result (the older scan)
    #[arg(value_name = "BASELINE_JSON")]
    baseline: PathBuf,

    /// Current JSON result (the newer scan)
    #[arg(value_name = "CURRENT_JSON")]
    current: PathBuf,

    /// Write delta JSON to this path
    #[arg(long, short = 'j', value_name = "PATH")]
    json_out: Option<PathBuf>,

    /// Write delta CSV to this path
    #[arg(long, short = 'c', value_name = "PATH")]
    csv_out: Option<PathBuf>,

    /// Write delta Excel (.xlsx) to this path
    #[arg(long, short = 'x', value_name = "PATH")]
    xlsx_out: Option<PathBuf>,

    /// Machine-readable key=value terminal output
    #[arg(long)]
    plain: bool,

    /// Suppress all output except errors
    #[arg(long, short = 'q')]
    quiet: bool,
}

// ── serve ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct ServeArgs {
    /// Load configuration from a TOML file
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override the bind address (e.g. 0.0.0.0:4317)
    #[arg(long, value_name = "ADDR")]
    bind: Option<String>,
    /// Enable multi-user LAN mode: binds to 0.0.0.0, suppresses browser auto-open,
    /// disables desktop-only routes, and allows cross-origin API requests from LAN clients.
    /// Without this flag CORS restricts API access to localhost.
    #[arg(long)]
    server: bool,
}

// ── init ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct InitArgs {
    /// Where to write the config file (default: .oxide-sloc.toml in the current directory)
    #[arg(value_name = "PATH", default_value = ".oxide-sloc.toml")]
    output: PathBuf,

    /// Overwrite if the file already exists
    #[arg(long)]
    force: bool,
}

// ── validate ──────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct ValidateArgs {
    /// Path to the config file to validate (default: .oxide-sloc.toml)
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

// ── bundle ────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct BundleArgs {
    /// Path to the JSON analysis result produced by `analyze --json-out`
    #[arg(value_name = "RESULT_JSON")]
    input: PathBuf,

    /// Output directory that mirrors the local web UI's `out/web/` — the run
    /// directory and `registry.json` are created/updated under here
    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,

    /// Run identifier. Defaults to the run_id recorded in the JSON, else a new UUID.
    #[arg(long, value_name = "ID")]
    run_id: Option<String>,

    /// Project label for the run directory / registry entry. Defaults to the
    /// basename of the first input root, matching the web UI's derivation.
    #[arg(long, value_name = "LABEL")]
    label: Option<String>,
}

// ── send ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct SendArgs {
    /// Path to the JSON analysis result produced by `analyze --json-out`
    #[arg(value_name = "RESULT_JSON")]
    input: PathBuf,

    // --- SMTP ---
    /// Send report via email. Comma-separated recipient list.
    #[arg(long, value_name = "EMAIL,...")]
    smtp_to: Vec<String>,
    /// Sender address (From:). Required when --smtp-to is set.
    #[arg(long, value_name = "EMAIL")]
    smtp_from: Option<String>,
    /// SMTP host. Defaults to `SLOC_SMTP_HOST` env var.
    #[arg(long, value_name = "HOST", env = "SLOC_SMTP_HOST")]
    smtp_host: Option<String>,
    /// SMTP port (default 587).
    #[arg(long, value_name = "PORT", default_value = "587")]
    smtp_port: u16,
    /// SMTP username. Defaults to `SLOC_SMTP_USER` env var.
    #[arg(long, value_name = "USER", env = "SLOC_SMTP_USER")]
    smtp_user: Option<String>,
    /// SMTP password. Defaults to `SLOC_SMTP_PASS` env var.
    #[arg(long, value_name = "PASS", env = "SLOC_SMTP_PASS")]
    smtp_pass: Option<String>,

    // --- Webhook ---
    /// POST the JSON result to this URL (repeatable).
    #[arg(long, value_name = "URL")]
    webhook_url: Vec<String>,
    /// Bearer token for webhook auth. Defaults to `SLOC_WEBHOOK_TOKEN` env var.
    #[arg(long, value_name = "TOKEN", env = "SLOC_WEBHOOK_TOKEN")]
    webhook_token: Option<String>,
    /// Allow HTTP scheme and private/RFC-1918 IP addresses for webhook delivery.
    /// Also settable via `SLOC_ALLOW_PRIVATE_WEBHOOK=1`.
    /// Use in lab or corporate-intranet environments where the receiver is not publicly reachable.
    #[arg(long, env = "SLOC_ALLOW_PRIVATE_WEBHOOK")]
    allow_private_net: bool,

    // --- Microsoft Teams ---
    /// Post an Adaptive Card summary to a Microsoft Teams Incoming Webhook URL (repeatable).
    /// Obtain the URL from Teams: channel → Connectors → Incoming Webhook.
    #[arg(long, value_name = "URL")]
    notify_teams: Vec<String>,
    /// Optional URL linking to the full HTML report, included in the Teams card.
    #[arg(long, value_name = "URL")]
    report_url: Option<String>,

    // --- Atlassian Confluence ---
    /// Confluence base URL (e.g. <https://myco.atlassian.net> or <https://confluence.corp.com>).
    /// Defaults to `SLOC_CONFLUENCE_URL` env var.
    #[arg(long, value_name = "URL", env = "SLOC_CONFLUENCE_URL")]
    confluence_url: Option<String>,
    /// Atlassian account email (Cloud) or username (Server).
    /// Defaults to `SLOC_CONFLUENCE_USER` env var.
    #[arg(long, value_name = "USER", env = "SLOC_CONFLUENCE_USER")]
    confluence_username: Option<String>,
    /// API token (Cloud) or password/PAT (Server).
    /// Prefer the `SLOC_CONFLUENCE_TOKEN` env var to avoid credential exposure in process listings.
    #[arg(long, value_name = "TOKEN", env = "SLOC_CONFLUENCE_TOKEN")]
    confluence_token: Option<String>,
    /// Target Confluence space key (e.g. ENG). Defaults to `SLOC_CONFLUENCE_SPACE` env var.
    #[arg(long, value_name = "KEY", env = "SLOC_CONFLUENCE_SPACE")]
    confluence_space: Option<String>,
    /// Optional numeric parent page ID to nest the created page under.
    #[arg(long, value_name = "ID")]
    confluence_parent_id: Option<String>,
    /// Title of the Confluence page to create or update.
    #[arg(long, value_name = "TITLE")]
    confluence_page_title: Option<String>,
    /// URL linking to the full oxide-sloc HTML report, embedded in the Confluence page body.
    /// Defaults to --report-url if set.
    #[arg(long, value_name = "URL")]
    confluence_report_url: Option<String>,
}

// ── pr-comment ────────────────────────────────────────────────────────────────

/// Which VCS hosting the pull request lives on.
#[derive(Debug, Clone, clap::ValueEnum)]
enum VcsProvider {
    #[value(name = "github")]
    GitHub,
    #[value(name = "gitlab")]
    GitLab,
    #[value(name = "bitbucket")]
    Bitbucket,
}

#[derive(Debug, Args)]
struct PrCommentArgs {
    /// Path to the current scan JSON (the PR head).
    #[arg(value_name = "CURRENT_JSON")]
    current: PathBuf,

    /// Path to the baseline scan JSON (the target branch). Optional — if omitted,
    /// the comment shows absolute counts without a delta section.
    #[arg(long, value_name = "BASELINE_JSON")]
    baseline: Option<PathBuf>,

    /// VCS provider.
    #[arg(long, value_enum, default_value = "github")]
    provider: VcsProvider,

    /// API base URL. Defaults to `https://api.github.com` (GitHub),
    /// `https://gitlab.com` (GitLab), or `https://api.bitbucket.org` (Bitbucket).
    /// Override for self-hosted / Server-Data-Center instances.
    #[arg(long, value_name = "URL", env = "SLOC_VCS_API_URL")]
    api_url: Option<String>,

    /// Repository. `owner/repo` (GitHub), numeric project ID or `namespace/project`
    /// (GitLab), repo slug (Bitbucket Cloud, with --workspace), or `PROJECT/repo`
    /// (Bitbucket Server/DC).
    #[arg(long, value_name = "REPO", env = "SLOC_VCS_REPO")]
    repo: String,

    /// Pull request / merge request number.
    #[arg(long, value_name = "NUMBER", env = "SLOC_PR_NUMBER")]
    pr_number: u64,

    /// API token with `repo` (GitHub), `api` (GitLab), or PR-write (Bitbucket) scope.
    /// Defaults to `SLOC_VCS_TOKEN` env var.
    #[arg(long, value_name = "TOKEN", env = "SLOC_VCS_TOKEN")]
    token: String,

    /// Bitbucket Cloud workspace (ignored by GitHub/GitLab and Bitbucket Server/DC).
    #[arg(long, value_name = "WORKSPACE", env = "SLOC_BB_WORKSPACE")]
    workspace: Option<String>,

    /// Bitbucket username for HTTP Basic (Cloud app passwords). Omit to use a
    /// Bearer access token / Server PAT. Ignored by GitHub/GitLab.
    #[arg(long, value_name = "USER", env = "SLOC_BB_USER")]
    bitbucket_user: Option<String>,

    /// Allow HTTP scheme and private/RFC-1918 addresses (self-hosted / on-prem hosts).
    /// Also settable via `SLOC_ALLOW_PRIVATE_WEBHOOK=1`.
    #[arg(long, env = "SLOC_ALLOW_PRIVATE_WEBHOOK")]
    allow_private_net: bool,

    /// Optional URL linking to the full HTML report, appended to the comment.
    #[arg(long, value_name = "URL")]
    report_url: Option<String>,
}

// ── jira ──────────────────────────────────────────────────────────────────────

/// How the SLOC summary is attached to the target Jira issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum JiraMode {
    /// Add a comment rendering the SLOC summary on the issue.
    #[value(name = "comment")]
    Comment,
    /// Attach a "remote link" pointing at the full HTML report.
    #[value(name = "remote-link")]
    RemoteLink,
    /// Write a headline metric into a custom field (requires --field-id).
    #[value(name = "field")]
    Field,
}

#[derive(Debug, Args)]
struct JiraArgs {
    /// Path to the current scan JSON produced by `analyze --json-out`.
    #[arg(value_name = "RESULT_JSON")]
    current: PathBuf,

    /// Optional baseline scan JSON. When set, a delta section is added to the comment.
    #[arg(long, value_name = "BASELINE_JSON")]
    baseline: Option<PathBuf>,

    /// Jira base URL (e.g. <https://jira.corp.com> or <https://myco.atlassian.net>).
    /// Defaults to `SLOC_JIRA_URL` env var.
    #[arg(long, value_name = "URL", env = "SLOC_JIRA_URL")]
    jira_url: Option<String>,

    /// Jira account email (Cloud) or username (Server). Omit to use a Bearer PAT.
    /// Defaults to `SLOC_JIRA_USER` env var.
    #[arg(long, value_name = "USER", env = "SLOC_JIRA_USER")]
    jira_username: Option<String>,

    /// API token (Cloud) or personal access token / password (Server).
    /// Prefer the `SLOC_JIRA_TOKEN` env var to avoid credential exposure in process listings.
    #[arg(long, value_name = "TOKEN", env = "SLOC_JIRA_TOKEN")]
    jira_token: Option<String>,

    /// Target issue key (e.g. ENG-1234). Defaults to `SLOC_JIRA_ISSUE` env var.
    #[arg(long, value_name = "KEY", env = "SLOC_JIRA_ISSUE")]
    issue_key: Option<String>,

    /// How to attach the summary to the issue.
    #[arg(long, value_enum, default_value = "comment")]
    mode: JiraMode,

    /// Custom field id to write when --mode field (e.g. customfield_10050).
    #[arg(long, value_name = "ID")]
    field_id: Option<String>,

    /// URL linking to the full HTML report (embedded in the comment / used as the
    /// remote-link href).
    #[arg(long, value_name = "URL")]
    report_url: Option<String>,

    /// Allow HTTP scheme and private/RFC-1918 addresses (on-prem Jira hosts).
    /// Also settable via `SLOC_ALLOW_PRIVATE_WEBHOOK=1`.
    #[arg(long, env = "SLOC_ALLOW_PRIVATE_WEBHOOK")]
    allow_private_net: bool,

    /// Print the HTTP method, URL, and redacted body without making a network call.
    #[arg(long)]
    dry_run: bool,
}

// ── bitbucket-status ────────────────────────────────────────────────────────--

/// Build outcome reported to Bitbucket for a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum BuildState {
    #[value(name = "SUCCESSFUL", alias = "successful")]
    Successful,
    #[value(name = "FAILED", alias = "failed")]
    Failed,
    #[value(name = "INPROGRESS", alias = "inprogress")]
    InProgress,
}

impl BuildState {
    /// Bitbucket's `state` token. Cloud and Server/DC share the same vocabulary.
    fn as_token(self) -> &'static str {
        match self {
            BuildState::Successful => "SUCCESSFUL",
            BuildState::Failed => "FAILED",
            BuildState::InProgress => "INPROGRESS",
        }
    }
}

#[derive(Debug, Args)]
struct BitbucketStatusArgs {
    /// Path to the current scan JSON produced by `analyze --json-out`.
    #[arg(value_name = "RESULT_JSON")]
    current: PathBuf,

    /// Build state to report.
    #[arg(long, value_enum, default_value = "SUCCESSFUL")]
    state: BuildState,

    /// Bitbucket base URL. Defaults to `https://api.bitbucket.org` (Cloud) or
    /// `SLOC_BB_URL` env var. Point at your Server/DC host for on-prem.
    #[arg(long, value_name = "URL", env = "SLOC_BB_URL")]
    bitbucket_url: Option<String>,

    /// Bitbucket Cloud workspace (Cloud only; ignored on Server/DC).
    #[arg(long, value_name = "WORKSPACE", env = "SLOC_BB_WORKSPACE")]
    workspace: Option<String>,

    /// Repository: repo slug (Cloud, with --workspace) or `PROJECT/repo` (Server/DC).
    #[arg(long, value_name = "REPO", env = "SLOC_BB_REPO")]
    repo: String,

    /// Commit SHA to attach the build status to. Defaults to `SLOC_BB_COMMIT` env var.
    #[arg(long, value_name = "SHA", env = "SLOC_BB_COMMIT")]
    commit: Option<String>,

    /// Bitbucket username for HTTP Basic (Cloud app passwords). Omit to use a
    /// Bearer access token / Server PAT.
    #[arg(long, value_name = "USER", env = "SLOC_BB_USER")]
    bitbucket_user: Option<String>,

    /// API token / app password / PAT. Defaults to `SLOC_BB_TOKEN` env var.
    #[arg(long, value_name = "TOKEN", env = "SLOC_BB_TOKEN")]
    token: Option<String>,

    /// Status key (stable identifier for this check). Default: `oxide-sloc`.
    #[arg(long, value_name = "KEY", default_value = "oxide-sloc")]
    key: String,

    /// URL linking to the full HTML report (the build-status target URL).
    #[arg(long, value_name = "URL")]
    report_url: Option<String>,

    /// Allow HTTP scheme and private/RFC-1918 addresses (on-prem Bitbucket hosts).
    /// Also settable via `SLOC_ALLOW_PRIVATE_WEBHOOK=1`.
    #[arg(long, env = "SLOC_ALLOW_PRIVATE_WEBHOOK")]
    allow_private_net: bool,

    /// Print the HTTP method, URL, and redacted body without making a network call.
    #[arg(long)]
    dry_run: bool,
}

// ── git-scan ──────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct GitScanArgs {
    /// Repository URL or local path
    #[arg(value_name = "REPO")]
    repo: String,

    /// Branch, tag, or commit SHA to scan (default: HEAD / default branch)
    #[arg(long, default_value = "HEAD", value_name = "REF")]
    git_ref: String,

    /// Directory to cache cloned repositories
    #[arg(long, value_name = "DIR")]
    clones_dir: Option<PathBuf>,

    /// Allow offline import from a git bundle, file:// mirror, or local path
    /// (sets SLOC_GIT_ALLOW_LOCAL; requires --local-root or SLOC_GIT_LOCAL_ROOT)
    #[arg(long)]
    allow_local: bool,

    /// Directory local/offline sources must resolve under (sets SLOC_GIT_LOCAL_ROOT)
    #[arg(long, value_name = "DIR")]
    local_root: Option<PathBuf>,

    /// Write JSON result to this path
    #[arg(long, short = 'j', value_name = "PATH")]
    json_out: Option<PathBuf>,

    /// Write HTML report to this path
    #[arg(long, short = 'H', value_name = "PATH")]
    html_out: Option<PathBuf>,

    /// Write CSV summary to this path
    #[arg(long, short = 'c', value_name = "PATH")]
    csv_out: Option<PathBuf>,

    /// Machine-readable key=value terminal output
    #[arg(long)]
    plain: bool,

    /// Suppress all output except errors
    #[arg(long, short = 'q')]
    quiet: bool,
}

// ── git-compare ───────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct GitCompareArgs {
    /// Repository URL or local path
    #[arg(value_name = "REPO")]
    repo: String,

    /// Baseline (older) ref — branch, tag, or commit SHA
    #[arg(value_name = "BASELINE_REF")]
    baseline_ref: String,

    /// Current (newer) ref — branch, tag, or commit SHA
    #[arg(value_name = "CURRENT_REF")]
    current_ref: String,

    /// Directory to cache cloned repositories
    #[arg(long, value_name = "DIR")]
    clones_dir: Option<PathBuf>,

    /// Allow offline import from a git bundle, file:// mirror, or local path
    /// (sets SLOC_GIT_ALLOW_LOCAL; requires --local-root or SLOC_GIT_LOCAL_ROOT)
    #[arg(long)]
    allow_local: bool,

    /// Directory local/offline sources must resolve under (sets SLOC_GIT_LOCAL_ROOT)
    #[arg(long, value_name = "DIR")]
    local_root: Option<PathBuf>,

    /// Write delta JSON to this path
    #[arg(long, short = 'j', value_name = "PATH")]
    json_out: Option<PathBuf>,

    /// Write delta CSV to this path
    #[arg(long, short = 'c', value_name = "PATH")]
    csv_out: Option<PathBuf>,

    /// Machine-readable key=value terminal output
    #[arg(long)]
    plain: bool,

    /// Suppress all output except errors
    #[arg(long, short = 'q')]
    quiet: bool,
}

// ── watch ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
struct WatchArgs {
    /// Repository URL or local path to monitor
    #[arg(value_name = "REPO")]
    repo: String,

    /// Branch to watch for new commits
    #[arg(long, default_value = "main", value_name = "BRANCH")]
    branch: String,

    /// Poll interval in seconds (minimum 60)
    #[arg(long, default_value = "300", value_name = "SECS")]
    interval: u64,

    /// Directory to cache cloned repositories
    #[arg(long, value_name = "DIR")]
    clones_dir: Option<PathBuf>,

    /// Allow offline import from a git bundle, file:// mirror, or local path
    /// (sets SLOC_GIT_ALLOW_LOCAL; requires --local-root or SLOC_GIT_LOCAL_ROOT)
    #[arg(long)]
    allow_local: bool,

    /// Directory local/offline sources must resolve under (sets SLOC_GIT_LOCAL_ROOT)
    #[arg(long, value_name = "DIR")]
    local_root: Option<PathBuf>,

    /// Write each scan's JSON result to this directory
    #[arg(long, value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// Suppress non-error output
    #[arg(long, short = 'q')]
    quiet: bool,
}

// ── prune ───────────────────────────────────────────────────────────────────────

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Args)]
struct PruneArgs {
    /// Delete scan runs older than this many days
    #[arg(long, value_name = "DAYS")]
    older_than: Option<u32>,

    /// Keep only the N most recent scan runs; delete the rest
    #[arg(long, value_name = "N")]
    keep_last: Option<u32>,

    /// Output tree to prune (defaults to the same location the web server uses:
    /// `$OXIDE_SLOC_ROOT/out/web`, or `./out/web`)
    #[arg(long, value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// Also rotate/remove the audit log (`$SLOC_AUDIT_LOG`) and its rotated history
    #[arg(long)]
    logs: bool,

    /// Actually delete. Without this flag prune only reports what it *would* remove.
    #[arg(long, short = 'y')]
    yes: bool,

    /// Emit a machine-readable JSON summary instead of human text
    #[arg(long)]
    json: bool,
}

// ── entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("warn,headless_chrome::browser::transport=error")
            }),
        )
        .init();

    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Serve(ServeArgs {
        config: None,
        bind: None,
        server: false,
    })) {
        Commands::Analyze(args) => run_analyze(*args).await,
        Commands::Report(args) => run_report(&args),
        Commands::Diff(args) => run_diff(&args),
        Commands::Serve(args) => run_serve(args).await,
        Commands::Init(args) => run_init(&args),
        Commands::Validate(args) => run_validate(&args),
        Commands::Send(args) => run_send(*args).await,
        Commands::Bundle(args) => run_bundle(&args),
        Commands::GitScan(args) => run_git_scan(args).await,
        Commands::GitCompare(args) => run_git_compare(args),
        Commands::Watch(args) => run_watch(args).await,
        Commands::Prune(args) => run_prune(&args),
        Commands::VerifyAudit(args) => run_verify_audit(&args),
        Commands::PrComment(args) => run_pr_comment(args).await,
        Commands::Jira(args) => run_jira(args).await,
        Commands::BitbucketStatus(args) => run_bitbucket_status(args).await,
        Commands::Healthz(args) => run_healthz(args).await,
        Commands::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "oxide-sloc",
                &mut std::io::stdout(),
            );
            Ok(())
        }
    }
}

// ── verify-audit handler ──────────────────────────────────────────────────────

/// Resolve the default health-probe URL from `SLOC_BIND` (falling back to the
/// default bind), normalising a wildcard host to loopback so the probe connects.
fn default_healthz_url() -> String {
    let bind = std::env::var("SLOC_BIND").unwrap_or_else(|_| "127.0.0.1:4317".to_string());
    let host_port = bind
        .replace("0.0.0.0", "127.0.0.1")
        .replace("[::]", "[::1]");
    format!("http://{host_port}/healthz")
}

/// Probe the server's `/healthz` endpoint. Prints a one-line status and exits 0
/// when healthy; exits 1 (via a non-zero process code) when unreachable or the
/// endpoint returns a non-2xx status. Used by Docker's HEALTHCHECK.
async fn run_healthz(args: HealthzArgs) -> Result<()> {
    let url = args.url.unwrap_or_else(default_healthz_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(args.timeout))
        .build()
        .context("failed to build HTTP client")?;
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!("healthy: {url} -> {}", resp.status());
            Ok(())
        }
        Ok(resp) => {
            eprintln!("unhealthy: {url} -> {}", resp.status());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("unhealthy: {url} -> {e}");
            std::process::exit(1);
        }
    }
}

fn run_verify_audit(args: &VerifyAuditArgs) -> Result<()> {
    let log = args
        .log
        .clone()
        .or_else(|| std::env::var_os("SLOC_AUDIT_LOG").map(PathBuf::from))
        .context("no audit log path given; pass one as an argument or set SLOC_AUDIT_LOG")?;
    let key = args
        .key
        .clone()
        .or_else(|| std::env::var("SLOC_AUDIT_HMAC_KEY").ok())
        .filter(|s| !s.is_empty())
        .context(
            "no integrity key given; pass --key or set SLOC_AUDIT_HMAC_KEY (the same \
             value used when the log was written)",
        )?;

    let report = sloc_web::verify_audit_file(&log, &key);
    if report.ok {
        println!(
            "OK: {} record(s) verified — chain intact ({})",
            report.records,
            log.display()
        );
        Ok(())
    } else {
        let detail = report.detail.as_deref().unwrap_or("verification failed");
        match report.first_bad_line {
            Some(line) => eprintln!(
                "FAIL: {} ({}:{}) after {} record(s)",
                detail,
                log.display(),
                line,
                report.records.saturating_sub(1)
            ),
            None => eprintln!("FAIL: {} ({})", detail, log.display()),
        }
        // Non-zero exit so CI / SIEM tooling can gate on tamper detection.
        std::process::exit(2);
    }
}

// ── analyze handler ───────────────────────────────────────────────────────────

fn log_written(path: &Path, quiet: bool) {
    if !quiet {
        eprintln!("wrote {}", path.display());
    }
}

/// Write all requested output artifacts and print paths when not quiet.
fn write_outputs(run: &AnalysisRun, args: &AnalyzeArgs, quiet: bool) -> Result<()> {
    if let Some(path) = &args.json_out {
        write_json(run, path)?;
        log_written(path, quiet);
    }

    if let Some(path) = &args.html_out {
        // Embed a relative link to the PDF when both outputs land in the same
        // directory — lets "View PDF" open the pre-generated file from Jenkins
        // HTML Publisher (or any static host) without a live oxide-sloc server.
        write_html_with_pdf_link(run, path, args.pdf_out.as_deref())?;
        log_written(path, quiet);
        if args.open {
            open_path(path);
        }
    }

    if let Some(path) = &args.pdf_out {
        write_pdf_from_run(run, path)?;
        log_written(path, quiet);
    }

    if let Some(path) = &args.csv_out {
        write_csv(run, path)?;
        log_written(path, quiet);
    }

    if let Some(path) = &args.xlsx_out {
        write_xlsx(run, path)?;
        log_written(path, quiet);
    }

    if let Some(path) = &args.scan_config_out {
        write_scan_config(run, path, args)?;
        log_written(path, quiet);
    }

    if let Some(dir) = &args.sub_html_out_dir {
        write_sub_html_reports(run, dir, quiet)?;
    }

    Ok(())
}

fn write_scan_config(run: &AnalysisRun, path: &Path, args: &AnalyzeArgs) -> Result<()> {
    let policy_str = serde_json::to_value(run.effective_configuration.analysis.mixed_line_policy)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "code_only".to_string());
    let behavior_str =
        serde_json::to_value(run.effective_configuration.analysis.binary_file_behavior)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "skip".to_string());

    let json = serde_json::to_string_pretty(&serde_json::json!({
        "oxide_sloc_version": env!("CARGO_PKG_VERSION"),
        "path": run.input_roots.first().cloned().unwrap_or_default(),
        "include_globs": run.effective_configuration.discovery.include_globs.join("\n"),
        "exclude_globs": run.effective_configuration.discovery.exclude_globs.join("\n"),
        "submodule_breakdown": run.effective_configuration.discovery.submodule_breakdown,
        "mixed_line_policy": policy_str,
        "python_docstrings_as_comments":
            run.effective_configuration.analysis.python_docstrings_as_comments,
        "generated_file_detection":
            run.effective_configuration.analysis.generated_file_detection,
        "minified_file_detection":
            run.effective_configuration.analysis.minified_file_detection,
        "vendor_directory_detection":
            run.effective_configuration.analysis.vendor_directory_detection,
        "include_lockfiles": run.effective_configuration.analysis.include_lockfiles,
        "binary_file_behavior": behavior_str,
        "output_dir": path.parent().and_then(|p| p.to_str()).unwrap_or(""),
        "report_title": run.effective_configuration.reporting.report_title,
        "generate_html": args.html_out.is_some(),
        "generate_pdf": args.pdf_out.is_some(),
    }))
    .context("scan-config serialization failed")?;
    std::fs::write(path, json).with_context(|| format!("writing scan-config to {}", path.display()))
}

fn write_sub_html_reports(run: &AnalysisRun, dir: &Path, quiet: bool) -> Result<()> {
    if run.submodule_summaries.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating sub-html dir {}", dir.display()))?;
    let parent_path = run.input_roots.first().map_or("", String::as_str);
    for sub in &run.submodule_summaries {
        let safe = sloc_web::sanitize_project_label(&sub.name);
        let sub_run = sloc_web::build_sub_run(run, sub, parent_path);
        let html = sloc_report::render_sub_report_html(&sub_run, None)
            .with_context(|| format!("rendering sub-report for '{}'", sub.name))?;
        let out_path = dir.join(format!("sub_{safe}.html"));
        std::fs::write(&out_path, html.as_bytes())
            .with_context(|| format!("writing sub-report to {}", out_path.display()))?;
        log_written(&out_path, quiet);
    }
    Ok(())
}

/// Check threshold and warning-count exit conditions after outputs are written.
fn check_exit_conditions(
    run: &AnalysisRun,
    fail_on_warnings: bool,
    fail_below: Option<u64>,
    fail_on_budget: bool,
) {
    if fail_on_warnings && !run.warnings.is_empty() {
        eprintln!(
            "error: {} warning(s) found — failing due to --fail-on-warnings",
            run.warnings.len()
        );
        std::process::exit(2);
    }

    if let Some(threshold) = fail_below
        && run.summary_totals.code_lines < threshold
    {
        eprintln!(
            "error: code lines ({}) below threshold {} (--fail-below)",
            run.summary_totals.code_lines, threshold
        );
        std::process::exit(3);
    }

    if fail_on_budget && let Some(budget) = &run.effective_configuration.analysis.budget {
        check_budget(run, budget);
    }
}

// Multiple conditions (total and per-language) each set `violated`; an if-let-seq refactor
// would be less clear here since both conditions need to print their own error messages.
#[allow(clippy::useless_let_if_seq)]
fn check_budget(run: &AnalysisRun, budget: &sloc_config::BudgetConfig) {
    let mut violated = false;

    if budget.total_max > 0 && run.summary_totals.code_lines > budget.total_max {
        eprintln!(
            "error: budget exceeded — total code lines {} > limit {} (--fail-on-budget)",
            run.summary_totals.code_lines, budget.total_max
        );
        violated = true;
    }

    for lang_row in &run.totals_by_language {
        let key = lang_row.language.display_name().to_ascii_lowercase();
        if let Some(&limit) = budget.per_language.get(&key)
            && lang_row.code_lines > limit
        {
            eprintln!(
                "error: budget exceeded — {} code lines {} > limit {} (--fail-on-budget)",
                lang_row.language.display_name(),
                lang_row.code_lines,
                limit
            );
            violated = true;
        }
    }

    if violated {
        std::process::exit(4);
    }
}

fn apply_complexity_gate(run: &AnalysisRun, max_cc: u32) {
    let violators: Vec<_> = run
        .per_file_records
        .iter()
        .filter(|f| f.cyclomatic_complexity.is_some_and(|cc| cc > max_cc))
        .collect();
    if !violators.is_empty() {
        eprintln!(
            "error: {} file(s) exceed --max-complexity {} (exit 6)",
            violators.len(),
            max_cc
        );
        for f in violators.iter().take(10) {
            eprintln!(
                "  {} complexity={}",
                f.relative_path,
                f.cyclomatic_complexity.unwrap_or(0)
            );
        }
        std::process::exit(6);
    }
}

async fn run_analyze(args: AnalyzeArgs) -> Result<()> {
    let config = resolve_analyze_config(&args)?;
    let quiet = args.quiet;
    let mut run = tokio::task::spawn_blocking(move || analyze(&config, "analyze", None, None))
        .await
        .context("analysis task failed to join")??;

    // Allow explicit CI override of the git branch when auto-detection yields
    // nothing (e.g. detached HEAD checkouts in Jenkins with no env vars set).
    if let Some(ref branch) = args.git_branch
        && !branch.is_empty()
    {
        run.git_branch = Some(branch.clone());
    }

    if !quiet {
        print_summary(&run, args.per_file, args.plain);
    }

    write_outputs(&run, &args, quiet)?;

    // Save baseline snapshot if requested.
    if let Some(name) = &args.set_baseline {
        let baselines_path = resolve_baselines_path();
        let mut store = BaselineStore::load(&baselines_path);
        store.set(BaselineEntry {
            name: name.clone(),
            saved_at: chrono::Utc::now(),
            run_id: run.tool.run_id.clone(),
            summary: sloc_core::ScanSummarySnapshot::from(&run.summary_totals),
            json_path: args.json_out.clone(),
        });
        store.save(&baselines_path)?;
        if !quiet {
            eprintln!("baseline '{}' saved → {}", name, baselines_path.display());
        }
    }

    // Threshold / warning exit codes (checked after all outputs are written)
    check_exit_conditions(
        &run,
        args.fail_on_warnings,
        args.fail_below,
        args.fail_on_budget,
    );

    // Cyclomatic complexity gate: exit 6 if any file exceeds the threshold.
    if let Some(max_cc) = args.max_complexity {
        apply_complexity_gate(&run, max_cc);
    }

    // Baseline growth check.
    if let Some(baseline_name) = &args.fail_above_baseline {
        let baselines_path = resolve_baselines_path();
        let store = BaselineStore::load(&baselines_path);
        let result = check_against_baseline(
            &store,
            baseline_name,
            run.summary_totals.code_lines,
            args.max_delta_pct,
        )?;
        result.print_summary();
        if result.exceeded {
            std::process::exit(5);
        }
    }

    Ok(())
}

// ── report handler ────────────────────────────────────────────────────────────

fn run_report(args: &ReportArgs) -> Result<()> {
    let run = read_json(&args.input)?;

    if args.html_out.is_none()
        && args.pdf_out.is_none()
        && args.csv_out.is_none()
        && args.xlsx_out.is_none()
    {
        anyhow::bail!("provide at least one of --html-out, --pdf-out, --csv-out, --xlsx-out");
    }

    if let Some(path) = &args.html_out {
        write_html(&run, path)?;
        eprintln!("wrote {}", path.display());
        if args.open {
            open_path(path);
        }
    }

    if let Some(path) = &args.pdf_out {
        write_pdf_from_run(&run, path)?;
        eprintln!("wrote {}", path.display());
    }

    if let Some(path) = &args.csv_out {
        write_csv(&run, path)?;
        eprintln!("wrote {}", path.display());
    }

    if let Some(path) = &args.xlsx_out {
        write_xlsx(&run, path)?;
        eprintln!("wrote {}", path.display());
    }

    Ok(())
}

// ── bundle handler ────────────────────────────────────────────────────────────

fn run_bundle(args: &BundleArgs) -> Result<()> {
    let run = read_json(&args.input)
        .with_context(|| format!("failed to read result JSON: {}", args.input.display()))?;

    // Resolve the run id: explicit flag, else the run_id embedded in the JSON, else a
    // fresh UUID (uuid is already a CLI dependency — reuse it, no new dep).
    let run_id = args
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                let embedded = run.tool.run_id.trim();
                if embedded.is_empty() {
                    uuid::Uuid::new_v4().simple().to_string()
                } else {
                    embedded.to_string()
                }
            },
            String::from,
        );

    let run_dir = sloc_web::bundle_run(&run, &args.out_dir, &run_id, args.label.as_deref())
        .context("failed to bundle run into local web-UI layout")?;

    // The created run directory is the actionable output — print it to stdout.
    println!("{}", run_dir.display());
    Ok(())
}

// ── diff handler ──────────────────────────────────────────────────────────────

fn run_diff(args: &DiffArgs) -> Result<()> {
    let baseline = read_json(&args.baseline)
        .with_context(|| format!("failed to read baseline: {}", args.baseline.display()))?;
    let current = read_json(&args.current)
        .with_context(|| format!("failed to read current: {}", args.current.display()))?;

    let comparison = compute_delta(&baseline, &current);

    if !args.quiet {
        print_diff_summary(&comparison, args.plain);
    }

    if let Some(path) = &args.json_out {
        let json = serde_json::to_string_pretty(&comparison)
            .context("failed to serialize diff to JSON")?;
        std::fs::write(path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
        eprintln!("wrote {}", path.display());
    }

    if let Some(path) = &args.csv_out {
        write_diff_csv(&comparison, path)?;
        eprintln!("wrote {}", path.display());
    }

    if let Some(path) = &args.xlsx_out {
        write_diff_xlsx(&comparison, path)?;
        eprintln!("wrote {}", path.display());
    }

    Ok(())
}

// ── serve handler ─────────────────────────────────────────────────────────────

async fn run_serve(args: ServeArgs) -> Result<()> {
    let mut config = load_base_config(args.config.as_deref())?;
    // SLOC_BIND overrides the config file but is itself overridden by --bind.
    let bind_env = std::env::var("SLOC_BIND").ok().filter(|s| !s.is_empty());
    if args.server {
        config.web.server_mode = true;
        if args.bind.is_none()
            && bind_env.is_none()
            && config.web.bind_address.starts_with("127.0.0.1")
        {
            config.web.bind_address = "0.0.0.0:4317".into();
        }
    }
    if let Some(bind) = bind_env {
        config.web.bind_address = bind;
    }
    if let Some(bind) = args.bind {
        config.web.bind_address = bind;
    }
    if let Ok(roots_env) = std::env::var("SLOC_ALLOWED_ROOTS") {
        let roots: Vec<std::path::PathBuf> = roots_env
            .split(':')
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .collect();
        if !roots.is_empty() {
            config.discovery.allowed_scan_roots = roots;
        }
    }
    sloc_web::serve(config).await
}

// ── init handler ──────────────────────────────────────────────────────────────

fn run_init(args: &InitArgs) -> Result<()> {
    if args.output.exists() && !args.force {
        anyhow::bail!(
            "{} already exists; use --force to overwrite",
            args.output.display()
        );
    }

    let content = r##"# oxide-sloc configuration
# Generated by `oxide-sloc init`. Uncomment and adjust as needed.
# Full reference: https://github.com/oxide-sloc/oxide-sloc

[discovery]
# root_paths = ["."]
# include_globs = []
# exclude_globs = []
# excluded_directories = [".git", "node_modules", "target", "vendor"]
# honor_ignore_files = true
# ignore_hidden_files = true
# follow_symlinks = false
# max_file_size_bytes = 2097152   # 2 MB
# submodule_breakdown = false

[analysis]
# enabled_languages = []   # empty = all 60 supported languages
# mixed_line_policy = "code-only"   # code-only | code-and-comment | comment-only | separate-mixed-category
# python_docstrings_as_comments = true
# generated_file_detection = true
# minified_file_detection = true
# vendor_directory_detection = true
# include_lockfiles = false
#
# IEEE 1045-1992 counting parameters:
# continuation_line_policy = "each-physical-line"  # each-physical-line | collapse-to-logical
# blank_in_block_comment_policy = "count-as-comment"  # count-as-comment | count-as-blank
# count_compiler_directives = true   # false = exclude #include/#define from code SLOC (C/C++/ObjC)

# Override extension → language mappings (e.g. treat .h as C++)
# [analysis.extension_overrides]
# "h" = "cpp"

# SLOC budget thresholds — fail CI with --fail-on-budget if exceeded.
# [analysis.budget]
# total_max = 100000        # maximum code lines across all languages (0 = unlimited)
# rust = 60000              # per-language ceiling; key = lowercase display name
# typescript = 30000

[reporting]
# report_title = "OxideSLOC Report"
# theme = "auto"   # auto | light | dark
# include_summary_charts = true
# include_skipped_files_section = true
# include_warnings_section = true
#
# Team branding (all optional):
# company_name = "Acme Corp"        # replaces "OxideSLOC" in the report header
# logo_path = "assets/logo.png"     # PNG or SVG to embed in place of the default logo
# accent_color = "#3b82f6"          # primary accent colour (#RGB or #RRGGBB)

[web]
# bind_address = "127.0.0.1:4317"
# server_mode = false

# ── Named profiles ───────────────────────────────────────────────────────────
# Use `oxide-sloc analyze --profile frontend` to apply a profile.
# Each profile overrides its entire config section when selected.
#
# [profile.frontend]
# [profile.frontend.discovery]
# root_paths = ["frontend"]
# exclude_globs = ["**/node_modules/**", "**/dist/**"]
# [profile.frontend.analysis]
# enabled_languages = ["TypeScript", "JavaScript", "CSS", "HTML"]
# [profile.frontend.reporting]
# report_title = "Frontend SLOC Report"
#
# [profile.backend]
# [profile.backend.discovery]
# root_paths = ["backend", "shared"]
# [profile.backend.analysis]
# enabled_languages = ["Rust", "Python", "SQL"]
"##;

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    std::fs::write(&args.output, content)
        .with_context(|| format!("failed to write {}", args.output.display()))?;

    eprintln!("created {}", args.output.display());
    Ok(())
}

// ── validate handler ──────────────────────────────────────────────────────────

fn validate_path_list(paths: &[std::path::PathBuf], prefix: &str) -> Vec<String> {
    paths
        .iter()
        .filter(|p| !p.exists())
        .map(|p| format!("{prefix}: '{}' does not exist", p.display()))
        .collect()
}

fn validate_glob_list(patterns: &[String], prefix: &str) -> Vec<String> {
    patterns
        .iter()
        .filter(|p| globset::Glob::new(p).is_err())
        .map(|p| format!("{prefix}: invalid glob pattern '{p}'"))
        .collect()
}

fn run_validate(args: &ValidateArgs) -> Result<()> {
    let config_path = args
        .config
        .as_deref()
        .unwrap_or_else(|| std::path::Path::new(".oxide-sloc.toml"));

    if !config_path.exists() {
        anyhow::bail!(
            "config file not found: {} (use `oxide-sloc init` to create one)",
            config_path.display()
        );
    }

    let config = sloc_config::AppConfig::load_from_file(config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))?;

    let mut errors: Vec<String> = Vec::new();

    errors.extend(validate_path_list(
        &config.discovery.root_paths,
        "discovery.root_paths",
    ));
    errors.extend(validate_path_list(
        &config.discovery.allowed_scan_roots,
        "discovery.allowed_scan_roots",
    ));
    errors.extend(validate_glob_list(
        &config.discovery.include_globs,
        "discovery.include_globs",
    ));
    errors.extend(validate_glob_list(
        &config.discovery.exclude_globs,
        "discovery.exclude_globs",
    ));

    if let Some(logo) = &config.reporting.logo_path
        && !logo.exists()
    {
        errors.push(format!(
            "reporting.logo_path: '{}' does not exist",
            logo.display()
        ));
    }

    if let Some(color) = &config.reporting.accent_color
        && sloc_config::validate_hex_color(color).is_err()
    {
        errors.push(format!(
            "reporting.accent_color: '{color}' is not a valid hex colour (use #RGB or #RRGGBB)"
        ));
    }

    for name in config.profiles.keys() {
        if name.trim().is_empty() {
            errors.push("profiles: profile name must not be empty".into());
        }
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        anyhow::bail!("{} validation error(s) found", errors.len())
    }
    println!("config valid: {}", config_path.display());
    let profile_count = config.profiles.len();
    if profile_count > 0 {
        println!(
            "  {} profile(s): {}",
            profile_count,
            config
                .profiles
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

// ── send handler ──────────────────────────────────────────────────────────────

async fn run_send(args: SendArgs) -> Result<()> {
    if args.smtp_to.is_empty()
        && args.webhook_url.is_empty()
        && args.notify_teams.is_empty()
        && args.confluence_url.is_none()
    {
        anyhow::bail!(
            "provide at least one of --smtp-to, --webhook-url, --notify-teams, or --confluence-url"
        );
    }

    if args.smtp_pass.is_some() && std::env::var("SLOC_SMTP_PASS").is_err() {
        eprintln!(
            "WARNING: --smtp-pass exposes credentials in process listings. \
             Use the SLOC_SMTP_PASS environment variable instead."
        );
    }

    let run = read_json(&args.input)?;

    if !args.smtp_to.is_empty() {
        send_smtp(&args, &run).await?;
    }

    for url in &args.webhook_url {
        send_webhook(
            url,
            args.webhook_token.as_deref(),
            &run,
            args.allow_private_net,
        )
        .await?;
    }

    for url in &args.notify_teams {
        send_teams_card(
            url,
            &run,
            args.report_url.as_deref(),
            args.allow_private_net,
        )
        .await?;
    }

    if args.confluence_url.is_some() {
        send_confluence(&args, &run).await?;
    }

    println!("send: all deliveries completed");
    Ok(())
}

async fn send_smtp(args: &SendArgs, run: &AnalysisRun) -> Result<()> {
    let host = args.smtp_host.as_deref().ok_or_else(|| {
        anyhow::anyhow!("--smtp-host (or SLOC_SMTP_HOST) is required for SMTP delivery")
    })?;
    let from = args
        .smtp_from
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--smtp-from is required for SMTP delivery"))?;

    let html_body = render_html(run)?;
    let plain_body = format!(
        "oxide-sloc report: {} files analyzed, {} code lines\n\nSee attached HTML for the full report.",
        run.summary_totals.files_analyzed, run.summary_totals.code_lines,
    );

    for recipient in &args.smtp_to {
        let msg = Message::builder()
            .from(
                from.parse()
                    .with_context(|| format!("invalid from address: {from}"))?,
            )
            .to(recipient
                .parse()
                .with_context(|| format!("invalid recipient address: {recipient}"))?)
            .subject(format!(
                "oxide-sloc report — {}",
                run.effective_configuration.reporting.report_title
            ))
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(plain_body.clone()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html_body.clone()),
                    ),
            )
            .context("failed to build email message")?;

        let tls_params = lettre::transport::smtp::client::TlsParameters::builder(host.to_string())
            .dangerous_accept_invalid_certs(false)
            .build()
            .with_context(|| format!("failed to build TLS parameters for {host}"))?;
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .tls(lettre::transport::smtp::client::Tls::Required(tls_params))
            .port(args.smtp_port);

        if let (Some(user), Some(pass)) = (args.smtp_user.as_deref(), args.smtp_pass.as_deref()) {
            builder = builder.credentials(Credentials::new(user.to_owned(), pass.to_owned()));
        }

        let transport = builder.build();
        transport
            .send(msg)
            .await
            .with_context(|| format!("SMTP delivery to {recipient} failed"))?;

        println!("send: emailed {recipient}");
    }

    Ok(())
}

/// Hostnames that must never receive webhook payloads (cloud metadata endpoints, link-local names).
const BLOCKED_WEBHOOK_HOSTS: &[&str] = &[
    "169.254.169.254",
    "metadata.google.internal",
    "metadata.internal",
    "instance-data",
];

/// Returns `true` when `ip` falls into a range that must not receive outbound requests
/// (loopback, private RFC-1918/FC00, link-local, broadcast, unspecified, multicast).
const fn is_ip_blocked(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // Unique-local (FC00::/7)
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // Link-local (FE80::/10)
        }
    }
}

fn validate_webhook_url(raw: &str, allow_private_net: bool) -> Result<()> {
    let parsed = reqwest::Url::parse(raw).with_context(|| format!("invalid webhook URL: {raw}"))?;
    if allow_private_net {
        tracing::warn!(
            url = raw,
            "private-net webhook allowed — HTTPS and IP restrictions bypassed"
        );
        return Ok(());
    }
    if parsed.scheme() != "https" {
        anyhow::bail!(
            "webhook URL must use HTTPS (got scheme \"{}\"); \
             use --allow-private-net to send to HTTP endpoints",
            parsed.scheme()
        );
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("webhook URL has no host"))?;
    if BLOCKED_WEBHOOK_HOSTS.contains(&host) || host.to_ascii_lowercase().ends_with(".local") {
        anyhow::bail!("webhook URL host is blocked: {host}");
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>()
        && is_ip_blocked(ip)
    {
        anyhow::bail!(
            "webhook URL resolves to a blocked IP address: {ip}; \
                 use --allow-private-net to send to private addresses"
        );
    }
    Ok(())
}

async fn send_webhook(
    url: &str,
    token: Option<&str>,
    run: &AnalysisRun,
    allow_private_net: bool,
) -> Result<()> {
    validate_webhook_url(url, allow_private_net)?;

    let client = reqwest::Client::new();
    let mut req = client.post(url).json(run);

    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req
        .send()
        .await
        .with_context(|| format!("webhook POST to {url} failed"))?;

    if !resp.status().is_success() {
        anyhow::bail!("webhook {url} returned HTTP {}", resp.status());
    }

    println!("send: posted to {url}");
    Ok(())
}

fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        let pos_from_right = bytes.len() - 1 - i;
        if i > 0 && pos_from_right % 3 == 2 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Build and POST a Microsoft Teams Adaptive Card summary for the given run.
///
/// The payload uses the `attachments` envelope understood by both legacy
/// Incoming Webhook connectors and Power Automate-backed webhook flows.
async fn send_teams_card(
    url: &str,
    run: &AnalysisRun,
    report_url: Option<&str>,
    allow_private_net: bool,
) -> Result<()> {
    validate_webhook_url(url, allow_private_net)?;

    let totals = &run.summary_totals;
    let title = &run.effective_configuration.reporting.report_title;

    // Build a language summary string (top 5 languages).
    let lang_lines: String = run
        .totals_by_language
        .iter()
        .take(5)
        .map(|l| {
            format!(
                "- **{}**: {} code lines ({} files)",
                l.language.display_name(),
                fmt_thousands(l.code_lines),
                l.files
            )
        })
        .collect::<Vec<_>>()
        .join("  \n");

    let facts = serde_json::json!([
        { "title": "Files Analyzed", "value": totals.files_analyzed.to_string() },
        { "title": "Code Lines",     "value": fmt_thousands(totals.code_lines) },
        { "title": "Comment Lines",  "value": fmt_thousands(totals.comment_lines) },
        { "title": "Blank Lines",    "value": fmt_thousands(totals.blank_lines) },
        { "title": "Total Physical", "value": fmt_thousands(totals.total_physical_lines) },
    ]);

    let mut body_items = vec![
        serde_json::json!({
            "type": "TextBlock",
            "text": title,
            "weight": "Bolder",
            "size": "Medium"
        }),
        serde_json::json!({
            "type": "FactSet",
            "facts": facts
        }),
    ];

    if !lang_lines.is_empty() {
        body_items.push(serde_json::json!({
            "type": "TextBlock",
            "text": "**Top Languages**",
            "weight": "Bolder",
            "spacing": "Medium"
        }));
        body_items.push(serde_json::json!({
            "type": "TextBlock",
            "text": lang_lines,
            "wrap": true
        }));
    }

    let mut actions: Vec<serde_json::Value> = Vec::new();
    if let Some(link) = report_url {
        actions.push(serde_json::json!({
            "type": "Action.OpenUrl",
            "title": "View Full Report",
            "url": link
        }));
    }

    let card = serde_json::json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "content": {
                "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                "type": "AdaptiveCard",
                "version": "1.4",
                "body": body_items,
                "actions": actions
            }
        }]
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(&card)
        .send()
        .await
        .with_context(|| format!("Teams webhook POST to {url} failed"))?;

    if !resp.status().is_success() {
        anyhow::bail!("Teams webhook {url} returned HTTP {}", resp.status());
    }

    println!("send: posted Teams card to {url}");
    Ok(())
}

// ── config helpers ────────────────────────────────────────────────────────────

fn load_base_config(config_path: Option<&Path>) -> Result<AppConfig> {
    let config = config_path.map_or_else(|| Ok(AppConfig::default()), AppConfig::load_from_file)?;
    // Propagate the non-secret `[git]` offline-import gate into the env sloc-git reads
    // (explicit env vars still win). Done here so `serve`, `analyze`, and `validate` all
    // honor a config-file setting without each re-implementing it.
    config.apply_git_settings_to_env();
    Ok(config)
}

fn resolve_analyze_config(args: &AnalyzeArgs) -> Result<AppConfig> {
    let mut config = load_base_config(args.config.as_deref())?;
    apply_discovery_cli_args(&mut config, args);
    apply_analysis_cli_args(&mut config, args);
    if let Some(title) = &args.report_title {
        config.reporting.report_title.clone_from(title);
    }
    if let Some(profile) = &args.profile {
        config.apply_profile(profile)?;
    }
    config.validate()?;
    if config.discovery.root_paths.is_empty() {
        anyhow::bail!("provide at least one PATH or configure discovery.root_paths");
    }
    Ok(config)
}

fn apply_discovery_cli_args(config: &mut AppConfig, args: &AnalyzeArgs) {
    if !args.paths.is_empty() {
        config.discovery.root_paths.clone_from(&args.paths);
    }
    if !args.include_glob.is_empty() {
        config
            .discovery
            .include_globs
            .clone_from(&args.include_glob);
    }
    if !args.exclude_glob.is_empty() {
        config
            .discovery
            .exclude_globs
            .clone_from(&args.exclude_glob);
    }
    if args.no_ignore_files {
        config.discovery.honor_ignore_files = false;
    }
    if args.follow_symlinks {
        config.discovery.follow_symlinks = true;
    }
    if args.submodule_breakdown {
        config.discovery.submodule_breakdown = true;
    }
}

fn apply_analysis_cli_args(config: &mut AppConfig, args: &AnalyzeArgs) {
    if !args.enabled_language.is_empty() {
        config
            .analysis
            .enabled_languages
            .clone_from(&args.enabled_language);
    }
    if let Some(policy) = args.mixed_line_policy {
        config.analysis.mixed_line_policy = policy;
    }
    if args.python_docstrings_as_code {
        config.analysis.python_docstrings_as_comments = false;
    }
    if let Some(policy) = args.continuation_line_policy {
        config.analysis.continuation_line_policy = policy;
    }
    if let Some(policy) = args.blank_in_block_comment_policy {
        config.analysis.blank_in_block_comment_policy = policy;
    }
    if args.no_count_compiler_directives {
        config.analysis.count_compiler_directives = false;
    }
    if let Some(cov) = &args.coverage_file {
        config.analysis.coverage_file = Some(cov.clone());
    }
    if let Some(threshold) = args.style_col_threshold {
        config.analysis.style_col_threshold = threshold;
    }
    if let Some(window) = args.activity_window {
        config.analysis.activity_window_days = Some(window);
    }
}

// ── terminal output ───────────────────────────────────────────────────────────

fn print_plain_summary(run: &AnalysisRun) {
    println!("files_analyzed={}", run.summary_totals.files_analyzed);
    println!("files_skipped={}", run.summary_totals.files_skipped);
    println!("physical_lines={}", run.summary_totals.total_physical_lines);
    println!("code_lines={}", run.summary_totals.code_lines);
    println!("comment_lines={}", run.summary_totals.comment_lines);
    println!("blank_lines={}", run.summary_totals.blank_lines);
    println!(
        "mixed_lines_separate={}",
        run.summary_totals.mixed_lines_separate
    );
    println!(
        "cyclomatic_complexity={}",
        run.summary_totals.cyclomatic_complexity
    );
    if let Some(lsloc) = run.summary_totals.lsloc {
        println!("lsloc={lsloc}");
    }
    println!("uloc={}", run.uloc);
    if let Some(dry) = run.dryness_pct {
        println!("dryness_pct={dry:.1}");
    }
    println!("duplicate_groups={}", run.duplicate_groups.len());
    let st = &run.summary_totals;
    println!("functions={}", st.functions);
    println!("classes={}", st.classes);
    println!("variables={}", st.variables);
    if st.variables_member + st.variables_local + st.variables_global > 0 {
        println!("variables_member={}", st.variables_member);
        println!("variables_local={}", st.variables_local);
        println!("variables_global={}", st.variables_global);
    }
    if st.macro_definitions > 0 {
        println!("macro_definitions={}", st.macro_definitions);
    }
    println!("imports={}", st.imports);
    println!("unit_tests={}", st.test_count);
    println!("test_assertions={}", st.test_assertion_count);
    println!("test_suites={}", st.test_suite_count);
    if st.coverage_lines_found > 0 {
        println!("coverage_lines_found={}", st.coverage_lines_found);
        println!("coverage_lines_hit={}", st.coverage_lines_hit);
        println!(
            "coverage_line_pct={:.1}",
            line_pct(st.coverage_lines_hit, st.coverage_lines_found)
        );
    }
    if st.coverage_functions_found > 0 {
        println!("coverage_functions_found={}", st.coverage_functions_found);
        println!("coverage_functions_hit={}", st.coverage_functions_hit);
    }
    if st.coverage_branches_found > 0 {
        println!("coverage_branches_found={}", st.coverage_branches_found);
        println!("coverage_branches_hit={}", st.coverage_branches_hit);
    }
    if let Some(ref c) = run.cocomo {
        println!("cocomo_ksloc={:.2}", c.ksloc);
        println!("cocomo_effort_person_months={:.2}", c.effort_person_months);
        println!("cocomo_duration_months={:.2}", c.duration_months);
        println!("cocomo_avg_staff={:.2}", c.avg_staff);
    }
    if let Some(ref ss) = run.style_summary {
        println!("style_files_analyzed={}", ss.files_analyzed);
        println!("style_common_indent={}", ss.common_indent_style);
        println!("style_col_threshold={}", ss.col_threshold);
        println!("style_col_compliant_pct={}", ss.line_col_compliant_pct);
        println!("style_language_groups={}", ss.by_language.len());
    }
    println!("warning_count={}", run.warnings.len());
    for warning in &run.warnings {
        println!("warning={warning}");
    }
}

fn print_totals_header(run: &AnalysisRun, col: bool) {
    println!("{}", paint!(col, "1", "SLOC Analysis Complete"));
    println!(
        "  {}  {}",
        paint!(col, "36", "Files analyzed :"),
        paint!(col, "32", run.summary_totals.files_analyzed)
    );
    println!(
        "  {}  {}",
        paint!(col, "36", "Files skipped  :"),
        run.summary_totals.files_skipped
    );
    println!(
        "  {}  {}",
        paint!(col, "36", "Physical lines :"),
        run.summary_totals.total_physical_lines
    );
    println!(
        "  {}  {}",
        paint!(col, "36", "Code lines     :"),
        paint!(col, "32;1", run.summary_totals.code_lines)
    );
    println!(
        "  {}  {}",
        paint!(col, "36", "Comment lines  :"),
        run.summary_totals.comment_lines
    );
    println!(
        "  {}  {}",
        paint!(col, "36", "Blank lines    :"),
        run.summary_totals.blank_lines
    );
    if run.summary_totals.mixed_lines_separate > 0 {
        println!(
            "  {}  {}",
            paint!(col, "36", "Mixed separate :"),
            run.summary_totals.mixed_lines_separate
        );
    }
    if run.summary_totals.cyclomatic_complexity > 0 {
        println!(
            "  {}  {}",
            paint!(col, "36", "Complexity     :"),
            run.summary_totals.cyclomatic_complexity
        );
    }
    if let Some(lsloc) = run.summary_totals.lsloc {
        println!("  {}  {}", paint!(col, "36", "Logical SLOC   :"), lsloc);
    }
    if run.uloc > 0 {
        let dry_str = run
            .dryness_pct
            .map_or(String::new(), |d| format!("  ({d:.1}% unique)"));
        println!(
            "  {}  {}{}",
            paint!(col, "36", "ULOC           :"),
            run.uloc,
            dry_str
        );
    }
    if !run.duplicate_groups.is_empty() {
        println!(
            "  {}  {} group(s)",
            paint!(col, "33", "Duplicates     :"),
            run.duplicate_groups.len()
        );
    }
    print_semantic_metrics(run, col);
    print_test_coverage(run, col);
    if let Some(ref c) = run.cocomo {
        println!();
        println!("{}", paint!(col, "1", "COCOMO I Estimate (Organic)"));
        println!(
            "  {}  {:.2} person-months",
            paint!(col, "36", "Effort         :"),
            c.effort_person_months
        );
        println!(
            "  {}  {:.2} months",
            paint!(col, "36", "Schedule       :"),
            c.duration_months
        );
        println!(
            "  {}  {:.2} avg. engineers",
            paint!(col, "36", "Team size      :"),
            c.avg_staff
        );
    }
}

/// Line-coverage percentage helper for terminal display.
#[allow(clippy::cast_precision_loss)]
fn line_pct(hit: u64, found: u64) -> f64 {
    if found == 0 {
        0.0
    } else {
        hit as f64 / found as f64 * 100.0
    }
}

/// Print structural symbol counts (functions / classes / variables / imports) when any are
/// detected. These are best-effort lexical counts, not full semantic analysis.
fn print_semantic_metrics(run: &AnalysisRun, col: bool) {
    let s = &run.summary_totals;
    if s.functions == 0 && s.classes == 0 && s.variables == 0 && s.imports == 0 {
        return;
    }
    println!();
    println!("{}", paint!(col, "1", "Semantic Metrics"));
    println!(
        "  {}  {}",
        paint!(col, "36", "Functions      :"),
        paint!(col, "32", s.functions)
    );
    if s.classes > 0 {
        println!("  {}  {}", paint!(col, "36", "Classes/Types  :"), s.classes);
    }
    if s.variables > 0 {
        // For C/C++ the total splits into member/local/global; annotate inline when tracked.
        let breakdown = if s.variables_member + s.variables_local + s.variables_global > 0 {
            // Scope is only tracked for C/C++; label it so the subset does not imply it sums to
            // the (possibly multi-language) total.
            format!(
                "  (C/C++: {} member, {} local, {} global)",
                s.variables_member, s.variables_local, s.variables_global
            )
        } else {
            String::new()
        };
        println!(
            "  {}  {}{}",
            paint!(col, "36", "Variables      :"),
            s.variables,
            paint!(col, "2", breakdown)
        );
    }
    if s.macro_definitions > 0 {
        println!(
            "  {}  {}",
            paint!(col, "36", "Macro consts   :"),
            s.macro_definitions
        );
    }
    if s.imports > 0 {
        println!("  {}  {}", paint!(col, "36", "Imports        :"), s.imports);
    }
}

/// Print unit-test and coverage metrics when any tests are detected or coverage was ingested.
fn print_test_coverage(run: &AnalysisRun, col: bool) {
    let s = &run.summary_totals;
    let has_tests = s.test_count > 0 || s.test_assertion_count > 0 || s.test_suite_count > 0;
    let has_cov = s.coverage_lines_found > 0;
    if !has_tests && !has_cov {
        return;
    }
    println!();
    println!("{}", paint!(col, "1", "Tests & Coverage"));
    if has_tests {
        println!(
            "  {}  {}",
            paint!(col, "36", "Unit tests     :"),
            paint!(col, "32;1", s.test_count)
        );
        if s.test_assertion_count > 0 {
            println!(
                "  {}  {}",
                paint!(col, "36", "Assertions     :"),
                s.test_assertion_count
            );
        }
        if s.test_suite_count > 0 {
            println!(
                "  {}  {}",
                paint!(col, "36", "Test suites    :"),
                s.test_suite_count
            );
        }
    }
    if has_cov {
        println!(
            "  {}  {:.1}%  ({} / {} lines)",
            paint!(col, "36", "Line coverage  :"),
            line_pct(s.coverage_lines_hit, s.coverage_lines_found),
            s.coverage_lines_hit,
            s.coverage_lines_found
        );
        if s.coverage_functions_found > 0 {
            println!(
                "  {}  {:.1}%  ({} / {} functions)",
                paint!(col, "36", "Func coverage  :"),
                line_pct(s.coverage_functions_hit, s.coverage_functions_found),
                s.coverage_functions_hit,
                s.coverage_functions_found
            );
        }
        if s.coverage_branches_found > 0 {
            println!(
                "  {}  {:.1}%  ({} / {} branches)",
                paint!(col, "36", "Branch coverage:"),
                line_pct(s.coverage_branches_hit, s.coverage_branches_found),
                s.coverage_branches_hit,
                s.coverage_branches_found
            );
        }
    }
}

fn print_language_table(run: &AnalysisRun, col: bool) {
    if run.totals_by_language.is_empty() {
        return;
    }
    println!();
    println!("{}", paint!(col, "1", "By Language"));
    println!(
        "  {:<14} {:>6} {:>8} {:>9} {:>7} {:>8}",
        paint!(col, "2", "Language"),
        paint!(col, "2", "Files"),
        paint!(col, "2", "Code"),
        paint!(col, "2", "Comments"),
        paint!(col, "2", "Blank"),
        paint!(col, "2", "Total"),
    );
    for lang in &run.totals_by_language {
        println!(
            "  {:<14} {:>6} {:>8} {:>9} {:>7} {:>8}",
            lang.language.display_name(),
            lang.files,
            lang.code_lines,
            lang.comment_lines,
            lang.blank_lines,
            lang.total_physical_lines,
        );
    }
}

fn print_per_file_table(run: &AnalysisRun, col: bool) {
    if run.per_file_records.is_empty() {
        return;
    }
    println!();
    println!("{}", paint!(col, "1", "Per-File Detail"));
    for file in &run.per_file_records {
        let sub_tag = file
            .submodule
            .as_deref()
            .map(|s| format!("[{s}] "))
            .unwrap_or_default();
        println!(
            "  {:<50} {:<14} code={:<6} comment={:<6} blank={:<6}",
            truncate(&format!("{sub_tag}{}", file.relative_path), 50),
            file.language
                .map_or_else(|| "-".into(), |l| l.display_name().to_string()),
            file.effective_counts.code_lines,
            file.effective_counts.comment_lines,
            file.effective_counts.blank_lines,
        );
    }
}

fn print_style_summary(run: &AnalysisRun, col: bool) {
    let Some(ref ss) = run.style_summary else {
        return;
    };
    println!();
    println!("{}", paint!(col, "1", "Code Style Analysis"));
    println!(
        "  {}  {}  |  {}  {}  |  {}  {}  |  {}  {}",
        paint!(col, "36", "Files:"),
        paint!(col, "32", ss.files_analyzed),
        paint!(col, "36", "Groups:"),
        ss.by_language.len(),
        paint!(col, "36", "Indent:"),
        ss.common_indent_style,
        paint!(col, "36", format!("{}-Col:", ss.col_threshold)),
        paint!(col, "32;1", format!("{}%", ss.line_col_compliant_pct)),
    );
    for grp in &ss.by_language {
        let guides: String = grp
            .guide_avg_scores
            .iter()
            .take(3)
            .map(|(name, score)| format!("{name} {score}%"))
            .collect::<Vec<_>>()
            .join("  |  ");
        println!(
            "  {} ({} files): {}",
            paint!(col, "33", &grp.language_family),
            grp.files_count,
            guides,
        );
    }
}

fn print_submodule_table(run: &AnalysisRun, col: bool) {
    if run.submodule_summaries.is_empty() {
        return;
    }
    println!();
    println!("{}", paint!(col, "1", "By Submodule"));
    for sub in &run.submodule_summaries {
        println!(
            "  {:<30} files={:<4} code={:<6} comment={:<6} blank={:<6}",
            truncate(&sub.name, 30),
            sub.files_analyzed,
            sub.code_lines,
            sub.comment_lines,
            sub.blank_lines,
        );
    }
}

fn print_summary(run: &AnalysisRun, per_file: bool, plain: bool) {
    if plain {
        print_plain_summary(run);
        return;
    }

    let col = color_enabled();
    print_totals_header(run, col);
    print_language_table(run, col);
    if per_file {
        print_per_file_table(run, col);
    }
    print_submodule_table(run, col);
    print_style_summary(run, col);

    if !run.warnings.is_empty() {
        println!();
        println!(
            "  {} {}",
            paint!(col, "33", "Warnings:"),
            run.warnings.len()
        );
        for warning in &run.warnings {
            println!("    {} {warning}", paint!(col, "33", "-"));
        }
    }
}

fn fmt_delta(col: bool, v: i64) -> String {
    match v.cmp(&0) {
        std::cmp::Ordering::Greater => paint!(col, "32", format!("+{v}")),
        std::cmp::Ordering::Less => paint!(col, "31", v.to_string()),
        std::cmp::Ordering::Equal => paint!(col, "2", "0"),
    }
}

fn print_diff_summary(cmp: &ScanComparison, plain: bool) {
    let s = &cmp.summary;

    if plain {
        println!("baseline_run_id={}", s.baseline_run_id);
        println!("current_run_id={}", s.current_run_id);
        println!("files_added={}", cmp.files_added);
        println!("files_removed={}", cmp.files_removed);
        println!("files_modified={}", cmp.files_modified);
        println!("files_unchanged={}", cmp.files_unchanged);
        println!("files_total={}", cmp.files_total);
        println!("code_lines_delta={}", s.code_lines_delta);
        println!("comment_lines_delta={}", s.comment_lines_delta);
        println!("blank_lines_delta={}", s.blank_lines_delta);
        println!("total_lines_delta={}", s.total_lines_delta);
        return;
    }

    let col = color_enabled();

    println!("{}", paint!(col, "1", "SLOC Diff"));
    println!("  Baseline : {}", s.baseline_run_id);
    println!("  Current  : {}", s.current_run_id);
    println!();
    println!(
        "  Files  added={} removed={} modified={} unchanged={} total={}",
        paint!(col, "32", cmp.files_added),
        paint!(col, "31", cmp.files_removed),
        paint!(col, "33", cmp.files_modified),
        paint!(col, "2", cmp.files_unchanged),
        paint!(col, "1", cmp.files_total),
    );
    println!("  Code Δ   : {}", fmt_delta(col, s.code_lines_delta));
    println!("  Comment Δ: {}", fmt_delta(col, s.comment_lines_delta));
    println!("  Blank Δ  : {}", fmt_delta(col, s.blank_lines_delta));
    println!("  Total Δ  : {}", fmt_delta(col, s.total_lines_delta));

    let changed: Vec<_> = cmp
        .file_deltas
        .iter()
        .filter(|f| f.status != sloc_core::FileChangeStatus::Unchanged)
        .take(20)
        .collect();

    if !changed.is_empty() {
        println!();
        println!("{}", paint!(col, "1", "Changed Files (top 20)"));
        for f in changed {
            let status_str = match f.status {
                sloc_core::FileChangeStatus::Added => paint!(col, "32", "A"),
                sloc_core::FileChangeStatus::Removed => paint!(col, "31", "D"),
                sloc_core::FileChangeStatus::Modified => paint!(col, "33", "M"),
                sloc_core::FileChangeStatus::Unchanged => paint!(col, "2", " "),
            };
            println!(
                "  {} {:<50} code {}",
                status_str,
                truncate(&f.relative_path, 50),
                fmt_delta(col, f.code_delta),
            );
        }
    }
}

// ── utilities ─────────────────────────────────────────────────────────────────

fn truncate(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if input.chars().count() <= width {
        return input.to_string();
    }
    let keep_chars = width.saturating_sub(1);
    let cut = input
        .char_indices()
        .nth(keep_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(input.len());
    format!("{}…", &input[..cut])
}

fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        // Use explorer.exe, which receives the path as a single, non-shell argument.
        // `cmd /c start` is shell-parsed and mishandles `&`, `%`, `^` in paths.
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

// Write diff as XLSX — thin wrapper delegating to sloc_report
fn write_diff_xlsx(cmp: &ScanComparison, path: &Path) -> Result<()> {
    sloc_report::write_diff_xlsx(cmp, path)
}

// ── Confluence delivery ───────────────────────────────────────────────────────

fn build_confluence_auth(username: Option<&str>, token: &str) -> String {
    atlassian_auth(username, token)
}

async fn confluence_upsert_cloud(
    client: &reqwest::Client,
    base_url: &str,
    auth: &str,
    space: &str,
    page_title: &str,
    body_html: &str,
    parent_id: Option<&str>,
) -> Result<()> {
    let space_resp: serde_json::Value = client
        .get(format!(
            "{base_url}/wiki/api/v2/spaces?keys={space}&limit=1"
        ))
        .header("Authorization", auth)
        .header("Accept", "application/json")
        .send()
        .await
        .context("Confluence space lookup")?
        .json()
        .await
        .context("Confluence space response")?;

    let space_id = space_resp["results"][0]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Confluence space '{space}' not found"))?
        .to_owned();

    let enc_title = percent_encode(page_title);
    let find_resp: serde_json::Value = client
        .get(format!(
            "{base_url}/wiki/api/v2/pages?spaceId={space_id}&title={enc_title}&limit=1&expand=version"
        ))
        .header("Authorization", auth)
        .header("Accept", "application/json")
        .send()
        .await?
        .json()
        .await?;

    let existing_id = find_resp["results"][0]["id"].as_str().map(str::to_owned);
    let existing_ver = find_resp["results"][0]["version"]["number"]
        .as_u64()
        // Confluence version numbers are tiny; truncation is not possible in practice.
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));

    if let (Some(page_id), Some(ver)) = (existing_id, existing_ver) {
        let payload = serde_json::json!({
            "version": { "number": ver + 1 },
            "title": page_title,
            "body": { "representation": "storage", "value": body_html }
        });
        let resp = client
            .put(format!("{base_url}/wiki/api/v2/pages/{page_id}"))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("Confluence update failed (HTTP {})", resp.status());
        }
        println!("send: updated Confluence page '{page_title}' (id: {page_id})");
    } else {
        let mut payload = serde_json::json!({
            "spaceId": space_id,
            "title": page_title,
            "body": { "representation": "storage", "value": body_html }
        });
        if let Some(pid) = parent_id {
            payload["parentId"] = serde_json::Value::String(pid.to_owned());
        }
        let resp = client
            .post(format!("{base_url}/wiki/api/v2/pages"))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Confluence create failed (HTTP {status}): {body}");
        }
        let created: serde_json::Value = resp.json().await?;
        let new_id = created["id"].as_str().unwrap_or("?");
        println!("send: created Confluence page '{page_title}' (id: {new_id})");
    }
    Ok(())
}

async fn confluence_upsert_server(
    client: &reqwest::Client,
    base_url: &str,
    auth: &str,
    space: &str,
    page_title: &str,
    body_html: &str,
    parent_id: Option<&str>,
) -> Result<()> {
    let enc_title = percent_encode(page_title);
    let find_resp: serde_json::Value = client
        .get(format!(
            "{base_url}/rest/api/content?spaceKey={space}&title={enc_title}&type=page&expand=version&limit=1"
        ))
        .header("Authorization", auth)
        .header("Accept", "application/json")
        .send()
        .await?
        .json()
        .await?;

    let existing_id = find_resp["results"][0]["id"].as_str().map(str::to_owned);
    let existing_ver = find_resp["results"][0]["version"]["number"]
        .as_u64()
        // Confluence version numbers are tiny; truncation is not possible in practice.
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));

    if let (Some(page_id), Some(ver)) = (existing_id, existing_ver) {
        let payload = serde_json::json!({
            "version": { "number": ver + 1 },
            "type": "page",
            "title": page_title,
            "space": { "key": space },
            "body": { "storage": { "value": body_html, "representation": "storage" } }
        });
        let resp = client
            .put(format!("{base_url}/rest/api/content/{page_id}"))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("Confluence update failed (HTTP {})", resp.status());
        }
        println!("send: updated Confluence page '{page_title}' (id: {page_id})");
    } else {
        let mut payload = serde_json::json!({
            "type": "page",
            "space": { "key": space },
            "title": page_title,
            "body": { "storage": { "value": body_html, "representation": "storage" } }
        });
        if let Some(pid) = parent_id {
            payload["ancestors"] = serde_json::json!([{ "id": pid }]);
        }
        let resp = client
            .post(format!("{base_url}/rest/api/content"))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Confluence create failed (HTTP {status}): {body}");
        }
        let created: serde_json::Value = resp.json().await?;
        let new_id = created["id"].as_str().unwrap_or("?");
        println!("send: created Confluence page '{page_title}' (id: {new_id})");
    }
    Ok(())
}

async fn send_confluence(args: &SendArgs, run: &AnalysisRun) -> Result<()> {
    let base_url = args
        .confluence_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--confluence-url is required"))?
        .trim_end_matches('/');
    let token = args.confluence_token.as_deref().ok_or_else(|| {
        anyhow::anyhow!("--confluence-token (or SLOC_CONFLUENCE_TOKEN) is required")
    })?;
    let space = args.confluence_space.as_deref().ok_or_else(|| {
        anyhow::anyhow!("--confluence-space (or SLOC_CONFLUENCE_SPACE) is required")
    })?;

    let page_title = args
        .confluence_page_title
        .as_deref()
        .unwrap_or(&run.effective_configuration.reporting.report_title);

    let report_url = args
        .confluence_report_url
        .as_deref()
        .or(args.report_url.as_deref());

    let body_html = sloc_report::render_confluence_storage(run, report_url);
    let auth = build_confluence_auth(args.confluence_username.as_deref(), token);
    let client = reqwest::Client::new();
    let parent_id = args.confluence_parent_id.as_deref();

    match detect_tier(base_url) {
        AtlassianTier::Cloud => {
            confluence_upsert_cloud(
                &client, base_url, &auth, space, page_title, &body_html, parent_id,
            )
            .await
        }
        AtlassianTier::ServerDc => {
            confluence_upsert_server(
                &client, base_url, &auth, space, page_title, &body_html, parent_id,
            )
            .await
        }
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                write!(out, "{b:02X}").expect("write to String is infallible");
            }
        }
    }
    out
}

// ── pr-comment handler ────────────────────────────────────────────────────────

async fn run_pr_comment(args: PrCommentArgs) -> Result<()> {
    let current = read_json(&args.current)
        .with_context(|| format!("failed to read current: {}", args.current.display()))?;

    let comparison = if let Some(baseline_path) = &args.baseline {
        let baseline = read_json(baseline_path)
            .with_context(|| format!("failed to read baseline: {}", baseline_path.display()))?;
        Some(compute_delta(&baseline, &current))
    } else {
        None
    };

    let body = build_pr_comment_body(&current, comparison.as_ref(), args.report_url.as_deref());

    match args.provider {
        VcsProvider::GitHub => {
            post_github_comment(&args, &body).await?;
        }
        VcsProvider::GitLab => {
            post_gitlab_comment(&args, &body).await?;
        }
        VcsProvider::Bitbucket => {
            post_bitbucket_comment(&args, &body).await?;
        }
    }

    println!("pr-comment: posted comment to PR #{}", args.pr_number);
    Ok(())
}

fn build_pr_comment_body(
    run: &AnalysisRun,
    comparison: Option<&ScanComparison>,
    report_url: Option<&str>,
) -> String {
    let totals = &run.summary_totals;
    let mut out = String::new();

    out.push_str("## SLOC Report\n\n");

    // Summary table
    out.push_str("| Metric | Value |\n");
    out.push_str("|--------|-------|\n");
    writeln!(out, "| Files analyzed | {} |", totals.files_analyzed).expect("infallible");
    writeln!(out, "| Code lines | {} |", fmt_thousands(totals.code_lines)).expect("infallible");
    writeln!(
        out,
        "| Comment lines | {} |",
        fmt_thousands(totals.comment_lines)
    )
    .expect("infallible");
    writeln!(
        out,
        "| Blank lines | {} |",
        fmt_thousands(totals.blank_lines)
    )
    .expect("infallible");

    // Delta section
    if let Some(cmp) = comparison {
        let s = &cmp.summary;
        let sign = |v: i64| {
            if v >= 0 {
                format!("+{v}")
            } else {
                v.to_string()
            }
        };
        out.push_str("\n### Changes vs. Target Branch\n\n");
        out.push_str("| | Delta |\n");
        out.push_str("|--|-------|\n");
        writeln!(out, "| Code Δ | {} |", sign(s.code_lines_delta)).expect("infallible");
        writeln!(out, "| Comment Δ | {} |", sign(s.comment_lines_delta)).expect("infallible");
        writeln!(out, "| Blank Δ | {} |", sign(s.blank_lines_delta)).expect("infallible");
        writeln!(
            out,
            "| Files | +{} added / -{} removed / ~{} modified / {} total |",
            cmp.files_added, cmp.files_removed, cmp.files_modified, cmp.files_total
        )
        .expect("infallible");
    }

    // Top languages
    if !run.totals_by_language.is_empty() {
        out.push_str("\n<details><summary>Language breakdown</summary>\n\n");
        out.push_str("| Language | Files | Code | Comments | Blank |\n");
        out.push_str("|----------|-------|------|----------|-------|\n");
        for l in run.totals_by_language.iter().take(10) {
            writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                l.language.display_name(),
                l.files,
                l.code_lines,
                l.comment_lines,
                l.blank_lines,
            )
            .expect("infallible");
        }
        out.push_str("\n</details>\n");
    }

    if let Some(url) = report_url {
        writeln!(out, "\n[View full report]({url})").expect("infallible");
    }

    out.push_str("\n*Generated by [oxide-sloc](https://github.com/oxide-sloc/oxide-sloc)*\n");
    out
}

async fn post_github_comment(args: &PrCommentArgs, body: &str) -> Result<()> {
    let base = args
        .api_url
        .as_deref()
        .unwrap_or("https://api.github.com")
        .trim_end_matches('/');
    let url = format!(
        "{base}/repos/{}/issues/{}/comments",
        args.repo, args.pr_number
    );

    validate_webhook_url(&url, args.allow_private_net)?;

    let payload = serde_json::json!({ "body": body });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", args.token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "oxide-sloc")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("GitHub API POST to {url} failed"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitHub API returned HTTP {status}: {text}");
    }
    Ok(())
}

async fn post_gitlab_comment(args: &PrCommentArgs, body: &str) -> Result<()> {
    let base = args
        .api_url
        .as_deref()
        .unwrap_or("https://gitlab.com")
        .trim_end_matches('/');
    // GitLab encodes the namespace/project as URL-encoded path for the API.
    let encoded_repo = percent_encode(&args.repo);
    let url = format!(
        "{base}/api/v4/projects/{encoded_repo}/merge_requests/{}/notes",
        args.pr_number
    );

    validate_webhook_url(&url, args.allow_private_net)?;

    let payload = serde_json::json!({ "body": body });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("PRIVATE-TOKEN", &args.token)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("GitLab API POST to {url} failed"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("GitLab API returned HTTP {status}: {text}");
    }
    Ok(())
}

/// Bitbucket Cloud lives at `bitbucket.org`; anything else (or an explicit
/// workspace being absent while the host is custom) is treated as Server/DC.
/// A `--workspace` value is only meaningful on Cloud, so its presence is a
/// strong Cloud signal.
fn bitbucket_is_cloud(base_url: &str, workspace: Option<&str>) -> bool {
    workspace.is_some() || base_url.to_lowercase().contains("bitbucket.org")
}

async fn post_bitbucket_comment(args: &PrCommentArgs, body: &str) -> Result<()> {
    let base = args
        .api_url
        .as_deref()
        .unwrap_or("https://api.bitbucket.org")
        .trim_end_matches('/');
    let auth = atlassian_auth(args.bitbucket_user.as_deref(), &args.token);

    let (url, payload) = if bitbucket_is_cloud(base, args.workspace.as_deref()) {
        let ws = args.workspace.as_deref().ok_or_else(|| {
            anyhow::anyhow!("--workspace (or SLOC_BB_WORKSPACE) is required for Bitbucket Cloud")
        })?;
        let url = format!(
            "{base}/2.0/repositories/{ws}/{}/pullrequests/{}/comments",
            args.repo, args.pr_number
        );
        (url, serde_json::json!({ "content": { "raw": body } }))
    } else {
        // Server/DC keys the repo as PROJECT/repo.
        let (proj, slug) = args.repo.split_once('/').ok_or_else(|| {
            anyhow::anyhow!("Bitbucket Server/DC --repo must be in PROJECT/repo form")
        })?;
        let url = format!(
            "{base}/rest/api/1.0/projects/{proj}/repos/{slug}/pull-requests/{}/comments",
            args.pr_number
        );
        (url, serde_json::json!({ "text": body }))
    };

    atlassian_ssrf_check(&url, args.allow_private_net)?;

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", &auth)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Bitbucket API POST to {url} failed"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Bitbucket API returned HTTP {status}: {text}");
    }
    Ok(())
}

// ── jira handler ──────────────────────────────────────────────────────────────

async fn run_jira(args: JiraArgs) -> Result<()> {
    let base = args
        .jira_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--jira-url (or SLOC_JIRA_URL) is required"))?
        .trim_end_matches('/');
    let token = args
        .jira_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--jira-token (or SLOC_JIRA_TOKEN) is required"))?;
    let issue = args
        .issue_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--issue-key (or SLOC_JIRA_ISSUE) is required"))?;

    let run = read_json(&args.current)
        .with_context(|| format!("failed to read current: {}", args.current.display()))?;
    // A baseline is accepted for symmetry with pr-comment; the delta is folded
    // into the rendered summary body by the shared renderers.
    if let Some(baseline_path) = &args.baseline {
        read_json(baseline_path)
            .with_context(|| format!("failed to read baseline: {}", baseline_path.display()))?;
    }

    let tier = detect_tier(base);
    // Jira Cloud is REST v3 (ADF bodies); Server/DC is REST v2 (wiki markup).
    let api = match tier {
        AtlassianTier::Cloud => "3",
        AtlassianTier::ServerDc => "2",
    };
    let auth = atlassian_auth(args.jira_username.as_deref(), token);
    let report_url = args.report_url.as_deref();

    // Only the custom-field write is a PUT; comment and remote-link are POSTs.
    let use_put = matches!(args.mode, JiraMode::Field);
    let (url, payload) = match args.mode {
        JiraMode::Comment => {
            let url = format!("{base}/rest/api/{api}/issue/{issue}/comment");
            let body = match tier {
                AtlassianTier::Cloud => {
                    serde_json::json!({ "body": atlassian::render_adf_summary(&run, report_url) })
                }
                AtlassianTier::ServerDc => {
                    serde_json::json!({ "body": atlassian::render_wiki_summary(&run, report_url) })
                }
            };
            (url, body)
        }
        JiraMode::RemoteLink => {
            let target = report_url.ok_or_else(|| {
                anyhow::anyhow!("--report-url is required for --mode remote-link")
            })?;
            let url = format!("{base}/rest/api/{api}/issue/{issue}/remotelink");
            let body = serde_json::json!({
                "object": {
                    "url": target,
                    "title": "oxide-sloc SLOC report",
                    "summary": atlassian::summary_oneline(&run),
                }
            });
            (url, body)
        }
        JiraMode::Field => {
            let field = args
                .field_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--field-id is required for --mode field"))?;
            let url = format!("{base}/rest/api/{api}/issue/{issue}");
            let body = serde_json::json!({
                "fields": { field: atlassian::summary_oneline(&run) }
            });
            (url, body)
        }
    };

    atlassian_ssrf_check(&url, args.allow_private_net)?;

    let method = if use_put { "PUT" } else { "POST" };
    if args.dry_run {
        println!("jira: DRY-RUN {method} {url}");
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return Ok(());
    }

    let client = reqwest::Client::new();
    let req = if use_put {
        client.put(&url)
    } else {
        client.post(&url)
    };
    let resp = req
        .header("Authorization", &auth)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Jira API {method} to {url} failed"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Jira API returned HTTP {status}: {text}");
    }

    let what = match args.mode {
        JiraMode::Comment => "comment",
        JiraMode::RemoteLink => "remote link",
        JiraMode::Field => "field update",
    };
    println!("jira: posted {what} to {issue}");
    Ok(())
}

// ── bitbucket-status handler ────────────────────────────────────────────────--

async fn run_bitbucket_status(args: BitbucketStatusArgs) -> Result<()> {
    let base = args
        .bitbucket_url
        .as_deref()
        .unwrap_or("https://api.bitbucket.org")
        .trim_end_matches('/');
    let token = args
        .token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--token (or SLOC_BB_TOKEN) is required"))?;
    let commit = args
        .commit
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--commit (or SLOC_BB_COMMIT) is required"))?;

    let run = read_json(&args.current)
        .with_context(|| format!("failed to read current: {}", args.current.display()))?;
    let auth = atlassian_auth(args.bitbucket_user.as_deref(), token);

    let url = if bitbucket_is_cloud(base, args.workspace.as_deref()) {
        let ws = args.workspace.as_deref().ok_or_else(|| {
            anyhow::anyhow!("--workspace (or SLOC_BB_WORKSPACE) is required for Bitbucket Cloud")
        })?;
        format!(
            "{base}/2.0/repositories/{ws}/{}/commit/{commit}/statuses/build",
            args.repo
        )
    } else {
        // Server/DC build-status is keyed solely by commit SHA.
        format!("{base}/rest/build-status/1.0/commits/{commit}")
    };

    // Bitbucket requires a non-empty target URL on a build status.
    let target_url = args
        .report_url
        .as_deref()
        .unwrap_or("https://github.com/oxide-sloc/oxide-sloc");
    let payload = serde_json::json!({
        "state": args.state.as_token(),
        "key": args.key,
        "name": "oxide-sloc SLOC",
        "url": target_url,
        "description": atlassian::summary_oneline(&run),
    });

    atlassian_ssrf_check(&url, args.allow_private_net)?;

    if args.dry_run {
        println!("bitbucket-status: DRY-RUN POST {url}");
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return Ok(());
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", &auth)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("Bitbucket API POST to {url} failed"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Bitbucket API returned HTTP {status}: {text}");
    }

    println!(
        "bitbucket-status: reported {} for commit {commit}",
        args.state.as_token()
    );
    Ok(())
}

// ── git-scan handler ──────────────────────────────────────────────────────────

/// Apply the CLI `--allow-local` / `--local-root` flags to the environment `sloc-git` reads.
/// An explicit env var always wins (values are only set when unset).
fn apply_local_gate(allow_local: bool, local_root: Option<&Path>) {
    if allow_local && std::env::var_os("SLOC_GIT_ALLOW_LOCAL").is_none() {
        // SAFETY: set at CLI command entry before any git op or worker task is spawned.
        unsafe { std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1") };
    }
    if let Some(root) = local_root
        && std::env::var_os("SLOC_GIT_LOCAL_ROOT").is_none()
    {
        // SAFETY: see above.
        unsafe { std::env::set_var("SLOC_GIT_LOCAL_ROOT", root) };
    }
}

async fn run_git_scan(args: GitScanArgs) -> Result<()> {
    apply_local_gate(args.allow_local, args.local_root.as_deref());
    let clones_dir = resolve_clones_dir(args.clones_dir.as_deref())?;
    let quiet = args.quiet;

    if !quiet {
        eprintln!("Cloning / fetching {}…", args.repo);
    }
    let dest = git_clone_path(&args.repo, &clones_dir);
    clone_or_fetch(&args.repo, &dest)?;

    let wt_path = clones_dir.join(format!("wt-cli-{}", uuid_simple()));
    create_worktree(&dest, &args.git_ref, &wt_path)?;

    let config = build_git_scan_config(&wt_path);
    if !quiet {
        eprintln!("Scanning {} at {}…", args.repo, args.git_ref);
    }
    let run_result =
        tokio::task::spawn_blocking(move || analyze(&config, "git-scan", None, None)).await;
    let _ = destroy_worktree(&dest, &wt_path);
    let run = run_result.context("analysis task failed")??;

    if !quiet {
        print_summary(&run, false, args.plain);
    }
    write_git_scan_outputs(
        &run,
        args.json_out.as_deref(),
        args.html_out.as_deref(),
        args.csv_out.as_deref(),
        quiet,
    )?;
    Ok(())
}

fn build_git_scan_config(path: &Path) -> AppConfig {
    let mut config = AppConfig::default();
    config.discovery.root_paths = vec![path.to_path_buf()];
    config
}

fn write_git_scan_outputs(
    run: &AnalysisRun,
    json_out: Option<&Path>,
    html_out: Option<&Path>,
    csv_out: Option<&Path>,
    quiet: bool,
) -> Result<()> {
    if let Some(p) = json_out {
        write_json(run, p)?;
        log_written(p, quiet);
    }
    if let Some(p) = html_out {
        write_html(run, p)?;
        log_written(p, quiet);
    }
    if let Some(p) = csv_out {
        write_csv(run, p)?;
        log_written(p, quiet);
    }
    Ok(())
}

// ── git-compare handler ───────────────────────────────────────────────────────

// Args are matched by the dispatch pattern; taking ownership is idiomatic for handler functions.
#[allow(clippy::needless_pass_by_value)]
fn run_git_compare(args: GitCompareArgs) -> Result<()> {
    apply_local_gate(args.allow_local, args.local_root.as_deref());
    let clones_dir = resolve_clones_dir(args.clones_dir.as_deref())?;
    let quiet = args.quiet;
    let dest = git_clone_path(&args.repo, &clones_dir);
    clone_or_fetch(&args.repo, &dest)?;

    let baseline_run = scan_at_ref(&dest, &args.baseline_ref, &clones_dir, quiet)?;
    let current_run = scan_at_ref(&dest, &args.current_ref, &clones_dir, quiet)?;

    let comparison = compute_delta(&baseline_run, &current_run);
    if !quiet {
        print_diff_summary(&comparison, args.plain);
    }
    write_compare_outputs(
        &comparison,
        args.json_out.as_deref(),
        args.csv_out.as_deref(),
        quiet,
    )?;
    Ok(())
}

fn scan_at_ref(dest: &Path, ref_name: &str, clones_dir: &Path, quiet: bool) -> Result<AnalysisRun> {
    let wt = clones_dir.join(format!("wt-cli-{}", uuid_simple()));
    create_worktree(dest, ref_name, &wt)?;
    if !quiet {
        eprintln!("Scanning ref {ref_name}…");
    }
    let config = build_git_scan_config(&wt);
    let run = analyze(&config, "git-compare", None, None)?;
    let _ = destroy_worktree(dest, &wt);
    Ok(run)
}

fn write_compare_outputs(
    cmp: &ScanComparison,
    json_out: Option<&Path>,
    csv_out: Option<&Path>,
    quiet: bool,
) -> Result<()> {
    if let Some(p) = json_out {
        let json = serde_json::to_string_pretty(cmp)?;
        std::fs::write(p, json)?;
        log_written(p, quiet);
    }
    if let Some(p) = csv_out {
        write_diff_csv(cmp, p)?;
        log_written(p, quiet);
    }
    Ok(())
}

// ── watch handler ─────────────────────────────────────────────────────────────

async fn run_watch(args: WatchArgs) -> Result<()> {
    apply_local_gate(args.allow_local, args.local_root.as_deref());
    let clones_dir = resolve_clones_dir(args.clones_dir.as_deref())?;
    let quiet = args.quiet;
    let interval = args.interval.max(60);

    if !quiet {
        eprintln!(
            "Watching {} ({}) — polling every {}s. Ctrl-C to stop.",
            args.repo, args.branch, interval
        );
    }
    let dest = git_clone_path(&args.repo, &clones_dir);
    clone_or_fetch(&args.repo, &dest)?;

    let mut last_sha = get_sha(&dest, &format!("origin/{}", args.branch)).unwrap_or_default();

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        clone_or_fetch(&args.repo, &dest)?;
        let sha = match get_sha(&dest, &format!("origin/{}", args.branch)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[watch] resolve failed: {e}");
                continue;
            }
        };

        if sha == last_sha {
            continue;
        }
        last_sha.clone_from(&sha);
        if !quiet {
            eprintln!("[watch] new commit {sha} — scanning…");
        }
        run_watch_scan(&dest, &sha, &clones_dir, args.output_dir.as_deref(), quiet);
    }
}

fn run_watch_scan(
    dest: &Path,
    sha: &str,
    clones_dir: &Path,
    output_dir: Option<&Path>,
    quiet: bool,
) {
    let wt = clones_dir.join(format!("wt-watch-{}", uuid_simple()));
    if let Err(e) = create_worktree(dest, sha, &wt) {
        eprintln!("[watch] worktree error: {e}");
        return;
    }
    let config = build_git_scan_config(&wt);
    match analyze(&config, "watch", None, None) {
        Ok(run) => {
            if !quiet {
                print_summary(&run, false, false);
            }
            write_watch_output(&run, output_dir, sha, quiet);
        }
        Err(e) => eprintln!("[watch] scan error: {e:#}"),
    }
    let _ = destroy_worktree(dest, &wt);
}

fn write_watch_output(run: &AnalysisRun, output_dir: Option<&Path>, sha: &str, quiet: bool) {
    let Some(dir) = output_dir else { return };
    let path = dir.join(format!("{}.json", &sha[..sha.len().min(8)]));
    match write_json(run, &path) {
        Err(e) => {
            eprintln!("[watch] write failed: {e}");
        }
        _ => {
            log_written(&path, quiet);
        }
    }
}

// ── prune ───────────────────────────────────────────────────────────────────────

/// Human-readable byte size (e.g. `1.4 GB`). Binary units to match disk tooling.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    #[allow(clippy::cast_precision_loss)]
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Collect the audit-log file and its rotated generations for the `--logs` sweep.
fn audit_log_targets() -> Vec<PathBuf> {
    let Some(raw) = std::env::var("SLOC_AUDIT_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        return Vec::new();
    };
    let base = PathBuf::from(raw);
    let mut targets = rotated_log_paths(&base);
    if base.exists() {
        targets.insert(0, base);
    }
    targets
}

/// `oxide-sloc prune` — operator-driven disk reclamation. No server, no login:
/// the local shell is the authority. Dry-run unless `--yes` is given.
fn run_prune(args: &PruneArgs) -> Result<()> {
    let c = color_enabled();
    let output_root = resolve_output_root(args.output_dir.as_ref().and_then(|p| p.to_str()));
    let registry_path = resolve_registry_path(&output_root);
    let mut registry = ScanRegistry::load(&registry_path);

    let plan = plan_run_prune(&registry, args.older_than, args.keep_last);
    let no_rules = args.older_than.is_none() && args.keep_last.is_none() && !args.logs;

    // Gather log targets (bytes) up front so the summary is accurate in dry-run too.
    let log_targets = if args.logs {
        audit_log_targets()
    } else {
        Vec::new()
    };
    let log_bytes: u64 = log_targets
        .iter()
        .map(|p| std::fs::metadata(p).map_or(0, |m| m.len()))
        .sum();

    if args.json {
        return emit_prune_json(
            args,
            &output_root,
            &plan,
            &log_targets,
            log_bytes,
            &mut registry,
        );
    }

    if no_rules {
        eprintln!(
            "Nothing to do. Specify at least one rule: {}, {}, or {}.",
            paint!(c, "1;36", "--older-than <DAYS>"),
            paint!(c, "1;36", "--keep-last <N>"),
            paint!(c, "1;36", "--logs")
        );
        eprintln!(
            "Add {} to perform the deletion.",
            paint!(c, "1;33", "--yes")
        );
        return Ok(());
    }

    println!(
        "{} {}",
        paint!(c, "1", "Prune target:"),
        output_root.display()
    );

    print_prune_plan(c, &plan);
    if args.logs {
        print_log_sweep(c, &log_targets, log_bytes);
    }

    let total = plan.total_bytes + log_bytes;

    // ── Dry-run vs execute ──────────────────────────────────────────────────────
    if !args.yes {
        println!(
            "\n{} — nothing was deleted. Re-run with {} to reclaim {}.",
            paint!(c, "1;33", "DRY RUN"),
            paint!(c, "1;33", "--yes"),
            paint!(c, "1;32", &human_bytes(total))
        );
        return Ok(());
    }

    let report = execute_run_prune(&mut registry, &plan);
    if !plan.is_empty() {
        registry
            .save(&registry_path)
            .context("failed to update registry after prune")?;
    }

    let mut freed = report.bytes_freed;
    if args.logs {
        freed += remove_log_targets(&log_targets);
    }

    for (path, err) in &report.failures {
        eprintln!("{} could not remove {path}: {err}", paint!(c, "1;31", "!"));
    }

    println!(
        "\n{} Removed {} run(s){}; reclaimed {}.",
        paint!(c, "1;32", "✓"),
        report.deleted_runs,
        if args.logs {
            format!(" and {} log file(s)", log_targets.len())
        } else {
            String::new()
        },
        paint!(c, "1;32", &human_bytes(freed))
    );
    Ok(())
}

/// Print the run-artifact prune plan (or a "nothing matched" line).
fn print_prune_plan(c: bool, plan: &sloc_core::PrunePlan) {
    if plan.is_empty() {
        println!("  No scan runs match the retention rules.");
        return;
    }
    println!(
        "  {} scan run(s), {} to reclaim:",
        paint!(c, "1", &plan.runs.len().to_string()),
        paint!(c, "1;32", &human_bytes(plan.total_bytes))
    );
    for r in &plan.runs {
        println!(
            "    {}  {}  {}  {}",
            paint!(c, "2", &r.timestamp_utc.format("%Y-%m-%d").to_string()),
            r.run_id,
            paint!(c, "2", &r.project_label),
            human_bytes(r.bytes)
        );
    }
}

/// Print the audit-log sweep section (or a "nothing to remove" line).
fn print_log_sweep(c: bool, log_targets: &[PathBuf], log_bytes: u64) {
    if log_targets.is_empty() {
        println!("  No audit log to remove ($SLOC_AUDIT_LOG unset or empty).");
        return;
    }
    println!(
        "  {} log file(s), {} to reclaim:",
        paint!(c, "1", &log_targets.len().to_string()),
        paint!(c, "1;32", &human_bytes(log_bytes))
    );
    for p in log_targets {
        println!("    {}", p.display());
    }
}

/// Delete the given log files, returning the total bytes removed.
fn remove_log_targets(targets: &[PathBuf]) -> u64 {
    let mut freed = 0;
    for p in targets {
        let sz = std::fs::metadata(p).map_or(0, |m| m.len());
        if std::fs::remove_file(p).is_ok() {
            freed += sz;
        }
    }
    freed
}

/// JSON output path for `prune --json`. Executes when `--yes`, otherwise reports
/// the plan only.
fn emit_prune_json(
    args: &PruneArgs,
    output_root: &Path,
    plan: &sloc_core::PrunePlan,
    log_targets: &[PathBuf],
    log_bytes: u64,
    registry: &mut ScanRegistry,
) -> Result<()> {
    let runs: Vec<_> = plan
        .runs
        .iter()
        .map(|r| {
            serde_json::json!({
                "run_id": r.run_id,
                "project": r.project_label,
                "timestamp_utc": r.timestamp_utc,
                "bytes": r.bytes,
            })
        })
        .collect();

    let mut freed = 0u64;
    let mut deleted = 0usize;
    if args.yes {
        let registry_path = resolve_registry_path(output_root);
        let report = execute_run_prune(registry, plan);
        if !plan.is_empty() {
            registry.save(&registry_path)?;
        }
        deleted = report.deleted_runs;
        freed = report.bytes_freed;
        if args.logs {
            freed += remove_log_targets(log_targets);
        }
    }

    let out = serde_json::json!({
        "output_root": output_root.display().to_string(),
        "dry_run": !args.yes,
        "runs_planned": runs,
        "run_bytes": plan.total_bytes,
        "log_files": log_targets.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "log_bytes": log_bytes,
        "deleted_runs": deleted,
        "bytes_freed": if args.yes { freed } else { plan.total_bytes + log_bytes },
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// ── git helpers ───────────────────────────────────────────────────────────────

fn resolve_clones_dir(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    if let Some(v) = std::env::var("SLOC_GIT_CLONES_DIR")
        .ok()
        .filter(|v| !v.is_empty())
    {
        return Ok(PathBuf::from(v));
    }
    // Default to a PER-USER cache, not a single shared /tmp path. The old default
    // (temp_dir()/sloc-git-clones) was one world-accessible directory shared by
    // every user on the host: a second user collided on it (EACCES on reuse of the
    // URL-keyed subdir), and — worse — any local user could pre-create that subdir
    // and seed content another user would then clone-into and scan. Scope the cache
    // to the current user and lock it to 0700.
    let dir = default_clones_dir();
    ensure_private_clones_dir(&dir)?;
    Ok(dir)
}

/// Per-user default clone-cache directory. Honors `XDG_CACHE_HOME` on Unix,
/// otherwise a uid- (or username-) suffixed path under the system temp dir. On
/// Windows the system temp dir is already per-user, so no suffix is needed.
fn default_clones_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Some(xdg) = std::env::var("XDG_CACHE_HOME")
            .ok()
            .filter(|v| !v.is_empty())
        {
            return PathBuf::from(xdg).join("oxide-sloc").join("git-clones");
        }
        let who = current_uid()
            .map(|u| u.to_string())
            .or_else(|| std::env::var("USER").ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "user".to_owned());
        std::env::temp_dir().join(format!("sloc-git-clones-{who}"))
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join("sloc-git-clones")
    }
}

/// Current effective uid. std has no `getuid()`, so read it back off a file we
/// just created (owned by our euid) — dependency-free and correct under rootless
/// uid remaps. `None` if the probe could not be created.
#[cfg(unix)]
fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    let probe = std::env::temp_dir().join(format!("sloc-uid-probe-{}", std::process::id()));
    std::fs::File::create(&probe).ok()?;
    let uid = std::fs::metadata(&probe).ok().map(|m| m.uid());
    let _ = std::fs::remove_file(&probe);
    uid
}

/// Create the default clone cache privately (0700) or, if it already exists,
/// verify the current user owns it — refusing to reuse a directory another user
/// (potentially an attacker) pre-created and seeded.
#[cfg(unix)]
fn ensure_private_clones_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    if let Ok(meta) = std::fs::metadata(dir) {
        if let Some(uid) = current_uid()
            && meta.uid() != uid
        {
            anyhow::bail!(
                "clone cache {} is owned by another user (uid {}, expected {}); refusing to \
                 reuse it. Set SLOC_GIT_CLONES_DIR to a private path.",
                dir.display(),
                meta.uid(),
                uid
            );
        }
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_clones_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

fn git_clone_path(repo_url: &str, clones_dir: &Path) -> PathBuf {
    let safe: String = repo_url
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    clones_dir.join(safe)
}

fn uuid_simple() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate;

    #[test]
    fn truncate_handles_zero_width() {
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn truncate_preserves_utf8_boundaries() {
        assert_eq!(truncate("héllo", 4), "hél…");
    }
}
