use crate::auth::{read_codex_auth_status, AuthCommandOptions};
use crate::error::AppError;
use crate::format::to_pretty_json;
use crate::limits::{read_rate_limit_samples_report, RateLimitSamplesReadOptions};
use crate::pricing::{
    calculate_credit_cost, list_known_unpriced_models, list_model_pricing, normalize_model_name,
    TokenUsage as PricingTokenUsage, CODEX_RATE_CARD_SOURCE,
};
use crate::stats::{read_usage_records_report, UsageRecordsReadOptions};
use crate::storage::{path_to_string, resolve_storage_paths, StorageOptions};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIN_NODE_VERSION: NodeVersion = NodeVersion {
    major: 20,
    minor: 12,
    patch: 0,
};
const MIN_NODE_VERSION_LABEL: &str = ">=20.12.0";

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DoctorOptions {
    pub auth_file: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub sessions_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq)]
pub struct DoctorCheck {
    pub name: String,
    pub status: String,
    pub message: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DoctorReport {
    pub now: DateTime<Utc>,
    pub codex_home: String,
    pub auth_file: String,
    pub sessions_dir: String,
    pub helper_dir: String,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Default)]
struct RecentUsageSummary {
    read_files: usize,
    token_count_events: usize,
    included_usage_events: usize,
    unpriced_models: BTreeMap<String, RecentUnpricedModel>,
}

#[derive(Debug, Clone, Default)]
struct RecentRateLimitsSummary {
    read_files: usize,
    sample_count: usize,
    five_hour_samples: usize,
    seven_day_samples: usize,
    latest_observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct NodeVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

#[derive(Debug, Clone, Default)]
struct RecentUnpricedModel {
    model: String,
    calls: usize,
    total_tokens: i64,
    note: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorJson<'a> {
    now: String,
    codex_home: &'a str,
    auth_file: &'a str,
    sessions_dir: &'a str,
    helper_dir: &'a str,
    checks: &'a [DoctorCheck],
    summary: DoctorSummary,
}

#[derive(Serialize)]
struct DoctorSummary {
    errors: usize,
    warnings: usize,
}

pub fn read_doctor_report(options: &DoctorOptions, now: DateTime<Utc>) -> DoctorReport {
    let storage = resolve_storage_paths(&StorageOptions {
        codex_home: options.codex_home.clone(),
        auth_file: options.auth_file.clone(),
        sessions_dir: options.sessions_dir.clone(),
        profile_store_dir: None,
        account_history_file: None,
        usage_mode_history_file: None,
    });

    let checks = vec![
        check_node_version(),
        check_directory("Codex home", &storage.codex_home, false),
        check_auth_file(&storage.auth_file, options, now),
        check_directory("Sessions directory", &storage.sessions_dir, false),
        check_helper_directory(&storage.helper_dir),
        check_recent_usage(&storage.sessions_dir, now),
        check_recent_rate_limits(&storage.sessions_dir, now),
        check_pricing(),
    ];

    DoctorReport {
        now,
        codex_home: path_to_string(&storage.codex_home),
        auth_file: path_to_string(&storage.auth_file),
        sessions_dir: path_to_string(&storage.sessions_dir),
        helper_dir: path_to_string(&storage.helper_dir),
        checks,
    }
}

pub fn format_doctor_report(report: &DoctorReport, json: bool) -> Result<String, AppError> {
    if json {
        let value = DoctorJson {
            now: format_iso(report.now),
            codex_home: &report.codex_home,
            auth_file: &report.auth_file,
            sessions_dir: &report.sessions_dir,
            helper_dir: &report.helper_dir,
            checks: &report.checks,
            summary: DoctorSummary {
                errors: report
                    .checks
                    .iter()
                    .filter(|check| check.status == "error")
                    .count(),
                warnings: report
                    .checks
                    .iter()
                    .filter(|check| check.status == "warn")
                    .count(),
            },
        };

        return Ok(format!(
            "{}\n",
            to_pretty_json(&value).map_err(|error| AppError::new(error.to_string()))?
        ));
    }

    let mut lines = vec![
        "Codex Ops doctor".to_string(),
        format!("Codex home: {}", report.codex_home),
        format!("Auth file: {}", report.auth_file),
        format!("Sessions dir: {}", report.sessions_dir),
        format!("Helper dir: {}", report.helper_dir),
        String::new(),
    ];

    for check in &report.checks {
        lines.push(format!(
            "[{}] {}: {}",
            check.status, check.name, check.message
        ));
        for detail in &check.details {
            lines.push(format!("  {detail}"));
        }
    }

    let errors = report
        .checks
        .iter()
        .filter(|check| check.status == "error")
        .count();
    let warnings = report
        .checks
        .iter()
        .filter(|check| check.status == "warn")
        .count();
    lines.push(String::new());
    lines.push(format!("Result: {errors} error(s), {warnings} warning(s)"));
    Ok(format!("{}\n", lines.join("\n")))
}

fn check_node_version() -> DoctorCheck {
    match Command::new("node").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();

            if parse_node_version(&version).is_some_and(|parsed| parsed >= MIN_NODE_VERSION) {
                ok(
                    "Node.js",
                    format!("{version} satisfies {MIN_NODE_VERSION_LABEL}"),
                    Vec::new(),
                )
            } else {
                check_error(
                    "Node.js",
                    format!("{version} is below the required {MIN_NODE_VERSION_LABEL}"),
                    Vec::new(),
                )
            }
        }
        Ok(output) => check_error(
            "Node.js",
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            Vec::new(),
        ),
        Err(error) => check_error("Node.js", error.to_string(), Vec::new()),
    }
}

fn parse_node_version(version: &str) -> Option<NodeVersion> {
    let mut parts = version.strip_prefix('v').unwrap_or(version).split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch_part = parts.next()?;
    let patch_digits = patch_part
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    let patch = patch_digits.parse::<u32>().ok()?;

    Some(NodeVersion {
        major,
        minor,
        patch,
    })
}

fn check_auth_file(auth_file: &Path, options: &DoctorOptions, now: DateTime<Utc>) -> DoctorCheck {
    match read_codex_auth_status(
        &AuthCommandOptions {
            auth_file: Some(auth_file.to_path_buf()),
            codex_home: options.codex_home.clone(),
            store_dir: None,
            account_history_file: None,
        },
        now,
    ) {
        Ok(report) => {
            let summary = report.summary;
            let label = summary
                .email
                .as_deref()
                .or(summary.name.as_deref())
                .or(summary.user_id.as_deref())
                .unwrap_or("authenticated");
            let mut details = vec![
                format!(
                    "Account: {}",
                    summary
                        .chatgpt_account_id
                        .as_deref()
                        .or(summary.token_account_id.as_deref())
                        .unwrap_or("unknown")
                ),
                format!(
                    "Plan: {}",
                    summary.plan_type.as_deref().unwrap_or("unknown")
                ),
            ];
            if let Some(expires_at) = summary.expires_at {
                details.push(format!("Token expires: {expires_at}"));
            }

            if summary.is_expired == Some(true) {
                warn(
                    "Auth file",
                    format!(
                        "Decoded {}, but the ID token is expired",
                        path_to_string(auth_file)
                    ),
                    details,
                )
            } else {
                ok(
                    "Auth file",
                    format!("Decoded {} for {label}", path_to_string(auth_file)),
                    details,
                )
            }
        }
        Err(error) if error.message().starts_with("ENOENT:") => warn(
            "Auth file",
            format!("Missing auth.json at {}", path_to_string(auth_file)),
            Vec::new(),
        ),
        Err(error) => check_error("Auth file", error.message().to_string(), Vec::new()),
    }
}

fn check_directory(name: &str, path: &Path, writable: bool) -> DoctorCheck {
    match fs::metadata(path) {
        Ok(info) if !info.is_dir() => check_error(
            name,
            format!("{} exists but is not a directory", path_to_string(path)),
            Vec::new(),
        ),
        Ok(_) => {
            if let Err(error) = fs::read_dir(path) {
                return check_error(name, error.to_string(), Vec::new());
            }
            if writable && is_readonly(path) {
                return check_error(name, "permission denied".to_string(), Vec::new());
            }
            ok(
                name,
                format!("{} is accessible", path_to_string(path)),
                Vec::new(),
            )
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => warn(
            name,
            format!("{} does not exist", path_to_string(path)),
            Vec::new(),
        ),
        Err(error) => check_error(name, error.to_string(), Vec::new()),
    }
}

fn check_helper_directory(helper_dir: &Path) -> DoctorCheck {
    match fs::metadata(helper_dir) {
        Ok(info) if !info.is_dir() => check_error(
            "Helper directory",
            format!(
                "{} exists but is not a directory",
                path_to_string(helper_dir)
            ),
            Vec::new(),
        ),
        Ok(_) => {
            if let Err(error) = fs::read_dir(helper_dir) {
                return check_error("Helper directory", error.to_string(), Vec::new());
            }
            if is_readonly(helper_dir) {
                return check_error(
                    "Helper directory",
                    "permission denied".to_string(),
                    Vec::new(),
                );
            }
            ok(
                "Helper directory",
                format!("{} is readable and writable", path_to_string(helper_dir)),
                Vec::new(),
            )
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => ok(
            "Helper directory",
            format!(
                "{} does not exist yet; helper commands will create it",
                path_to_string(helper_dir)
            ),
            Vec::new(),
        ),
        Err(error) => check_error("Helper directory", error.to_string(), Vec::new()),
    }
}

fn check_recent_usage(sessions_dir: &Path, now: DateTime<Utc>) -> DoctorCheck {
    if !sessions_dir.exists() {
        return warn(
            "Recent usage",
            format!(
                "Cannot scan usage because {} does not exist",
                path_to_string(sessions_dir)
            ),
            Vec::new(),
        );
    }

    match read_recent_usage_summary(sessions_dir, now) {
        Ok(summary) => {
            let details = vec![
                format!("Files read: {}", summary.read_files),
                format!("Token events: {}", summary.token_count_events),
                format!("Included usage events: {}", summary.included_usage_events),
            ];

            if !summary.unpriced_models.is_empty() {
                let mut details = details;
                for model in summary.unpriced_models.values() {
                    details.push(format!(
                        "{}: {} call(s), {} token(s){}",
                        model.model,
                        model.calls,
                        model.total_tokens,
                        model
                            .note
                            .as_ref()
                            .map(|note| format!(" ({note})"))
                            .unwrap_or_default()
                    ));
                }
                return warn(
                    "Recent usage",
                    format!(
                        "{} usage event(s), with unpriced model usage found",
                        summary.included_usage_events
                    ),
                    details,
                );
            }

            if summary.included_usage_events == 0 {
                return warn(
                    "Recent usage",
                    "No token_count usage events found in the last 7 days",
                    details,
                );
            }

            ok(
                "Recent usage",
                format!(
                    "{} usage event(s) found in the last 7 days",
                    summary.included_usage_events
                ),
                details,
            )
        }
        Err(error) => check_error("Recent usage", error, Vec::new()),
    }
}

fn check_recent_rate_limits(sessions_dir: &Path, now: DateTime<Utc>) -> DoctorCheck {
    if !sessions_dir.exists() {
        return warn(
            "Recent rate limits",
            format!(
                "Cannot scan rate limits because {} does not exist",
                path_to_string(sessions_dir)
            ),
            Vec::new(),
        );
    }

    match read_recent_rate_limits_summary(sessions_dir, now) {
        Ok(summary) => {
            let details = vec![
                format!("Files read: {}", summary.read_files),
                format!("Samples: {}", summary.sample_count),
                format!("5h samples: {}", summary.five_hour_samples),
                format!("7d samples: {}", summary.seven_day_samples),
                format!(
                    "Latest observed at: {}",
                    summary
                        .latest_observed_at
                        .map(format_iso)
                        .unwrap_or_else(|| "none".to_string())
                ),
            ];

            if summary.sample_count == 0 {
                return warn(
                    "Recent rate limits",
                    "No observed rate limits found in the last 7 days",
                    details,
                );
            }

            ok(
                "Recent rate limits",
                format!(
                    "{} rate-limit sample(s) found in the last 7 days",
                    summary.sample_count
                ),
                details,
            )
        }
        Err(error) => check_error("Recent rate limits", error, Vec::new()),
    }
}

fn check_pricing() -> DoctorCheck {
    let priced = list_model_pricing();
    let unpriced_count = list_known_unpriced_models().len();
    let mut details = vec![
        format!("Source: {}", CODEX_RATE_CARD_SOURCE.name),
        format!("Checked: {}", CODEX_RATE_CARD_SOURCE.checked_at),
        format!("Credits: {}", CODEX_RATE_CARD_SOURCE.credit_to_usd),
    ];

    for model in priced.iter().filter(|model| model.note.is_some()) {
        details.push(format!(
            "{}: {}",
            model.label,
            model.note.as_deref().unwrap_or_default()
        ));
    }

    ok(
        "Pricing",
        format!(
            "{} priced model(s), {} known unpriced model(s)",
            priced.len(),
            unpriced_count
        ),
        details,
    )
}

fn read_recent_usage_summary(
    sessions_dir: &Path,
    now: DateTime<Utc>,
) -> Result<RecentUsageSummary, String> {
    let start = now - Duration::days(7);
    let report = read_usage_records_report(&UsageRecordsReadOptions {
        start,
        end: now,
        sessions_dir: sessions_dir.to_path_buf(),
        scan_all_files: false,
        account_history_file: None,
        usage_mode_history_file: None,
        account_id: None,
    })
    .map_err(|error| error.message().to_string())?;

    let mut summary = RecentUsageSummary {
        read_files: report.diagnostics.read_files.max(0) as usize,
        token_count_events: report.diagnostics.token_count_events.max(0) as usize,
        included_usage_events: report.diagnostics.included_usage_events.max(0) as usize,
        ..RecentUsageSummary::default()
    };

    for record in report.records {
        let cost = calculate_credit_cost(
            &record.model,
            PricingTokenUsage {
                input_tokens: record.usage.input_tokens.max(0) as u64,
                cached_input_tokens: record.usage.cached_input_tokens.max(0) as u64,
                output_tokens: record.usage.output_tokens.max(0) as u64,
            },
        );
        if !cost.priced {
            let key = normalize_model_name(&record.model);
            let entry = summary
                .unpriced_models
                .entry(key)
                .or_insert_with(|| RecentUnpricedModel {
                    model: record.model.clone(),
                    calls: 0,
                    total_tokens: 0,
                    note: cost.unpriced_reason.clone(),
                });
            entry.calls += 1;
            entry.total_tokens += record.usage.total_tokens;
        }
    }

    Ok(summary)
}

fn read_recent_rate_limits_summary(
    sessions_dir: &Path,
    now: DateTime<Utc>,
) -> Result<RecentRateLimitsSummary, String> {
    let start = now - Duration::days(7);
    let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
        start,
        end: now,
        sessions_dir: sessions_dir.to_path_buf(),
        scan_all_files: false,
        account_history_file: None,
        account_id: None,
        plan_type: None,
        window_minutes: None,
    })
    .map_err(|error| error.message().to_string())?;

    Ok(RecentRateLimitsSummary {
        read_files: report.diagnostics.read_files.max(0) as usize,
        sample_count: report.samples.len(),
        five_hour_samples: report
            .samples
            .iter()
            .filter(|sample| sample.window_minutes == 300)
            .count(),
        seven_day_samples: report
            .samples
            .iter()
            .filter(|sample| sample.window_minutes == 10_080)
            .count(),
        latest_observed_at: report.samples.iter().map(|sample| sample.timestamp).max(),
    })
}

fn ok(name: &str, message: impl Into<String>, details: Vec<String>) -> DoctorCheck {
    DoctorCheck {
        name: name.to_string(),
        status: "ok".to_string(),
        message: message.into(),
        details,
    }
}

fn warn(name: &str, message: impl Into<String>, details: Vec<String>) -> DoctorCheck {
    DoctorCheck {
        name: name.to_string(),
        status: "warn".to_string(),
        message: message.into(),
        details,
    }
}

fn check_error(name: &str, message: impl Into<String>, details: Vec<String>) -> DoctorCheck {
    DoctorCheck {
        name: name.to_string(),
        status: "error".to_string(),
        message: message.into(),
        details,
    }
}

fn is_readonly(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(false)
}

fn format_iso(date: DateTime<Utc>) -> String {
    date.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_check_matches_typescript_summary() {
        let check = check_pricing();

        assert_eq!(check.name, "Pricing");
        assert_eq!(check.status, "ok");
        assert_eq!(
            check.message,
            "11 priced model(s), 0 known unpriced model(s)"
        );
        assert!(check
            .details
            .iter()
            .any(|detail| detail.contains("GPT-5.3-Codex-Spark")));
    }

    #[test]
    fn parses_node_version_with_major_minor_patch() {
        assert_eq!(
            parse_node_version("v20.12.0"),
            Some(NodeVersion {
                major: 20,
                minor: 12,
                patch: 0
            })
        );
        assert_eq!(
            parse_node_version("20.12.1"),
            Some(NodeVersion {
                major: 20,
                minor: 12,
                patch: 1
            })
        );
        assert_eq!(
            parse_node_version("v24.15.0-pre"),
            Some(NodeVersion {
                major: 24,
                minor: 15,
                patch: 0
            })
        );
        assert_eq!(parse_node_version("v20"), None);
        assert_eq!(parse_node_version("not-node"), None);
    }

    #[test]
    fn node_version_minimum_uses_minor_and_patch() {
        assert!(parse_node_version("v20.12.0").is_some_and(|version| version >= MIN_NODE_VERSION));
        assert!(parse_node_version("v20.12.1").is_some_and(|version| version >= MIN_NODE_VERSION));
        assert!(parse_node_version("v21.0.0").is_some_and(|version| version >= MIN_NODE_VERSION));
        assert!(parse_node_version("v20.11.9").is_some_and(|version| version < MIN_NODE_VERSION));
        assert!(parse_node_version("v19.99.99").is_some_and(|version| version < MIN_NODE_VERSION));
    }
}
