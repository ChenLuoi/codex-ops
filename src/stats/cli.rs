use super::accumulators::{
    LimitUsageAccumulator, LimitUsageAccumulatorConfig, UsageSessionDetailAccumulator,
    UsageSessionsAccumulator, UsageStatsAccumulator,
};
use super::fast_candidates::{build_fast_candidate_report, FastCandidateReport};
use super::formatters::{
    format_fast_candidates, format_limit_usage, format_usage_session_detail, format_usage_sessions,
    format_usage_stats,
};
use super::reports::{
    LimitUsageGroupBy, LimitUsageReport, UsageRecordsReadOptions, UsageRecordsReport,
    UsageSessionDetailReport, UsageSessionsReport, UsageStatsReport,
};
use super::scan::{process_usage_records, process_usage_records_parallel};
use super::{StatFormat, StatSort};
use crate::account_history::{self, UsageAccountHistory};
use crate::auth::{ensure_usage_account_history, AuthCommandOptions};
use crate::error::AppError;
use crate::limits::{
    build_limit_windows_report, read_rate_limit_samples_report, LimitReportOptions,
    LimitWindowSelector, RateLimitSamplesReadOptions,
};
use crate::pricing::{calculate_credit_cost_with_context_at, PricingContext};
use crate::storage::{
    normalize_optional_string, path_to_string, resolve_storage_paths, StorageOptions,
};
use crate::time::{self, RawRangeOptions, StatGroupBy};
use crate::usage_mode_history::{self, UsageModeHistory};
use chrono::{DateTime, Duration, Utc};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct StatCommandOptions {
    pub start: Option<String>,
    pub end: Option<String>,
    pub group_by: Option<String>,
    pub limit_window: Option<String>,
    pub format: Option<String>,
    pub codex_home: Option<PathBuf>,
    pub sessions_dir: Option<PathBuf>,
    pub auth_file: Option<PathBuf>,
    pub account_history_file: Option<PathBuf>,
    pub usage_mode_history_file: Option<PathBuf>,
    pub today: bool,
    pub yesterday: bool,
    pub month: bool,
    pub all: bool,
    pub reasoning_effort: bool,
    pub account_id: Option<String>,
    pub last: Option<String>,
    pub sort: Option<String>,
    pub limit: Option<String>,
    pub top: Option<String>,
    pub detail: bool,
    pub full_scan: bool,
    pub verbose: bool,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedStatOptions {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) group_by: StatGroupBy,
    pub(super) limit_window: Option<LimitWindowSelector>,
    pub(super) limit_group_by: Option<StatGroupBy>,
    pub(super) format: StatFormat,
    pub(super) sessions_dir: PathBuf,
    pub(super) account_history_file: Option<PathBuf>,
    pub(super) usage_mode_history_file: Option<PathBuf>,
    pub(super) sort_by: Option<StatSort>,
    pub(super) limit: Option<usize>,
    pub(super) include_reasoning_effort: bool,
    pub(super) scan_all_files: bool,
    pub(super) verbose: bool,
    pub(super) account_id: Option<String>,
    pub(super) account_history: Option<UsageAccountHistory>,
    pub(super) usage_mode_history: Option<UsageModeHistory>,
    pub(super) usage_mode_history_present: bool,
    pub(super) usage_mode_history_switch_count: i64,
    pub(super) usage_mode_history_include_path: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedStatRangeOptions {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub format: StatFormat,
    pub sessions_dir: PathBuf,
    pub verbose: bool,
}

#[derive(Debug, Clone)]
struct ResolvedFastCandidateOptions {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    format: StatFormat,
    sessions_dir: PathBuf,
    account_history_file: Option<PathBuf>,
    scan_all_files: bool,
    verbose: bool,
    account_id: Option<String>,
}

pub fn resolve_stat_range_options_from_raw(
    raw: &StatCommandOptions,
    now: DateTime<Utc>,
) -> Result<ResolvedStatRangeOptions, AppError> {
    let format = if raw.json {
        StatFormat::Json
    } else {
        match raw.format.as_deref() {
            Some(value) => StatFormat::parse(value)?,
            None => StatFormat::Table,
        }
    };
    let range_options = raw_range_options(raw);
    let range = time::resolve_date_range(&range_options, now)?;
    if range.start > range.end {
        return Err(AppError::new(
            "The stat start time must be earlier than or equal to the end time.",
        ));
    }
    let paths = resolve_storage_paths(&StorageOptions {
        codex_home: raw.codex_home.clone(),
        auth_file: raw.auth_file.clone(),
        profile_store_dir: None,
        account_history_file: raw.account_history_file.clone(),
        usage_mode_history_file: raw.usage_mode_history_file.clone(),
        sessions_dir: raw.sessions_dir.clone(),
    });

    Ok(ResolvedStatRangeOptions {
        start: range.start,
        end: range.end,
        format,
        sessions_dir: paths.sessions_dir,
        verbose: raw.verbose,
    })
}

pub fn read_usage_records_report(
    options: &UsageRecordsReadOptions,
) -> Result<UsageRecordsReport, AppError> {
    let account_history = match &options.account_history_file {
        Some(path) => account_history::read_optional_usage_account_history(path)?,
        None => None,
    };
    let (usage_mode_history, usage_mode_history_present, usage_mode_history_switch_count) =
        match &options.usage_mode_history_file {
            Some(path) => read_usage_mode_history_summary(path)?,
            None => (None, false, 0),
        };
    let mut records = Vec::new();
    let resolved = ResolvedStatOptions {
        start: options.start,
        end: options.end,
        group_by: StatGroupBy::Day,
        limit_window: None,
        limit_group_by: None,
        format: StatFormat::Json,
        sessions_dir: options.sessions_dir.clone(),
        account_history_file: options.account_history_file.clone(),
        usage_mode_history_file: options.usage_mode_history_file.clone(),
        sort_by: None,
        limit: None,
        include_reasoning_effort: false,
        scan_all_files: options.scan_all_files,
        verbose: false,
        account_id: options.account_id.clone(),
        account_history,
        usage_mode_history,
        usage_mode_history_present,
        usage_mode_history_switch_count,
        usage_mode_history_include_path: false,
    };
    let mut fast_attributed_calls = 0_i64;
    let mut fast_attributed_credits = 0.0;
    let mut diagnostics = process_usage_records(&resolved, |record| {
        if record.usage_mode.is_fast() {
            let cost = calculate_credit_cost_with_context_at(
                record.model,
                record.usage.pricing_usage(),
                PricingContext::fast(),
                record.timestamp,
            );
            fast_attributed_calls += 1;
            fast_attributed_credits += cost.credits;
        }
        records.push(record.to_owned_record());
    })?;
    diagnostics.record_fast_attribution(fast_attributed_calls, fast_attributed_credits);

    Ok(UsageRecordsReport {
        start: options.start,
        end: options.end,
        sessions_dir: path_to_string(&options.sessions_dir),
        records,
        diagnostics,
    })
}

fn read_usage_mode_history_summary(
    path: &Path,
) -> Result<(Option<UsageModeHistory>, bool, i64), AppError> {
    let store = usage_mode_history::read_usage_mode_history_store(path)?;
    let history_present = store.default_mode.is_some() || !store.switches.is_empty();
    let switch_count = store.switches.len() as i64;
    let history = usage_mode_history::usage_mode_history_from_store(store)?;
    Ok((history, history_present, switch_count))
}

pub fn run_stat_command(
    view: Option<&str>,
    session: Option<&str>,
    options: StatCommandOptions,
    now: DateTime<Utc>,
) -> Result<String, AppError> {
    match view {
        None => {
            let resolved = resolve_stat_options(&options, now, false)?;
            if resolved.limit_window.is_some() {
                let report = read_limit_usage_stats(&resolved)?;
                format_limit_usage(&report, resolved.format, resolved.verbose)
            } else {
                let report = read_usage_stats(&resolved)?;
                format_usage_stats(&report, resolved.format, resolved.verbose)
            }
        }
        Some("sessions") => {
            if options.limit_window.is_some() {
                return Err(AppError::invalid_input(
                    "stat sessions does not support --limit-window. Use stat --limit-window 5h or 7d without a view.",
                ));
            }
            let mut resolved = resolve_stat_options(&options, now, session.is_some())?;
            if let Some(session_id) = session {
                resolved.scan_all_files = true;
                let report = read_usage_session_detail(&resolved, session_id)?;
                format_usage_session_detail(
                    &report,
                    resolved.format,
                    resolved.verbose,
                    options.detail,
                )
            } else {
                let top = match options.top.as_deref() {
                    Some(value) => Some(parse_positive_usize(value, "--top")?),
                    None => None,
                }
                .or(resolved.limit)
                .unwrap_or(10);
                let report = read_usage_sessions(&resolved, top)?;
                format_usage_sessions(&report, resolved.format, resolved.verbose)
            }
        }
        Some("fast-candidates") => Err(AppError::invalid_input(
            "fast candidate detection moved to: codex-ops fast candidates.",
        )),
        Some(other) => Err(AppError::new(format!("Unknown stat view: {other}"))),
    }
}

pub fn run_fast_candidates_command(
    options: StatCommandOptions,
    now: DateTime<Utc>,
) -> Result<String, AppError> {
    if options.limit_window.is_some() {
        return Err(AppError::invalid_input(
            "fast candidates always uses the 5h rate-limit window and does not accept --limit-window.",
        ));
    }

    let resolved = resolve_fast_candidate_options(&options, now)?;
    let report = read_fast_candidate_report(&resolved)?;
    format_fast_candidates(&report, resolved.format, resolved.verbose)
}

fn raw_range_options(raw: &StatCommandOptions) -> RawRangeOptions {
    RawRangeOptions {
        start: raw.start.clone(),
        end: raw.end.clone(),
        all: raw.all,
        today: raw.today,
        yesterday: raw.yesterday,
        month: raw.month,
        last: raw.last.clone(),
    }
}

fn resolve_stat_options(
    raw: &StatCommandOptions,
    now: DateTime<Utc>,
    force_full_scan: bool,
) -> Result<ResolvedStatOptions, AppError> {
    let format = if raw.json {
        StatFormat::Json
    } else {
        match raw.format.as_deref() {
            Some(value) => StatFormat::parse(value)?,
            None => StatFormat::Table,
        }
    };
    let range_options = raw_range_options(raw);
    let range = time::resolve_date_range(&range_options, now)?;
    if range.start > range.end {
        return Err(AppError::new(
            "The stat start time must be earlier than or equal to the end time.",
        ));
    }

    let explicit_group_by = match raw.group_by.as_deref() {
        Some(value) => Some(StatGroupBy::parse(value)?),
        None => None,
    };
    let limit_window = match raw.limit_window.as_deref() {
        Some(value) => Some(LimitWindowSelector::parse(value)?),
        None => None,
    };
    if limit_window.is_some() {
        validate_limit_window_group_by(explicit_group_by)?;
    }
    let group_by = match explicit_group_by {
        Some(value) => value,
        None => time::resolve_group_by(None, &range_options, &range)?,
    };
    let sort_by = match raw.sort.as_deref() {
        Some(value) => Some(StatSort::parse(value)?),
        None => None,
    };
    let limit = match raw.limit.as_deref() {
        Some(value) => Some(parse_positive_usize(value, "--limit")?),
        None => None,
    };
    let paths = resolve_storage_paths(&StorageOptions {
        codex_home: raw.codex_home.clone(),
        auth_file: raw.auth_file.clone(),
        profile_store_dir: None,
        account_history_file: raw.account_history_file.clone(),
        usage_mode_history_file: raw.usage_mode_history_file.clone(),
        sessions_dir: raw.sessions_dir.clone(),
    });
    let account_id = normalize_optional_string(raw.account_id.as_deref());
    let needs_required_account_history = account_id.is_some() || group_by == StatGroupBy::Account;
    let account_history = if needs_required_account_history {
        Some(ensure_usage_account_history(
            &paths.account_history_file,
            &AuthCommandOptions {
                auth_file: raw.auth_file.clone(),
                codex_home: raw.codex_home.clone(),
                store_dir: None,
                account_history_file: raw.account_history_file.clone(),
            },
            now,
        )?)
    } else if limit_window.is_some() {
        account_history::read_optional_usage_account_history(&paths.account_history_file)?
    } else {
        None
    };
    let (usage_mode_history, usage_mode_history_present, usage_mode_history_switch_count) =
        read_usage_mode_history_summary(&paths.usage_mode_history_file)?;

    Ok(ResolvedStatOptions {
        start: range.start,
        end: range.end,
        group_by,
        limit_window,
        limit_group_by: limit_window.and(explicit_group_by),
        format,
        sessions_dir: paths.sessions_dir,
        account_history_file: Some(paths.account_history_file),
        usage_mode_history_file: Some(paths.usage_mode_history_file),
        sort_by,
        limit,
        include_reasoning_effort: raw.reasoning_effort,
        scan_all_files: raw.full_scan || force_full_scan,
        verbose: raw.verbose,
        account_id,
        account_history,
        usage_mode_history,
        usage_mode_history_present,
        usage_mode_history_switch_count,
        usage_mode_history_include_path: raw.verbose && format == StatFormat::Json,
    })
}

fn validate_limit_window_group_by(group_by: Option<StatGroupBy>) -> Result<(), AppError> {
    match group_by {
        Some(StatGroupBy::Hour | StatGroupBy::Day | StatGroupBy::Week | StatGroupBy::Month) => {
            Err(AppError::invalid_input(
                "--limit-window can only be combined with --group-by model, cwd, or account. Time groupings hour, day, week, and month are not supported.",
            ))
        }
        Some(StatGroupBy::Model | StatGroupBy::Cwd | StatGroupBy::Account) | None => Ok(()),
    }
}

fn resolve_fast_candidate_options(
    raw: &StatCommandOptions,
    now: DateTime<Utc>,
) -> Result<ResolvedFastCandidateOptions, AppError> {
    let format = if raw.json {
        StatFormat::Json
    } else {
        match raw.format.as_deref() {
            Some(value) => StatFormat::parse(value)?,
            None => StatFormat::Table,
        }
    };
    let range_options = raw_range_options(raw);
    let range = time::resolve_date_range(&range_options, now)?;
    if range.start > range.end {
        return Err(AppError::new(
            "The stat start time must be earlier than or equal to the end time.",
        ));
    }

    let paths = resolve_storage_paths(&StorageOptions {
        codex_home: raw.codex_home.clone(),
        auth_file: raw.auth_file.clone(),
        profile_store_dir: None,
        account_history_file: raw.account_history_file.clone(),
        usage_mode_history_file: raw.usage_mode_history_file.clone(),
        sessions_dir: raw.sessions_dir.clone(),
    });
    let account_id = normalize_optional_string(raw.account_id.as_deref());
    if account_id.is_some() {
        ensure_usage_account_history(
            &paths.account_history_file,
            &AuthCommandOptions {
                auth_file: raw.auth_file.clone(),
                codex_home: raw.codex_home.clone(),
                store_dir: None,
                account_history_file: raw.account_history_file.clone(),
            },
            now,
        )?;
    }

    Ok(ResolvedFastCandidateOptions {
        start: range.start,
        end: range.end,
        format,
        sessions_dir: paths.sessions_dir,
        account_history_file: Some(paths.account_history_file),
        scan_all_files: raw.full_scan,
        verbose: raw.verbose,
        account_id,
    })
}

fn read_fast_candidate_report(
    options: &ResolvedFastCandidateOptions,
) -> Result<FastCandidateReport, AppError> {
    let samples = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
        start: options.start,
        end: options.end,
        sessions_dir: options.sessions_dir.clone(),
        scan_all_files: options.scan_all_files,
        account_history_file: options.account_history_file.clone(),
        account_id: options.account_id.clone(),
        plan_type: None,
        window_minutes: Some(LimitWindowSelector::FiveHours.window_minutes()),
    })?;
    let usage = read_usage_records_report(&UsageRecordsReadOptions {
        start: options.start,
        end: options.end,
        sessions_dir: options.sessions_dir.clone(),
        scan_all_files: options.scan_all_files,
        account_history_file: options.account_history_file.clone(),
        usage_mode_history_file: None,
        account_id: options.account_id.clone(),
    })?;

    Ok(build_fast_candidate_report(&samples, &usage))
}

fn read_limit_usage_stats(options: &ResolvedStatOptions) -> Result<LimitUsageReport, AppError> {
    let selector = options
        .limit_window
        .expect("limit usage report requires limit window");
    let sample_start = options
        .start
        .checked_sub_signed(Duration::minutes(selector.window_minutes()))
        .unwrap_or(options.start);
    let samples = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
        start: sample_start,
        end: options.end,
        sessions_dir: options.sessions_dir.clone(),
        scan_all_files: options.scan_all_files,
        account_history_file: options.account_history_file.clone(),
        account_id: options.account_id.clone(),
        plan_type: None,
        window_minutes: Some(selector.window_minutes()),
    })?;
    let windows = build_limit_windows_report(&samples, LimitReportOptions::default())
        .windows
        .into_iter()
        .filter(|window| window.reset_at > options.start && window.estimated_start <= options.end)
        .collect();
    let accumulator = LimitUsageAccumulator::new(LimitUsageAccumulatorConfig {
        start: options.start,
        end: options.end,
        selector,
        group_by: LimitUsageGroupBy::from_stat(options.limit_group_by),
        sessions_dir: path_to_string(&options.sessions_dir),
        include_reasoning_effort: options.include_reasoning_effort,
        sort_by: options.sort_by,
        limit: options.limit,
        windows,
    });
    let rate_limit_diagnostics = samples.diagnostics;
    let (accumulator, usage_diagnostics) = process_usage_records_parallel(options, accumulator)?;
    Ok(accumulator.finish(usage_diagnostics, rate_limit_diagnostics))
}

fn read_usage_stats(options: &ResolvedStatOptions) -> Result<UsageStatsReport, AppError> {
    let accumulator = UsageStatsAccumulator::new(
        options.start,
        options.end,
        options.group_by,
        path_to_string(&options.sessions_dir),
        options.include_reasoning_effort,
        options.sort_by,
        options.limit,
    );
    let (accumulator, diagnostics) = process_usage_records_parallel(options, accumulator)?;
    Ok(accumulator.finish(Some(diagnostics)))
}

fn read_usage_sessions(
    options: &ResolvedStatOptions,
    limit: usize,
) -> Result<UsageSessionsReport, AppError> {
    let accumulator = UsageSessionsAccumulator::new(
        options.start,
        options.end,
        path_to_string(&options.sessions_dir),
        options.sort_by,
        limit,
    );
    let (accumulator, diagnostics) = process_usage_records_parallel(options, accumulator)?;
    Ok(accumulator.finish(Some(diagnostics)))
}

fn read_usage_session_detail(
    options: &ResolvedStatOptions,
    session_id: &str,
) -> Result<UsageSessionDetailReport, AppError> {
    let accumulator = UsageSessionDetailAccumulator::new(
        options.start,
        options.end,
        path_to_string(&options.sessions_dir),
        options.limit,
        session_id.to_string(),
    );
    let (accumulator, diagnostics) = process_usage_records_parallel(options, accumulator)?;
    Ok(accumulator.finish(Some(diagnostics)))
}

fn parse_positive_usize(value: &str, name: &str) -> Result<usize, AppError> {
    let parsed = value.parse::<usize>().map_err(|_| {
        AppError::invalid_input(format!(
            "Invalid {name} value. Expected a positive integer."
        ))
    })?;
    if parsed == 0 {
        return Err(AppError::invalid_input(format!(
            "Invalid {name} value. Expected a positive integer."
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_stat_command_options() {
        let options = StatCommandOptions {
            group_by: Some("model".to_string()),
            sort: Some("credits".to_string()),
            limit: Some("1".to_string()),
            reasoning_effort: true,
            all: true,
            full_scan: true,
            verbose: true,
            json: true,
            sessions_dir: Some(PathBuf::from("/tmp/sessions")),
            ..StatCommandOptions::default()
        };
        let resolved = resolve_stat_options(
            &options,
            DateTime::parse_from_rfc3339("2026-05-17T00:00:00.000Z")
                .expect("now")
                .with_timezone(&Utc),
            false,
        )
        .expect("resolve");

        assert_eq!(resolved.group_by, StatGroupBy::Model);
        assert_eq!(resolved.sort_by, Some(StatSort::Credits));
        assert_eq!(resolved.limit, Some(1));
        assert!(resolved.include_reasoning_effort);
        assert!(resolved.scan_all_files);
        assert_eq!(resolved.format, StatFormat::Json);
    }

    #[test]
    fn validates_limit_window_contract_without_changing_default_group_by() {
        let now = DateTime::parse_from_rfc3339("2026-05-17T00:00:00.000Z")
            .expect("now")
            .with_timezone(&Utc);
        let default_group = resolve_stat_options(&StatCommandOptions::default(), now, false)
            .expect("default resolve")
            .group_by;

        let limit_default_group = resolve_stat_options(
            &StatCommandOptions {
                limit_window: Some("7d".to_string()),
                ..StatCommandOptions::default()
            },
            now,
            false,
        )
        .expect("limit window default resolve")
        .group_by;

        assert_eq!(limit_default_group, default_group);

        let model_group = resolve_stat_options(
            &StatCommandOptions {
                limit_window: Some("7d".to_string()),
                group_by: Some("model".to_string()),
                ..StatCommandOptions::default()
            },
            now,
            false,
        )
        .expect("model group is compatible");
        assert_eq!(model_group.group_by, StatGroupBy::Model);

        let bad_group = resolve_stat_options(
            &StatCommandOptions {
                limit_window: Some("7d".to_string()),
                group_by: Some("day".to_string()),
                ..StatCommandOptions::default()
            },
            now,
            false,
        )
        .expect_err("time group is incompatible");
        assert!(bad_group.message().contains("model, cwd, or account"));

        let bad_window = resolve_stat_options(
            &StatCommandOptions {
                limit_window: Some("bogus".to_string()),
                ..StatCommandOptions::default()
            },
            now,
            false,
        )
        .expect_err("unknown limit window");
        assert!(bad_window.message().contains("5h"));
        assert!(bad_window.message().contains("7d"));
    }

    #[test]
    fn limit_window_group_by_compatibility_is_explicit() {
        for group_by in [StatGroupBy::Model, StatGroupBy::Cwd, StatGroupBy::Account] {
            validate_limit_window_group_by(Some(group_by)).expect("allowed stat group");
        }

        for group_by in [
            StatGroupBy::Hour,
            StatGroupBy::Day,
            StatGroupBy::Week,
            StatGroupBy::Month,
        ] {
            let error =
                validate_limit_window_group_by(Some(group_by)).expect_err("time group rejected");
            assert!(error.message().contains("model, cwd, or account"));
        }

        validate_limit_window_group_by(None).expect("omitted group-by is valid");
    }
}
