// Embeds build provenance (short git SHA + build timestamp) into the binary via
// `cargo:rustc-env`, so the web server can report it from /api/version and
// /api/health. Fully offline-safe: when git is unavailable (e.g. an air-gapped
// build from a vendored source tree with no .git), the SHA falls back to
// "unknown". The build time honours SOURCE_DATE_EPOCH when set, so reproducible
// build pipelines stay deterministic.
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let git_sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=OXIDE_SLOC_GIT_SHA={git_sha}");

    // Capture the `origin` remote URL so a built binary — including an air-gapped fork built
    // from an internal `git clone` — knows which repository it came from. The web UI
    // "Report a Bug" page uses it to direct reports back to the correct upstream. Best-effort:
    // absent git / no remote simply omits the var and the runtime falls back to config/default.
    if let Some(url) = git_origin_url() {
        println!("cargo:rustc-env=SLOC_BUILD_ORIGIN_URL={url}");
    }

    let epoch = build_epoch_seconds();
    println!(
        "cargo:rustc-env=OXIDE_SLOC_BUILD_TIME={}",
        format_utc(epoch)
    );

    // Re-run when HEAD moves so the embedded SHA stays fresh; ignore if absent.
    for hint in ["../../.git/HEAD", "../../.git/index", "../../.git/config"] {
        if std::path::Path::new(hint).exists() {
            println!("cargo:rerun-if-changed={hint}");
        }
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}

fn git_origin_url() -> Option<String> {
    let out = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8(out.stdout)
        .ok()?
        .replace(['\n', '\r'], "")
        .trim()
        .to_string();
    if url.is_empty() { None } else { Some(url) }
}

fn git_short_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

fn build_epoch_seconds() -> u64 {
    if let Ok(sde) = std::env::var("SOURCE_DATE_EPOCH")
        && let Ok(secs) = sde.trim().parse::<u64>()
    {
        return secs;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Minimal civil-time conversion (UTC, RFC 3339) so we don't pull a date crate
// into build-dependencies. Valid for all dates the project will ever build on.
fn format_utc(epoch: u64) -> String {
    let days = epoch / 86_400;
    let secs_of_day = epoch % 86_400;
    let (hh, mm, ss) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

// Howard Hinnant's days-from-civil inverse (public-domain algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
