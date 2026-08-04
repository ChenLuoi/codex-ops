use crate::error::AppError;
use crate::format::{credits_to_usd, round_credits};
use crate::pricing::{
    calculate_credit_cost_with_context_at, PricingContext, TokenUsage as PricingTokenUsage,
};
use crate::stats::{UsageRateLimit, UsageRecord};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitParseDiagnostics {
    pub invalid_json_lines: i64,
    pub rate_limit_events: i64,
    pub included_samples: i64,
    pub null_rate_limits: i64,
    pub missing_rate_limits: i64,
    pub missing_timestamps: i64,
    pub missing_windows: i64,
    pub unknown_windows: i64,
    pub invalid_window_minutes: i64,
    pub invalid_used_percent: i64,
    pub invalid_resets_at: i64,
    pub out_of_range_percent: i64,
}

#[derive(Clone, Debug)]
pub struct RateLimitSamplesReadOptions {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: PathBuf,
    pub scan_all_files: bool,
    pub account_history_file: Option<PathBuf>,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub window_minutes: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct RateLimitSamplesReport {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub samples: Vec<RateLimitSample>,
    pub diagnostics: RateLimitDiagnostics,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitDiagnostics {
    pub scan_all_files: bool,
    pub scanned_directories: i64,
    pub skipped_directories: i64,
    pub read_files: i64,
    pub skipped_files: i64,
    pub prefiltered_files: i64,
    pub tail_read_files: i64,
    pub tail_read_hits: i64,
    pub mtime_read_files: i64,
    pub mtime_tail_hits: i64,
    pub mtime_read_hits: i64,
    pub fork_files: i64,
    pub fork_parent_missing: i64,
    pub fork_replay_lines: i64,
    pub read_lines: i64,
    pub invalid_json_lines: i64,
    pub rate_limit_events: i64,
    pub included_samples: i64,
    pub null_rate_limits: i64,
    pub missing_rate_limits: i64,
    pub missing_timestamps: i64,
    pub missing_windows: i64,
    pub unknown_windows: i64,
    pub invalid_window_minutes: i64,
    pub invalid_used_percent: i64,
    pub invalid_resets_at: i64,
    pub out_of_range_percent: i64,
    pub rate_limit_only_files: i64,
    pub account_mismatches: i64,
    pub plan_mismatches: i64,
    pub window_mismatches: i64,
    pub out_of_range_samples: i64,
    pub fork_replay_lines_skipped: i64,
    pub file_read_concurrency: i64,
    #[serde(skip)]
    pub(crate) source_spans: Vec<SourceSpan>,
}

impl RateLimitDiagnostics {
    pub(crate) fn new(file_read_concurrency: i64, scan_all_files: bool) -> Self {
        Self {
            scan_all_files,
            file_read_concurrency,
            ..Self::default()
        }
    }

    pub(crate) fn merge_parse(&mut self, other: &RateLimitParseDiagnostics) {
        self.invalid_json_lines += other.invalid_json_lines;
        self.rate_limit_events += other.rate_limit_events;
        self.null_rate_limits += other.null_rate_limits;
        self.missing_rate_limits += other.missing_rate_limits;
        self.missing_timestamps += other.missing_timestamps;
        self.missing_windows += other.missing_windows;
        self.unknown_windows += other.unknown_windows;
        self.invalid_window_minutes += other.invalid_window_minutes;
        self.invalid_used_percent += other.invalid_used_percent;
        self.invalid_resets_at += other.invalid_resets_at;
        self.out_of_range_percent += other.out_of_range_percent;
    }

    pub(crate) fn merge_file_scan(&mut self, other: &RateLimitDiagnostics) {
        self.read_lines += other.read_lines;
        self.invalid_json_lines += other.invalid_json_lines;
        self.rate_limit_events += other.rate_limit_events;
        self.included_samples += other.included_samples;
        self.null_rate_limits += other.null_rate_limits;
        self.missing_rate_limits += other.missing_rate_limits;
        self.missing_timestamps += other.missing_timestamps;
        self.missing_windows += other.missing_windows;
        self.unknown_windows += other.unknown_windows;
        self.invalid_window_minutes += other.invalid_window_minutes;
        self.invalid_used_percent += other.invalid_used_percent;
        self.invalid_resets_at += other.invalid_resets_at;
        self.out_of_range_percent += other.out_of_range_percent;
        self.rate_limit_only_files += other.rate_limit_only_files;
        self.account_mismatches += other.account_mismatches;
        self.plan_mismatches += other.plan_mismatches;
        self.window_mismatches += other.window_mismatches;
        self.out_of_range_samples += other.out_of_range_samples;
        self.fork_replay_lines_skipped += other.fork_replay_lines_skipped;
        self.source_spans.extend(other.source_spans.iter().cloned());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpan {
    pub(crate) path: String,
    pub(crate) line_number: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSample {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub window: String,
    pub window_minutes: i64,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: DateTime<Utc>,
    #[serde(skip)]
    pub(crate) source: Option<SourceSpan>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LimitReportOptions {
    pub include_diagnostics: bool,
    pub include_source_evidence: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitWindowSelector {
    FiveHours,
    SevenDays,
}

impl LimitWindowSelector {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "5h" => Ok(Self::FiveHours),
            "7d" => Ok(Self::SevenDays),
            _ => Err(AppError::invalid_input(
                "Invalid limit window. Expected one of: 5h, 7d.",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveHours => "5h",
            Self::SevenDays => "7d",
        }
    }

    pub fn window_minutes(self) -> i64 {
        match self {
            Self::FiveHours => 300,
            Self::SevenDays => 10_080,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitSourceEvidence {
    pub path: String,
    pub line_number: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitReportDiagnostics {
    #[serde(flatten)]
    pub scan: RateLimitDiagnostics,
    pub duplicate_samples: i64,
    pub unknown_limit_samples: i64,
    pub unknown_limit_reset_events: i64,
    pub ignored_inactive_stream_samples: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_evidence: Vec<LimitSourceEvidence>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    pub id: String,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub window: String,
    pub window_minutes: i64,
    pub estimated_start: DateTime<Utc>,
    pub reset_at: DateTime<Utc>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub min_used_percent: f64,
    pub max_used_percent: f64,
    pub last_used_percent: f64,
    pub sample_count: i64,
    pub reset_kind: String,
    pub total_tokens: i64,
    pub credits: f64,
    pub usd: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitResetEvent {
    pub at: DateTime<Utc>,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub window: String,
    pub window_minutes: i64,
    pub previous_used_percent: f64,
    pub next_used_percent: f64,
    pub previous_resets_at: DateTime<Utc>,
    pub next_resets_at: DateTime<Utc>,
    pub early_by_seconds: i64,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitTrendChange {
    pub at: DateTime<Utc>,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub window: String,
    pub window_minutes: i64,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub delta_used_percent: Option<f64>,
    pub resets_at: DateTime<Utc>,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitCurrentWindow {
    pub id: String,
    pub status: String,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub window: String,
    pub window_minutes: i64,
    pub last_seen: Option<DateTime<Utc>>,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_in_seconds: Option<i64>,
    pub total_tokens: i64,
    pub credits: f64,
    pub usd: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitSamplesReport {
    pub status: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub samples: Vec<RateLimitSample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<LimitReportDiagnostics>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindowsReport {
    pub status: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub windows: Vec<LimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<LimitReportDiagnostics>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitResetsReport {
    pub status: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub early_only: bool,
    pub resets: Vec<LimitResetEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<LimitReportDiagnostics>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitTrendReport {
    pub status: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub changes: Vec<LimitTrendChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<LimitReportDiagnostics>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitCurrentReport {
    pub status: String,
    pub now: DateTime<Utc>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub current: Vec<LimitCurrentWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<LimitReportDiagnostics>,
}

pub fn build_limit_samples_report(
    input: &RateLimitSamplesReport,
    options: LimitReportOptions,
) -> LimitSamplesReport {
    let (_, duplicate_samples) = normalized_samples(&input.samples);
    LimitSamplesReport {
        status: status_for_count(input.samples.len()),
        start: input.start,
        end: input.end,
        sessions_dir: input.sessions_dir.clone(),
        samples: input.samples.clone(),
        diagnostics: diagnostics_for_options(input, duplicate_samples, 0, 0, options),
    }
}

pub fn build_limit_windows_report(
    input: &RateLimitSamplesReport,
    options: LimitReportOptions,
) -> LimitWindowsReport {
    let (samples, duplicate_samples, ignored_inactive_stream_samples) =
        normalized_derived_samples(&input.samples);
    let windows = build_windows(&samples);

    LimitWindowsReport {
        status: status_for_count(windows.len()),
        start: input.start,
        end: input.end,
        sessions_dir: input.sessions_dir.clone(),
        windows,
        diagnostics: diagnostics_for_options(
            input,
            duplicate_samples,
            ignored_inactive_stream_samples,
            0,
            options,
        ),
    }
}

pub fn build_limit_resets_report(
    input: &RateLimitSamplesReport,
    early_only: bool,
    options: LimitReportOptions,
) -> LimitResetsReport {
    let (samples, duplicate_samples, ignored_inactive_stream_samples) =
        normalized_derived_samples(&input.samples);
    let mut resets = build_resets(&samples);
    if early_only {
        resets.retain(|reset| reset.kind == RESET_KIND_EARLY);
    }
    let unknown_limit_reset_events = count_unknown_limit_reset_events(&resets);

    LimitResetsReport {
        status: status_for_count(samples.len()),
        start: input.start,
        end: input.end,
        sessions_dir: input.sessions_dir.clone(),
        early_only,
        resets,
        diagnostics: diagnostics_for_options(
            input,
            duplicate_samples,
            ignored_inactive_stream_samples,
            unknown_limit_reset_events,
            options,
        ),
    }
}

pub fn build_limit_trend_report(
    input: &RateLimitSamplesReport,
    window_minutes: Option<i64>,
    options: LimitReportOptions,
) -> LimitTrendReport {
    let (samples, duplicate_samples, ignored_inactive_stream_samples) =
        normalized_derived_samples(&input.samples);
    let changes = build_trend_changes(&samples, window_minutes);

    LimitTrendReport {
        status: status_for_count(changes.len()),
        start: input.start,
        end: input.end,
        sessions_dir: input.sessions_dir.clone(),
        changes,
        diagnostics: diagnostics_for_options(
            input,
            duplicate_samples,
            ignored_inactive_stream_samples,
            0,
            options,
        ),
    }
}

pub fn build_limit_current_report(
    input: &RateLimitSamplesReport,
    now: DateTime<Utc>,
    options: LimitReportOptions,
) -> LimitCurrentReport {
    let (samples, duplicate_samples, ignored_inactive_stream_samples) =
        normalized_derived_samples(&input.samples);
    let current = build_current_windows(&samples, now);

    LimitCurrentReport {
        status: status_for_current(&current),
        now,
        start: input.start,
        end: input.end,
        sessions_dir: input.sessions_dir.clone(),
        current,
        diagnostics: diagnostics_for_options(
            input,
            duplicate_samples,
            ignored_inactive_stream_samples,
            0,
            options,
        ),
    }
}

const UNKNOWN_ACCOUNT: &str = "unknown_account";
const UNKNOWN_PLAN: &str = "unknown_plan";
const UNKNOWN_LIMIT: &str = "unknown_limit";
const RESET_KIND_FIRST_OBSERVED: &str = "firstObserved";
const RESET_KIND_NORMAL: &str = "normal";
const RESET_KIND_EARLY: &str = "early";
const RESET_KIND_CHANGED: &str = "changed";
const CURRENT_STATUS_ACTIVE: &str = "active";
const CURRENT_STATUS_EXPIRED: &str = "expired";
const TREND_KIND_INCREASED: &str = "increased";
const TREND_KIND_RESET_CHANGED: &str = "resetChanged";
const RESET_JITTER_TOLERANCE_SECONDS: i64 = 60;
const INACTIVE_STREAM_MIN_SAMPLES: usize = 3;
const INACTIVE_STREAM_MIN_SPAN_SECONDS: i64 = 60;
const PERCENT_EPSILON: f64 = 0.000_001;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct PartitionKey {
    account_id: String,
    plan_type: String,
    limit_id: String,
    window_minutes: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SampleIdentity {
    partition: PartitionKey,
    timestamp: DateTime<Utc>,
    resets_at: DateTime<Utc>,
    window_minutes: i64,
    limit_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrendPartitionKey {
    account_id: String,
    plan_type: String,
    limit_id: String,
    window_minutes: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrendStreamKey {
    account_id: String,
    plan_type: String,
    limit_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrendObservationKey {
    stream: TrendStreamKey,
    timestamp: DateTime<Utc>,
    session_id: String,
    source_path: String,
    source_line: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrendSourceKey {
    stream: TrendStreamKey,
    source: String,
}

#[derive(Clone, Debug)]
struct TrendObservation {
    stream: TrendStreamKey,
    timestamp: DateTime<Utc>,
    session_id: String,
    source_path: String,
    source_line: usize,
    windows: BTreeMap<i64, RateLimitSample>,
}

#[derive(Clone, Debug)]
struct WindowAccumulator {
    partition: PartitionKey,
    window: String,
    reset_at: DateTime<Utc>,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    min_used_percent: f64,
    max_used_percent: f64,
    last_used_percent: f64,
    sample_count: i64,
    reset_kind: &'static str,
}

impl WindowAccumulator {
    fn new(sample: &RateLimitSample, reset_kind: &'static str) -> Self {
        Self {
            partition: partition_key(sample),
            window: sample.window.clone(),
            reset_at: sample.resets_at,
            first_seen: sample.timestamp,
            last_seen: sample.timestamp,
            min_used_percent: sample.used_percent,
            max_used_percent: sample.used_percent,
            last_used_percent: sample.used_percent,
            sample_count: 1,
            reset_kind,
        }
    }

    fn push(&mut self, sample: &RateLimitSample) {
        self.reset_at = sample.resets_at;
        self.last_seen = sample.timestamp;
        self.min_used_percent = self.min_used_percent.min(sample.used_percent);
        self.max_used_percent = self.max_used_percent.max(sample.used_percent);
        self.last_used_percent = sample.used_percent;
        self.sample_count += 1;
    }

    fn finish(self) -> LimitWindow {
        let estimated_start = self
            .reset_at
            .checked_sub_signed(Duration::minutes(self.partition.window_minutes))
            .unwrap_or(self.reset_at);
        LimitWindow {
            id: limit_window_id(&self.partition, self.reset_at, &self.window),
            account_id: output_account_id(&self.partition),
            plan_type: output_plan_type(&self.partition),
            limit_id: output_limit_id(&self.partition),
            window: self.window,
            window_minutes: self.partition.window_minutes,
            estimated_start,
            reset_at: self.reset_at,
            first_seen: self.first_seen,
            last_seen: self.last_seen,
            min_used_percent: self.min_used_percent,
            max_used_percent: self.max_used_percent,
            last_used_percent: self.last_used_percent,
            sample_count: self.sample_count,
            reset_kind: self.reset_kind.to_string(),
            total_tokens: 0,
            credits: 0.0,
            usd: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct WindowUsageTotals {
    total_tokens: i64,
    credits: f64,
}

#[derive(Clone)]
struct UsageWindowTarget {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    account_id: Option<String>,
    plan_type: Option<String>,
    limit_id: Option<String>,
    window_minutes: i64,
    reset_at: DateTime<Utc>,
}

pub fn limit_windows_usage_range(
    windows: &[LimitWindow],
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    usage_range(
        windows
            .iter()
            .map(|window| (window.estimated_start, window.reset_at)),
    )
}

pub fn limit_current_usage_range(
    current: &[LimitCurrentWindow],
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    usage_range(current.iter().filter_map(limit_current_period))
}

pub fn attach_usage_to_limit_windows(windows: &mut [LimitWindow], records: &[UsageRecord]) {
    let targets = windows
        .iter()
        .map(|window| UsageWindowTarget {
            start: window.estimated_start,
            end: window.reset_at,
            account_id: window.account_id.clone(),
            plan_type: window.plan_type.clone(),
            limit_id: window.limit_id.clone(),
            window_minutes: window.window_minutes,
            reset_at: window.reset_at,
        })
        .collect::<Vec<_>>();
    let totals = usage_totals_for_targets(records, &targets);

    for (window, totals) in windows.iter_mut().zip(totals) {
        window.total_tokens = totals.total_tokens;
        window.credits = round_credits(totals.credits);
        window.usd = credits_to_usd(totals.credits);
    }
}

pub fn attach_usage_to_limit_current(current: &mut [LimitCurrentWindow], records: &[UsageRecord]) {
    let targets = current
        .iter()
        .map(|row| {
            limit_current_period(row).map(|(start, end)| UsageWindowTarget {
                start,
                end,
                account_id: row.account_id.clone(),
                plan_type: row.plan_type.clone(),
                limit_id: row.limit_id.clone(),
                window_minutes: row.window_minutes,
                reset_at: end,
            })
        })
        .collect::<Vec<_>>();
    let concrete_targets = targets.iter().flatten().cloned().collect::<Vec<_>>();
    let concrete_totals = usage_totals_for_targets(records, &concrete_targets);
    let mut totals_iter = concrete_totals.into_iter();

    for (row, target) in current.iter_mut().zip(targets) {
        let Some(_) = target else {
            continue;
        };
        let totals = totals_iter
            .next()
            .expect("current target and totals counts match");
        row.total_tokens = totals.total_tokens;
        row.credits = round_credits(totals.credits);
        row.usd = credits_to_usd(totals.credits);
    }
}

fn usage_range<I>(periods: I) -> Option<(DateTime<Utc>, DateTime<Utc>)>
where
    I: IntoIterator<Item = (DateTime<Utc>, DateTime<Utc>)>,
{
    let mut periods = periods.into_iter();
    let (mut start, mut end) = periods.next()?;
    for (period_start, period_end) in periods {
        start = start.min(period_start);
        end = end.max(period_end);
    }
    Some((start, end))
}

fn limit_current_period(row: &LimitCurrentWindow) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let reset_at = row.resets_at?;
    let start = reset_at
        .checked_sub_signed(Duration::minutes(row.window_minutes))
        .unwrap_or(reset_at);
    Some((start, reset_at))
}

fn usage_totals_for_targets(
    records: &[UsageRecord],
    targets: &[UsageWindowTarget],
) -> Vec<WindowUsageTotals> {
    let mut totals = vec![WindowUsageTotals::default(); targets.len()];
    for record in records {
        let cost = calculate_credit_cost_with_context_at(
            &record.model,
            PricingTokenUsage {
                input_tokens: record.usage.input_tokens.max(0) as u64,
                cached_input_tokens: record.usage.cached_input_tokens.max(0) as u64,
                output_tokens: record.usage.output_tokens.max(0) as u64,
            },
            if record.usage_mode.is_fast() {
                PricingContext::fast()
            } else {
                PricingContext::normal()
            },
            record.timestamp,
        );
        for target_index in usage_target_indexes_for_record(record, targets) {
            let target_totals = totals
                .get_mut(target_index)
                .expect("target index is from targets");
            target_totals.total_tokens += record.usage.total_tokens;
            target_totals.credits += cost.credits;
        }
    }
    totals
}

fn usage_target_indexes_for_record(
    record: &UsageRecord,
    targets: &[UsageWindowTarget],
) -> Vec<usize> {
    let time_account_matches = targets
        .iter()
        .enumerate()
        .filter(|(_, target)| {
            record.timestamp >= target.start
                && record.timestamp < target.end
                && usage_account_matches(target.account_id.as_deref(), record.account_id.as_deref())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if !record.rate_limits.is_empty() {
        let exact_matches = time_account_matches
            .iter()
            .copied()
            .filter(|index| {
                record
                    .rate_limits
                    .iter()
                    .any(|rate_limit| usage_rate_limit_matches_target(rate_limit, &targets[*index]))
            })
            .collect::<Vec<_>>();
        if !exact_matches.is_empty() {
            return exact_matches;
        }
    }

    non_ambiguous_legacy_target_indexes(&time_account_matches, targets)
}

fn non_ambiguous_legacy_target_indexes(
    candidate_indexes: &[usize],
    targets: &[UsageWindowTarget],
) -> Vec<usize> {
    let mut selected = Vec::new();
    for index in candidate_indexes {
        let target = &targets[*index];
        let equivalent_candidates = candidate_indexes
            .iter()
            .filter(|candidate_index| {
                let candidate = &targets[**candidate_index];
                candidate.window_minutes == target.window_minutes
                    && candidate.account_id == target.account_id
                    && candidate.plan_type == target.plan_type
            })
            .count();
        if equivalent_candidates == 1 {
            selected.push(*index);
        }
    }
    selected
}

fn usage_rate_limit_matches_target(
    rate_limit: &UsageRateLimit,
    target: &UsageWindowTarget,
) -> bool {
    rate_limit.window_minutes == target.window_minutes
        && is_reset_time_equal_within_jitter(rate_limit.resets_at, target.reset_at)
        && rate_limit.plan_type.as_deref() == target.plan_type.as_deref()
        && rate_limit.limit_id.as_deref() == target.limit_id.as_deref()
}

fn usage_account_matches(window_account_id: Option<&str>, record_account_id: Option<&str>) -> bool {
    match record_account_id {
        Some(account_id) => {
            window_account_id.is_none_or(|window_account| window_account == account_id)
        }
        None => window_account_id.is_none(),
    }
}

fn normalized_samples(samples: &[RateLimitSample]) -> (Vec<RateLimitSample>, i64) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| {
        partition_key(left)
            .cmp(&partition_key(right))
            .then_with(|| left.timestamp.cmp(&right.timestamp))
            .then_with(|| left.resets_at.cmp(&right.resets_at))
            .then_with(|| left.window_minutes.cmp(&right.window_minutes))
            .then_with(|| left.limit_id.cmp(&right.limit_id))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });

    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(sorted.len());
    let mut duplicate_samples = 0_i64;
    for sample in sorted {
        if seen.insert(sample_identity(&sample)) {
            normalized.push(sample);
        } else {
            duplicate_samples += 1;
        }
    }

    (normalized, duplicate_samples)
}

fn normalized_derived_samples(samples: &[RateLimitSample]) -> (Vec<RateLimitSample>, i64, i64) {
    let (normalized, duplicate_samples) = normalized_samples(samples);
    let (filtered, ignored_inactive_stream_samples) =
        filter_inactive_rolling_zero_streams(normalized);
    (filtered, duplicate_samples, ignored_inactive_stream_samples)
}

fn filter_inactive_rolling_zero_streams(
    samples: Vec<RateLimitSample>,
) -> (Vec<RateLimitSample>, i64) {
    let inactive_partitions = inactive_rolling_zero_partitions(&samples);
    if inactive_partitions.is_empty() {
        return (samples, 0);
    }

    let mut ignored = 0_i64;
    let mut filtered = Vec::with_capacity(samples.len());
    for sample in samples {
        if inactive_partitions.contains(&partition_key(&sample)) {
            ignored += 1;
        } else {
            filtered.push(sample);
        }
    }
    (filtered, ignored)
}

fn inactive_rolling_zero_partitions(samples: &[RateLimitSample]) -> BTreeSet<PartitionKey> {
    let mut inactive = BTreeSet::new();
    for (partition, partition_samples) in partitioned_samples(samples) {
        if is_inactive_rolling_zero_stream(&partition_samples) {
            inactive.insert(partition);
        }
    }
    inactive
}

fn is_inactive_rolling_zero_stream(samples: &[&RateLimitSample]) -> bool {
    if samples.len() < INACTIVE_STREAM_MIN_SAMPLES {
        return false;
    }

    let first_seen = samples
        .first()
        .expect("inactive stream has first sample")
        .timestamp;
    let last_seen = samples
        .last()
        .expect("inactive stream has last sample")
        .timestamp;
    if (last_seen - first_seen).num_seconds() < INACTIVE_STREAM_MIN_SPAN_SECONDS {
        return false;
    }

    samples.iter().all(|sample| {
        sample.used_percent.abs() <= PERCENT_EPSILON && is_rolling_full_window_reset(sample)
    })
}

fn is_rolling_full_window_reset(sample: &RateLimitSample) -> bool {
    let Some(expected_reset) = sample
        .timestamp
        .checked_add_signed(Duration::minutes(sample.window_minutes))
    else {
        return false;
    };
    is_reset_time_equal_within_jitter(expected_reset, sample.resets_at)
}

fn build_windows(samples: &[RateLimitSample]) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    for (_, partition_samples) in partitioned_samples(samples) {
        let mut current: Option<WindowAccumulator> = None;
        let mut previous_sample: Option<RateLimitSample> = None;

        for sample in partition_samples {
            let reset_kind = previous_sample
                .as_ref()
                .map(|previous| transition_kind(previous, sample))
                .unwrap_or(RESET_KIND_FIRST_OBSERVED);
            match current.as_mut() {
                Some(window)
                    if is_reset_time_equal_within_jitter(window.reset_at, sample.resets_at) =>
                {
                    window.push(sample);
                }
                Some(_) => {
                    windows.push(current.take().expect("window exists").finish());
                    current = Some(WindowAccumulator::new(sample, reset_kind));
                }
                None => current = Some(WindowAccumulator::new(sample, RESET_KIND_FIRST_OBSERVED)),
            }
            previous_sample = Some((*sample).clone());
        }

        if let Some(window) = current {
            windows.push(window.finish());
        }
    }

    windows.sort_by(|left, right| {
        left.reset_at
            .cmp(&right.reset_at)
            .then_with(|| left.first_seen.cmp(&right.first_seen))
            .then_with(|| left.window_minutes.cmp(&right.window_minutes))
            .then_with(|| left.account_id.cmp(&right.account_id))
            .then_with(|| left.plan_type.cmp(&right.plan_type))
            .then_with(|| left.limit_id.cmp(&right.limit_id))
    });
    windows
}

fn build_resets(samples: &[RateLimitSample]) -> Vec<LimitResetEvent> {
    let mut events = Vec::new();
    for (partition, partition_samples) in partitioned_samples(samples) {
        for pair in partition_samples.windows(2) {
            let previous = pair[0];
            let next = pair[1];
            if !is_reset_transition(previous, next) {
                continue;
            }
            let kind = transition_kind(previous, next);
            let early_by_seconds = (previous.resets_at - next.timestamp).num_seconds().max(0);
            events.push(LimitResetEvent {
                at: next.timestamp,
                account_id: output_account_id(&partition),
                plan_type: output_plan_type(&partition),
                limit_id: output_limit_id(&partition),
                window: next.window.clone(),
                window_minutes: next.window_minutes,
                previous_used_percent: previous.used_percent,
                next_used_percent: next.used_percent,
                previous_resets_at: previous.resets_at,
                next_resets_at: next.resets_at,
                early_by_seconds,
                kind: kind.to_string(),
            });
        }
    }

    events.sort_by(|left, right| {
        left.at
            .cmp(&right.at)
            .then_with(|| left.window_minutes.cmp(&right.window_minutes))
            .then_with(|| left.account_id.cmp(&right.account_id))
            .then_with(|| left.plan_type.cmp(&right.plan_type))
    });
    events
}

fn build_trend_changes(
    samples: &[RateLimitSample],
    window_minutes: Option<i64>,
) -> Vec<LimitTrendChange> {
    let mut changes = Vec::new();
    let observations = compact_trend_observations_by_source(trend_observations(samples));
    for (_, mut stream_observations) in trend_stream_observations(observations) {
        stream_observations.sort_by(compare_trend_observation_order);
        let mut state_by_window = BTreeMap::<i64, RateLimitSample>::new();

        for observation in stream_observations {
            let mut accepted_windows = BTreeMap::<i64, &'static str>::new();
            for (window, sample) in &observation.windows {
                if !is_active_trend_sample(sample) {
                    continue;
                }
                let kind = match state_by_window.get(window) {
                    Some(previous) => trend_window_change_kind(previous, sample),
                    None => Some(RESET_KIND_FIRST_OBSERVED),
                };
                if let Some(kind) = kind {
                    accepted_windows.insert(*window, kind);
                }
            }

            if accepted_windows.is_empty() {
                continue;
            }

            let output_windows = trend_output_windows(&observation, window_minutes);
            for window in output_windows {
                let Some(sample) = observation.windows.get(&window) else {
                    continue;
                };
                if !is_active_trend_sample(sample) {
                    continue;
                }

                let previous = state_by_window.get(&window);
                if let Some(kind) = accepted_windows.get(&window) {
                    changes.push(trend_change_from_sample(
                        sample,
                        previous.map(|previous| sample.used_percent - previous.used_percent),
                        kind,
                    ));
                    state_by_window.insert(window, sample.clone());
                }
            }

            for window in accepted_windows.keys() {
                if let Some(sample) = observation.windows.get(window) {
                    state_by_window.insert(*window, sample.clone());
                }
            }
        }
    }

    changes.sort_by(|left, right| {
        left.at
            .cmp(&right.at)
            .then_with(|| left.window_minutes.cmp(&right.window_minutes))
            .then_with(|| left.account_id.cmp(&right.account_id))
            .then_with(|| left.plan_type.cmp(&right.plan_type))
            .then_with(|| left.limit_id.cmp(&right.limit_id))
    });
    changes
}

fn build_current_windows(
    samples: &[RateLimitSample],
    now: DateTime<Utc>,
) -> Vec<LimitCurrentWindow> {
    let mut windows_by_partition = BTreeMap::<
        (i64, Option<String>, Option<String>, Option<String>, String),
        Vec<LimitWindow>,
    >::new();
    for window in build_windows(samples)
        .into_iter()
        .filter(|window| window.first_seen <= now)
    {
        windows_by_partition
            .entry((
                window.window_minutes,
                window.account_id.clone(),
                window.plan_type.clone(),
                window.limit_id.clone(),
                window.window.clone(),
            ))
            .or_default()
            .push(window);
    }

    let mut current = Vec::new();
    for (_, mut partition_windows) in windows_by_partition {
        partition_windows.sort_by(compare_limit_window_order);

        if let Some(window) = partition_windows.last() {
            current.push(limit_current_from_window(window, now));
        }
    }

    current.sort_by(|left, right| {
        left.window_minutes
            .cmp(&right.window_minutes)
            .then_with(|| left.account_id.cmp(&right.account_id))
            .then_with(|| left.plan_type.cmp(&right.plan_type))
            .then_with(|| left.limit_id.cmp(&right.limit_id))
            .then_with(|| left.status.cmp(&right.status))
            .then_with(|| left.resets_at.cmp(&right.resets_at))
            .then_with(|| left.last_seen.cmp(&right.last_seen))
    });
    current
}

fn compare_limit_window_order(left: &LimitWindow, right: &LimitWindow) -> std::cmp::Ordering {
    left.reset_at
        .cmp(&right.reset_at)
        .then_with(|| left.last_seen.cmp(&right.last_seen))
        .then_with(|| left.first_seen.cmp(&right.first_seen))
}

fn limit_current_from_window(window: &LimitWindow, now: DateTime<Utc>) -> LimitCurrentWindow {
    let active = window.reset_at > now;
    LimitCurrentWindow {
        id: format!("{}-current", window.id),
        status: if active {
            CURRENT_STATUS_ACTIVE
        } else {
            CURRENT_STATUS_EXPIRED
        }
        .to_string(),
        account_id: window.account_id.clone(),
        plan_type: window.plan_type.clone(),
        limit_id: window.limit_id.clone(),
        window: window.window.clone(),
        window_minutes: window.window_minutes,
        last_seen: Some(window.last_seen),
        used_percent: Some(window.last_used_percent),
        remaining_percent: Some(100.0 - window.last_used_percent),
        resets_at: Some(window.reset_at),
        reset_in_seconds: active.then_some((window.reset_at - now).num_seconds()),
        total_tokens: window.total_tokens,
        credits: window.credits,
        usd: window.usd,
    }
}

fn partitioned_samples(
    samples: &[RateLimitSample],
) -> BTreeMap<PartitionKey, Vec<&RateLimitSample>> {
    let mut partitions: BTreeMap<PartitionKey, Vec<&RateLimitSample>> = BTreeMap::new();
    for sample in samples {
        partitions
            .entry(partition_key(sample))
            .or_default()
            .push(sample);
    }
    for partition_samples in partitions.values_mut() {
        partition_samples.sort_by(|left, right| compare_sample_order(left, right));
    }
    partitions
}

fn trend_observations(samples: &[RateLimitSample]) -> Vec<TrendObservation> {
    let mut observations = BTreeMap::<TrendObservationKey, TrendObservation>::new();
    for sample in samples {
        let key = trend_observation_key(sample);
        observations
            .entry(key.clone())
            .or_insert_with(|| TrendObservation {
                stream: key.stream.clone(),
                timestamp: key.timestamp,
                session_id: key.session_id.clone(),
                source_path: key.source_path.clone(),
                source_line: key.source_line,
                windows: BTreeMap::new(),
            })
            .windows
            .insert(sample.window_minutes, sample.clone());
    }

    observations.into_values().collect()
}

fn compact_trend_observations_by_source(
    observations: Vec<TrendObservation>,
) -> Vec<TrendObservation> {
    let mut by_source = BTreeMap::<TrendSourceKey, Vec<TrendObservation>>::new();
    for observation in observations {
        by_source
            .entry(TrendSourceKey {
                stream: observation.stream.clone(),
                source: trend_observation_source(&observation),
            })
            .or_default()
            .push(observation);
    }

    let mut compacted = Vec::new();
    for observations in by_source.values_mut() {
        observations.sort_by(compare_trend_observation_order);
        let mut previous: Option<&TrendObservation> = None;
        for observation in observations.iter() {
            if previous
                .is_some_and(|previous| trend_observation_vector_equal(previous, observation))
            {
                continue;
            }
            compacted.push(observation.clone());
            previous = Some(observation);
        }
    }

    compacted
}

fn trend_stream_observations(
    observations: Vec<TrendObservation>,
) -> BTreeMap<TrendStreamKey, Vec<TrendObservation>> {
    let mut by_stream = BTreeMap::<TrendStreamKey, Vec<TrendObservation>>::new();
    for observation in observations {
        by_stream
            .entry(observation.stream.clone())
            .or_default()
            .push(observation);
    }
    by_stream
}

fn trend_observation_key(sample: &RateLimitSample) -> TrendObservationKey {
    let (source_path, source_line) = sample
        .source
        .as_ref()
        .map(|source| (source.path.clone(), source.line_number))
        .unwrap_or_else(|| (String::new(), 0));
    TrendObservationKey {
        stream: trend_stream_key(sample),
        timestamp: sample.timestamp,
        session_id: sample.session_id.clone(),
        source_path,
        source_line,
    }
}

fn trend_observation_source(observation: &TrendObservation) -> String {
    if observation.source_path.is_empty() {
        observation.session_id.clone()
    } else {
        observation.source_path.clone()
    }
}

fn compare_trend_observation_order(
    left: &TrendObservation,
    right: &TrendObservation,
) -> std::cmp::Ordering {
    left.timestamp
        .cmp(&right.timestamp)
        .then_with(|| left.source_path.cmp(&right.source_path))
        .then_with(|| left.source_line.cmp(&right.source_line))
        .then_with(|| left.session_id.cmp(&right.session_id))
}

fn trend_observation_vector_equal(left: &TrendObservation, right: &TrendObservation) -> bool {
    if left.windows.len() != right.windows.len() {
        return false;
    }

    left.windows.iter().all(|(window, left_sample)| {
        right.windows.get(window).is_some_and(|right_sample| {
            left_sample.used_percent == right_sample.used_percent
                && left_sample.remaining_percent == right_sample.remaining_percent
                && left_sample.resets_at == right_sample.resets_at
        })
    })
}

fn trend_output_windows(observation: &TrendObservation, window_minutes: Option<i64>) -> Vec<i64> {
    match window_minutes {
        Some(window) => vec![window],
        None => observation.windows.keys().copied().collect(),
    }
}

fn compare_sample_order(left: &RateLimitSample, right: &RateLimitSample) -> std::cmp::Ordering {
    left.timestamp
        .cmp(&right.timestamp)
        .then_with(|| left.resets_at.cmp(&right.resets_at))
        .then_with(|| left.window_minutes.cmp(&right.window_minutes))
        .then_with(|| left.limit_id.cmp(&right.limit_id))
        .then_with(|| left.session_id.cmp(&right.session_id))
}

fn sample_identity(sample: &RateLimitSample) -> SampleIdentity {
    SampleIdentity {
        partition: partition_key(sample),
        timestamp: sample.timestamp,
        resets_at: sample.resets_at,
        window_minutes: sample.window_minutes,
        limit_id: sample.limit_id.clone(),
    }
}

fn trend_partition_key(sample: &RateLimitSample) -> TrendPartitionKey {
    TrendPartitionKey {
        account_id: sample
            .account_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_ACCOUNT.to_string()),
        plan_type: sample
            .plan_type
            .clone()
            .unwrap_or_else(|| UNKNOWN_PLAN.to_string()),
        limit_id: sample
            .limit_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_LIMIT.to_string()),
        window_minutes: sample.window_minutes,
    }
}

fn trend_stream_key(sample: &RateLimitSample) -> TrendStreamKey {
    TrendStreamKey {
        account_id: sample
            .account_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_ACCOUNT.to_string()),
        plan_type: sample
            .plan_type
            .clone()
            .unwrap_or_else(|| UNKNOWN_PLAN.to_string()),
        limit_id: sample
            .limit_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_LIMIT.to_string()),
    }
}

fn partition_key(sample: &RateLimitSample) -> PartitionKey {
    PartitionKey {
        account_id: sample
            .account_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_ACCOUNT.to_string()),
        plan_type: sample
            .plan_type
            .clone()
            .unwrap_or_else(|| UNKNOWN_PLAN.to_string()),
        limit_id: sample
            .limit_id
            .clone()
            .unwrap_or_else(|| UNKNOWN_LIMIT.to_string()),
        window_minutes: sample.window_minutes,
    }
}

fn output_account_id(partition: &PartitionKey) -> Option<String> {
    (partition.account_id != UNKNOWN_ACCOUNT).then(|| partition.account_id.clone())
}

fn output_plan_type(partition: &PartitionKey) -> Option<String> {
    (partition.plan_type != UNKNOWN_PLAN).then(|| partition.plan_type.clone())
}

fn output_limit_id(partition: &PartitionKey) -> Option<String> {
    (partition.limit_id != UNKNOWN_LIMIT).then(|| partition.limit_id.clone())
}

fn output_trend_account_id(partition: &TrendPartitionKey) -> Option<String> {
    (partition.account_id != UNKNOWN_ACCOUNT).then(|| partition.account_id.clone())
}

fn output_trend_plan_type(partition: &TrendPartitionKey) -> Option<String> {
    (partition.plan_type != UNKNOWN_PLAN).then(|| partition.plan_type.clone())
}

fn output_trend_limit_id(partition: &TrendPartitionKey) -> Option<String> {
    (partition.limit_id != UNKNOWN_LIMIT).then(|| partition.limit_id.clone())
}

fn trend_change_from_sample(
    sample: &RateLimitSample,
    delta_used_percent: Option<f64>,
    kind: &str,
) -> LimitTrendChange {
    let partition = trend_partition_key(sample);
    LimitTrendChange {
        at: sample.timestamp,
        account_id: output_trend_account_id(&partition),
        plan_type: output_trend_plan_type(&partition),
        limit_id: output_trend_limit_id(&partition),
        window: sample.window.clone(),
        window_minutes: sample.window_minutes,
        used_percent: sample.used_percent,
        remaining_percent: sample.remaining_percent,
        delta_used_percent,
        resets_at: sample.resets_at,
        kind: kind.to_string(),
    }
}

fn trend_window_change_kind(
    previous: &RateLimitSample,
    next: &RateLimitSample,
) -> Option<&'static str> {
    let used_delta = next.used_percent - previous.used_percent;
    if is_significant_reset_change(previous, next) {
        Some(TREND_KIND_RESET_CHANGED)
    } else if used_delta > PERCENT_EPSILON {
        Some(TREND_KIND_INCREASED)
    } else {
        None
    }
}

fn is_active_trend_sample(sample: &RateLimitSample) -> bool {
    sample.resets_at > sample.timestamp
}

fn is_significant_reset_change(previous: &RateLimitSample, next: &RateLimitSample) -> bool {
    previous.resets_at != next.resets_at && !is_reset_equal_for_trend(previous, next)
}

fn is_reset_equal_for_trend(previous: &RateLimitSample, next: &RateLimitSample) -> bool {
    is_reset_time_equal_within_jitter(previous.resets_at, next.resets_at)
}

fn is_reset_time_equal_within_jitter(left: DateTime<Utc>, right: DateTime<Utc>) -> bool {
    (right - left).num_seconds().abs() <= RESET_JITTER_TOLERANCE_SECONDS
}

fn is_reset_transition(previous: &RateLimitSample, next: &RateLimitSample) -> bool {
    next.resets_at > next.timestamp
        && previous.resets_at != next.resets_at
        && !is_reset_equal_for_trend(previous, next)
        && next.used_percent < previous.used_percent
}

fn transition_kind(previous: &RateLimitSample, next: &RateLimitSample) -> &'static str {
    if is_reset_transition(previous, next) {
        if next.timestamp < previous.resets_at {
            RESET_KIND_EARLY
        } else {
            RESET_KIND_NORMAL
        }
    } else if previous.resets_at != next.resets_at {
        RESET_KIND_CHANGED
    } else {
        RESET_KIND_FIRST_OBSERVED
    }
}

fn limit_window_id(partition: &PartitionKey, reset_at: DateTime<Utc>, window: &str) -> String {
    format!(
        "{}-{}-{}-{}-reset-{}",
        sanitize_id_part(window),
        sanitize_id_part(&partition.account_id),
        sanitize_id_part(&partition.plan_type),
        sanitize_id_part(&partition.limit_id),
        reset_at.timestamp()
    )
}

fn sanitize_id_part(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn status_for_count(count: usize) -> String {
    if count == 0 {
        "unobserved".to_string()
    } else {
        "ok".to_string()
    }
}

fn status_for_current(current: &[LimitCurrentWindow]) -> String {
    if current.is_empty() {
        "unobserved".to_string()
    } else if current
        .iter()
        .any(|window| window.status == CURRENT_STATUS_ACTIVE)
    {
        "ok".to_string()
    } else {
        CURRENT_STATUS_EXPIRED.to_string()
    }
}

fn count_unknown_limit_samples(samples: &[RateLimitSample]) -> i64 {
    samples
        .iter()
        .filter(|sample| sample.limit_id.is_none())
        .count() as i64
}

fn count_unknown_limit_reset_events(resets: &[LimitResetEvent]) -> i64 {
    resets
        .iter()
        .filter(|reset| reset.limit_id.is_none())
        .count() as i64
}

fn diagnostics_for_options(
    input: &RateLimitSamplesReport,
    duplicate_samples: i64,
    ignored_inactive_stream_samples: i64,
    unknown_limit_reset_events: i64,
    options: LimitReportOptions,
) -> Option<LimitReportDiagnostics> {
    if !options.include_diagnostics {
        return None;
    }

    let mut scan = input.diagnostics.clone();
    let source_evidence = if options.include_source_evidence {
        scan.source_spans
            .iter()
            .map(|source| LimitSourceEvidence {
                path: source.path.clone(),
                line_number: source.line_number,
            })
            .collect()
    } else {
        Vec::new()
    };
    scan.source_spans.clear();

    Some(LimitReportDiagnostics {
        scan,
        duplicate_samples,
        unknown_limit_samples: count_unknown_limit_samples(&input.samples),
        unknown_limit_reset_events,
        ignored_inactive_stream_samples,
        source_evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::read_rate_limit_samples_report;
    use chrono::TimeZone;
    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn builds_windows_without_crossing_account_plan_or_window_partitions() {
        let input = fixture_samples_report();

        let report = build_limit_windows_report(&input, LimitReportOptions::default());

        assert_eq!(report.status, "ok");
        assert_eq!(report.windows.len(), 9);
        assert!(report
            .windows
            .windows(2)
            .all(|pair| pair[0].reset_at <= pair[1].reset_at));

        let first_primary = report
            .windows
            .iter()
            .find(|window| {
                window.window_minutes == 300
                    && window.account_id.as_deref() == Some("account-fixture")
                    && window.plan_type.as_deref() == Some("pro")
                    && window.reset_at == utc_time(2026, 5, 10, 14, 0)
            })
            .expect("first primary window");
        assert_eq!(first_primary.window, "5h");
        assert_eq!(first_primary.estimated_start, utc_time(2026, 5, 10, 9, 0));
        assert_eq!(first_primary.sample_count, 1);
        assert_eq!(first_primary.reset_kind, RESET_KIND_FIRST_OBSERVED);

        let weekly_early = report
            .windows
            .iter()
            .find(|window| {
                window.window_minutes == 10080
                    && window.account_id.as_deref() == Some("account-fixture")
                    && window.plan_type.as_deref() == Some("pro")
                    && window.reset_at == utc_time(2026, 5, 19, 9, 0)
            })
            .expect("early weekly window");
        assert_eq!(weekly_early.reset_kind, RESET_KIND_EARLY);
        assert_eq!(weekly_early.min_used_percent, 4.0);
        assert_eq!(weekly_early.max_used_percent, 4.0);

        let plus_window = report
            .windows
            .iter()
            .find(|window| window.account_id.as_deref() == Some("account-other"))
            .expect("plus account window");
        assert_eq!(plus_window.plan_type.as_deref(), Some("plus"));
        assert_eq!(plus_window.window_minutes, 300);
    }

    #[test]
    fn windows_merge_reset_jitter_into_one_logical_window() {
        let first_reset = utc_time(2026, 5, 12, 17, 0);
        let next_reset = utc_time(2026, 5, 12, 18, 0);
        let input = RateLimitSamplesReport {
            start: utc_time(2026, 5, 12, 12, 0),
            end: utc_time(2026, 5, 12, 13, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                trend_sample(0.0, first_reset, 0),
                trend_sample(1.0, first_reset + Duration::seconds(1), 1),
                trend_sample(0.0, first_reset, 2),
                trend_sample(2.0, first_reset + Duration::seconds(30), 3),
                trend_sample(0.0, next_reset, 4),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        };

        let report = build_limit_windows_report(&input, LimitReportOptions::default());

        assert_eq!(report.windows.len(), 2);
        let jittered = &report.windows[0];
        assert_eq!(jittered.reset_at, first_reset + Duration::seconds(30));
        assert_eq!(jittered.sample_count, 4);
        assert_eq!(jittered.min_used_percent, 0.0);
        assert_eq!(jittered.max_used_percent, 2.0);
        assert_eq!(jittered.last_used_percent, 2.0);
        assert_eq!(jittered.reset_kind, RESET_KIND_FIRST_OBSERVED);
        assert_eq!(report.windows[1].reset_kind, RESET_KIND_EARLY);
    }

    #[test]
    fn derived_reports_ignore_inactive_rolling_zero_streams() {
        let first_seen = utc_time(2026, 5, 5, 13, 0);
        let window_minutes = 10_080;
        let codex_reset = utc_time(2026, 5, 12, 7, 15);
        let rolling_reset = |offset_minutes: i64, jitter_seconds: i64| {
            first_seen
                + Duration::minutes(offset_minutes)
                + Duration::minutes(window_minutes)
                + Duration::seconds(jitter_seconds)
        };
        let input = RateLimitSamplesReport {
            start: first_seen,
            end: first_seen + Duration::minutes(10),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                report_sample("codex", first_seen, 4.0, codex_reset, window_minutes),
                report_sample(
                    "codex",
                    first_seen + Duration::minutes(4),
                    4.0,
                    codex_reset,
                    window_minutes,
                ),
                report_sample(
                    "codex_bengalfox",
                    first_seen,
                    0.0,
                    rolling_reset(0, -3),
                    window_minutes,
                ),
                report_sample(
                    "codex_bengalfox",
                    first_seen + Duration::minutes(2),
                    0.0,
                    rolling_reset(2, -1),
                    window_minutes,
                ),
                report_sample(
                    "codex_bengalfox",
                    first_seen + Duration::minutes(4),
                    0.0,
                    rolling_reset(4, 0),
                    window_minutes,
                ),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        };

        let samples = build_limit_samples_report(&input, LimitReportOptions::default());
        assert!(samples
            .samples
            .iter()
            .any(|sample| sample.limit_id.as_deref() == Some("codex_bengalfox")));

        let windows = build_limit_windows_report(
            &input,
            LimitReportOptions {
                include_diagnostics: true,
                include_source_evidence: false,
            },
        );
        assert_eq!(windows.windows.len(), 1);
        assert_eq!(windows.windows[0].limit_id.as_deref(), Some("codex"));
        assert_eq!(
            windows
                .diagnostics
                .as_ref()
                .expect("windows diagnostics")
                .ignored_inactive_stream_samples,
            3
        );

        let trend =
            build_limit_trend_report(&input, Some(window_minutes), LimitReportOptions::default());
        assert_eq!(trend.changes.len(), 1);
        assert_eq!(trend.changes[0].limit_id.as_deref(), Some("codex"));

        let current = build_limit_current_report(
            &input,
            first_seen + Duration::minutes(5),
            LimitReportOptions::default(),
        );
        assert_eq!(current.current.len(), 1);
        assert_eq!(current.current[0].limit_id.as_deref(), Some("codex"));
    }

    #[test]
    fn detects_normal_and_early_resets_within_each_partition() {
        let input = fixture_samples_report();

        let report = build_limit_resets_report(&input, false, LimitReportOptions::default());

        assert_eq!(report.status, "ok");
        assert_eq!(report.resets.len(), 4);
        assert!(report
            .resets
            .iter()
            .all(|reset| reset.account_id.as_deref() == Some("account-fixture")));
        assert!(report
            .resets
            .iter()
            .any(|reset| reset.window == "7d" && reset.kind == RESET_KIND_NORMAL));

        let early_weekly = report
            .resets
            .iter()
            .find(|reset| reset.window == "7d" && reset.kind == RESET_KIND_EARLY)
            .expect("early weekly reset");
        assert_eq!(early_weekly.at, utc_time(2026, 5, 12, 12, 0));
        assert_eq!(early_weekly.previous_used_percent, 91.0);
        assert_eq!(early_weekly.next_used_percent, 4.0);
        assert_eq!(early_weekly.previous_resets_at, utc_time(2026, 5, 18, 9, 0));
        assert_eq!(early_weekly.next_resets_at, utc_time(2026, 5, 19, 9, 0));
        assert_eq!(early_weekly.early_by_seconds, 507_600);

        let early_only = build_limit_resets_report(&input, true, LimitReportOptions::default());
        assert_eq!(early_only.resets.len(), 2);
        assert!(early_only
            .resets
            .iter()
            .all(|reset| reset.kind == RESET_KIND_EARLY));
    }

    #[test]
    fn resets_do_not_cross_limit_id_streams() {
        let input = RateLimitSamplesReport {
            start: utc_time(2026, 5, 12, 0, 0),
            end: utc_time(2026, 5, 12, 2, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                reset_sample("limit-alpha", 0, 80.0, utc_time(2026, 5, 19, 0, 0)),
                reset_sample("limit-beta", 1, 4.0, utc_time(2026, 5, 20, 0, 0)),
                reset_sample("limit-alpha", 2, 82.0, utc_time(2026, 5, 19, 0, 0)),
                reset_sample("limit-alpha", 3, 2.0, utc_time(2026, 5, 20, 0, 0)),
                reset_sample("limit-beta", 4, 5.0, utc_time(2026, 5, 20, 0, 0)),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        };

        let report = build_limit_resets_report(&input, false, LimitReportOptions::default());

        assert_eq!(report.resets.len(), 1);
        let reset = &report.resets[0];
        assert_eq!(reset.limit_id.as_deref(), Some("limit-alpha"));
        assert_eq!(reset.previous_used_percent, 82.0);
        assert_eq!(reset.next_used_percent, 2.0);
        assert_eq!(reset.previous_resets_at, utc_time(2026, 5, 19, 0, 0));
        assert_eq!(reset.next_resets_at, utc_time(2026, 5, 20, 0, 0));
    }

    #[test]
    fn reset_diagnostics_count_unknown_limit_risk() {
        let input = RateLimitSamplesReport {
            start: utc_time(2026, 5, 12, 0, 0),
            end: utc_time(2026, 5, 12, 2, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                unknown_limit_reset_sample(0, 40.0, utc_time(2026, 5, 19, 0, 0)),
                unknown_limit_reset_sample(1, 4.0, utc_time(2026, 5, 20, 0, 0)),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        };

        let report = build_limit_resets_report(
            &input,
            false,
            LimitReportOptions {
                include_diagnostics: true,
                include_source_evidence: false,
            },
        );
        let diagnostics = report.diagnostics.expect("diagnostics");

        assert_eq!(report.resets.len(), 1);
        assert_eq!(report.resets[0].limit_id, None);
        assert_eq!(diagnostics.unknown_limit_samples, 2);
        assert_eq!(diagnostics.unknown_limit_reset_events, 1);
    }

    #[test]
    fn builds_trend_change_points_and_compresses_duplicates() {
        let input = trend_samples_report();

        let report = build_limit_trend_report(&input, Some(300), LimitReportOptions::default());

        assert_eq!(report.status, "ok");
        assert_eq!(report.changes.len(), 3);
        assert!(report
            .changes
            .iter()
            .all(|change| change.used_percent != 24.0));
        assert!(report
            .changes
            .iter()
            .all(|change| change.resets_at != utc_time(2026, 5, 12, 17, 0) + Duration::seconds(1)));
        assert_eq!(report.changes[0].kind, RESET_KIND_FIRST_OBSERVED);
        assert_eq!(report.changes[0].used_percent, 20.0);
        assert_eq!(report.changes[0].delta_used_percent, None);
        assert_eq!(report.changes[1].kind, TREND_KIND_INCREASED);
        assert_eq!(report.changes[1].used_percent, 25.0);
        assert_eq!(report.changes[1].delta_used_percent, Some(5.0));
        assert_eq!(report.changes[2].kind, TREND_KIND_RESET_CHANGED);
        assert_eq!(report.changes[2].used_percent, 15.0);
        assert_eq!(report.changes[2].delta_used_percent, Some(-10.0));
        assert_eq!(report.changes[2].resets_at, utc_time(2026, 5, 12, 18, 0));
        assert!(report
            .changes
            .iter()
            .all(|change| change.limit_id.as_deref() == Some("fixture-trend-change")));
    }

    #[test]
    fn trend_uses_monotonic_window_progress_across_parallel_sources() {
        let reset_zero = utc_time(2026, 5, 18, 5, 12);
        let reset_progress = reset_zero + Duration::seconds(11);
        let expired_reset = utc_time(2026, 5, 17, 18, 30);
        let input = RateLimitSamplesReport {
            start: utc_time(2026, 5, 18, 0, 0),
            end: utc_time(2026, 5, 18, 1, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                trend_window_sample("stale", 1, 0, 0.0, reset_zero, 300),
                trend_window_sample("progress", 1, 1, 1.0, reset_progress, 300),
                trend_window_sample("stale", 2, 2, 0.0, reset_zero, 300),
                trend_window_sample("progress", 2, 3, 2.0, reset_progress, 300),
                trend_window_sample("expired", 1, 4, 18.0, expired_reset, 300),
                trend_window_sample("stale", 3, 5, 0.0, reset_zero, 300),
                trend_window_sample("progress", 3, 6, 4.0, reset_progress, 300),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        };

        let report = build_limit_trend_report(&input, Some(300), LimitReportOptions::default());

        assert_eq!(report.changes.len(), 4);
        assert_eq!(
            report
                .changes
                .iter()
                .map(|change| change.used_percent)
                .collect::<Vec<_>>(),
            vec![0.0, 1.0, 2.0, 4.0]
        );
        assert!(report
            .changes
            .iter()
            .all(|change| change.kind != "decreased"));
        assert!(report
            .changes
            .iter()
            .all(|change| change.resets_at != expired_reset));
    }

    #[test]
    fn trend_selected_window_omits_unchanged_sibling_points() {
        let five_hour_reset = utc_time(2026, 5, 18, 5, 0);
        let weekly_reset = utc_time(2026, 5, 25, 5, 0);
        let input = RateLimitSamplesReport {
            start: utc_time(2026, 5, 18, 0, 0),
            end: utc_time(2026, 5, 18, 1, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                trend_window_sample("vector", 1, 0, 0.0, five_hour_reset, 300),
                trend_window_sample("vector", 1, 0, 10.0, weekly_reset, 10_080),
                trend_window_sample("vector", 2, 1, 1.0, five_hour_reset, 300),
                trend_window_sample("vector", 2, 1, 10.0, weekly_reset, 10_080),
                trend_window_sample("vector", 3, 2, 2.0, five_hour_reset, 300),
                trend_window_sample("vector", 3, 2, 11.0, weekly_reset, 10_080),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        };

        let report = build_limit_trend_report(&input, Some(10_080), LimitReportOptions::default());

        assert_eq!(report.changes.len(), 2);
        assert!(report.changes.iter().all(|change| change.window == "7d"));
        assert_eq!(report.changes[0].kind, RESET_KIND_FIRST_OBSERVED);
        assert_eq!(report.changes[0].used_percent, 10.0);
        assert_eq!(report.changes[1].kind, TREND_KIND_INCREASED);
        assert_eq!(report.changes[1].used_percent, 11.0);
    }

    #[test]
    fn builds_current_report_and_unobserved_status() {
        let input = fixture_samples_report();

        let report = build_limit_current_report(
            &input,
            utc_time(2026, 5, 12, 13, 10),
            LimitReportOptions::default(),
        );

        assert_eq!(report.status, "ok");
        assert_eq!(report.current.len(), 3);
        let current_weekly = report
            .current
            .iter()
            .find(|current| {
                current.window == "7d" && current.resets_at == Some(utc_time(2026, 5, 19, 9, 0))
            })
            .expect("current weekly");
        assert_eq!(current_weekly.status, CURRENT_STATUS_ACTIVE);
        assert_eq!(current_weekly.used_percent, Some(4.0));
        assert_eq!(current_weekly.remaining_percent, Some(96.0));
        assert_eq!(current_weekly.reset_in_seconds, Some(589_800));

        let empty_input = RateLimitSamplesReport {
            start: utc_time(2026, 5, 1, 0, 0),
            end: utc_time(2026, 5, 1, 1, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: Vec::new(),
            diagnostics: RateLimitDiagnostics::default(),
        };
        let empty = build_limit_current_report(
            &empty_input,
            utc_time(2026, 5, 1, 1, 0),
            LimitReportOptions::default(),
        );
        assert_eq!(empty.status, "unobserved");
        assert!(empty.current.is_empty());
    }

    #[test]
    fn current_report_shows_last_expired_window_when_no_cycle_is_active() {
        let input = fixture_samples_report();

        let report = build_limit_current_report(
            &input,
            utc_time(2026, 5, 20, 0, 0),
            LimitReportOptions::default(),
        );

        assert_eq!(report.status, CURRENT_STATUS_EXPIRED);
        assert_eq!(report.current.len(), 3);
        assert!(report
            .current
            .iter()
            .all(|window| window.status == CURRENT_STATUS_EXPIRED));
        let expired_weekly = report
            .current
            .iter()
            .find(|current| current.window == "7d")
            .expect("expired weekly");
        assert_eq!(expired_weekly.resets_at, Some(utc_time(2026, 5, 19, 9, 0)));
        assert_eq!(expired_weekly.last_seen, Some(utc_time(2026, 5, 12, 12, 0)));
        assert_eq!(expired_weekly.used_percent, Some(4.0));
        assert_eq!(expired_weekly.reset_in_seconds, None);
    }

    #[test]
    fn samples_report_hides_source_by_default_and_exposes_it_for_verbose_diagnostics() {
        let input = fixture_samples_report();

        let default_report = build_limit_samples_report(&input, LimitReportOptions::default());
        let default_value = serde_json::to_value(&default_report).expect("default json");
        assert_no_source_evidence(&default_value);

        let verbose_report = build_limit_samples_report(
            &input,
            LimitReportOptions {
                include_diagnostics: true,
                include_source_evidence: true,
            },
        );
        let verbose_value = serde_json::to_value(&verbose_report).expect("verbose json");
        let evidence = verbose_value["diagnostics"]["sourceEvidence"]
            .as_array()
            .expect("source evidence");
        assert_eq!(evidence.len(), input.samples.len());
        assert!(evidence[0]["path"]
            .as_str()
            .expect("path")
            .contains("sessions"));
        assert!(evidence[0]["lineNumber"].as_u64().expect("line number") > 0);
    }

    #[test]
    fn duplicate_samples_are_counted_without_changing_window_semantics() {
        let mut input = fixture_samples_report();
        input
            .samples
            .push(input.samples.first().expect("sample").clone());

        let report = build_limit_windows_report(
            &input,
            LimitReportOptions {
                include_diagnostics: true,
                include_source_evidence: false,
            },
        );

        assert_eq!(report.windows.len(), 9);
        assert_eq!(
            report
                .diagnostics
                .as_ref()
                .expect("diagnostics")
                .duplicate_samples,
            1
        );
    }

    #[test]
    fn usage_attachment_prefers_same_line_rate_limit_identity() {
        let reset_at = utc_time(2026, 5, 19, 0, 0);
        let mut windows = vec![
            usage_window("limit-alpha", reset_at),
            usage_window("limit-beta", reset_at),
        ];
        let records = vec![usage_record_with_rate_limit("limit-beta", reset_at)];

        attach_usage_to_limit_windows(&mut windows, &records);

        assert_eq!(windows[0].total_tokens, 0);
        assert_eq!(windows[1].total_tokens, 42);
    }

    #[test]
    fn usage_attachment_skips_ambiguous_legacy_records() {
        let reset_at = utc_time(2026, 5, 19, 0, 0);
        let mut windows = vec![
            usage_window("limit-alpha", reset_at),
            usage_window("limit-beta", reset_at),
        ];
        let records = vec![usage_record_without_rate_limits()];

        attach_usage_to_limit_windows(&mut windows, &records);

        assert!(windows.iter().all(|window| window.total_tokens == 0));
    }

    #[test]
    fn usage_totals_price_records_across_rate_card_cutoff() {
        let cutoff = DateTime::parse_from_rfc3339("2026-07-30T17:17:05.167Z")
            .expect("cutoff")
            .with_timezone(&Utc);
        let mut old = usage_record_without_rate_limits();
        old.timestamp = cutoff - Duration::milliseconds(1);
        old.model = "gpt-5.6-terra".to_string();
        old.account_id = Some("account-fixture".to_string());
        old.usage.input_tokens = 1_000_000;
        old.usage.output_tokens = 0;
        old.usage.total_tokens = 1_000_000;

        let mut new_fast = old.clone();
        new_fast.timestamp = cutoff;
        new_fast.session_id = "usage-session-fast".to_string();
        new_fast.usage_mode = crate::stats::UsageMode::Fast;

        let reset_at = cutoff + Duration::hours(1);
        let targets = vec![UsageWindowTarget {
            start: cutoff - Duration::seconds(1),
            end: reset_at,
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: Some("cutoff".to_string()),
            window_minutes: 300,
            reset_at,
        }];

        let totals = usage_totals_for_targets(&[old, new_fast], &targets);

        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].total_tokens, 2_000_000);
        assert_eq!(totals[0].credits, 187.5);
    }

    #[test]
    fn report_json_contains_expected_schema_keys() {
        let input = fixture_samples_report();
        let windows = serde_json::to_value(build_limit_windows_report(
            &input,
            LimitReportOptions::default(),
        ))
        .expect("windows json");
        assert_has_keys(
            &windows["windows"][0],
            &[
                "id",
                "accountId",
                "planType",
                "limitId",
                "window",
                "windowMinutes",
                "estimatedStart",
                "resetAt",
                "firstSeen",
                "lastSeen",
                "minUsedPercent",
                "maxUsedPercent",
                "lastUsedPercent",
                "sampleCount",
                "resetKind",
            ],
        );

        let resets = serde_json::to_value(build_limit_resets_report(
            &input,
            false,
            LimitReportOptions::default(),
        ))
        .expect("resets json");
        assert_has_keys(
            &resets["resets"][0],
            &[
                "at",
                "accountId",
                "planType",
                "limitId",
                "window",
                "previousUsedPercent",
                "nextUsedPercent",
                "previousResetsAt",
                "nextResetsAt",
                "earlyBySeconds",
                "kind",
            ],
        );

        let trend = serde_json::to_value(build_limit_trend_report(
            &input,
            None,
            LimitReportOptions::default(),
        ))
        .expect("trend json");
        assert_has_keys(
            &trend["changes"][0],
            &[
                "at",
                "window",
                "windowMinutes",
                "accountId",
                "planType",
                "limitId",
                "usedPercent",
                "remainingPercent",
                "deltaUsedPercent",
                "resetsAt",
                "kind",
            ],
        );

        let current = serde_json::to_value(build_limit_current_report(
            &input,
            utc_time(2026, 5, 12, 13, 10),
            LimitReportOptions::default(),
        ))
        .expect("current json");
        assert_has_keys(
            &current["current"][0],
            &[
                "id",
                "status",
                "accountId",
                "planType",
                "limitId",
                "window",
                "windowMinutes",
                "lastSeen",
                "usedPercent",
                "remainingPercent",
                "resetsAt",
                "resetInSeconds",
            ],
        );
    }

    fn fixture_samples_report() -> RateLimitSamplesReport {
        let codex_home =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/rust-run/codex-home");
        read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 10, 0, 0),
            end: utc_time(2026, 5, 12, 14, 0),
            sessions_dir: codex_home.join("sessions"),
            scan_all_files: true,
            account_history_file: Some(codex_home.join("codex-ops/auth-account-history.json")),
            account_id: None,
            plan_type: None,
            window_minutes: None,
        })
        .expect("fixture samples")
    }

    fn trend_samples_report() -> RateLimitSamplesReport {
        let first_reset = utc_time(2026, 5, 12, 17, 0);
        let next_reset = utc_time(2026, 5, 12, 18, 0);
        RateLimitSamplesReport {
            start: utc_time(2026, 5, 12, 12, 0),
            end: utc_time(2026, 5, 12, 13, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![
                trend_sample(20.0, first_reset, 0),
                trend_sample(20.0, first_reset, 1),
                trend_sample(20.0, first_reset + Duration::seconds(1), 2),
                trend_sample(25.0, first_reset, 3),
                trend_sample(24.0, first_reset, 4),
                trend_sample(15.0, first_reset, 5),
                trend_sample(15.0, first_reset + Duration::seconds(1), 6),
                trend_sample(15.0, next_reset, 7),
            ],
            diagnostics: RateLimitDiagnostics::default(),
        }
    }

    fn report_sample(
        limit_id: &str,
        timestamp: DateTime<Utc>,
        used_percent: f64,
        resets_at: DateTime<Utc>,
        window_minutes: i64,
    ) -> RateLimitSample {
        let window = match window_minutes {
            300 => "5h",
            10_080 => "7d",
            _ => "primary",
        };
        RateLimitSample {
            timestamp,
            session_id: format!("report-session-{}", timestamp.timestamp_millis()),
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: Some(limit_id.to_string()),
            window: window.to_string(),
            window_minutes,
            used_percent,
            remaining_percent: 100.0 - used_percent,
            resets_at,
            source: None,
        }
    }

    fn reset_sample(
        limit_id: &str,
        minute_offset: i64,
        used_percent: f64,
        resets_at: DateTime<Utc>,
    ) -> RateLimitSample {
        let mut sample = unknown_limit_reset_sample(minute_offset, used_percent, resets_at);
        sample.limit_id = Some(limit_id.to_string());
        sample
    }

    fn unknown_limit_reset_sample(
        minute_offset: i64,
        used_percent: f64,
        resets_at: DateTime<Utc>,
    ) -> RateLimitSample {
        RateLimitSample {
            timestamp: utc_time(2026, 5, 12, minute_offset as u32, 0),
            session_id: format!("reset-session-{minute_offset}"),
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: None,
            window: "7d".to_string(),
            window_minutes: 10_080,
            used_percent,
            remaining_percent: 100.0 - used_percent,
            resets_at,
            source: None,
        }
    }

    fn trend_sample(
        used_percent: f64,
        resets_at: DateTime<Utc>,
        minute_offset: i64,
    ) -> RateLimitSample {
        RateLimitSample {
            timestamp: utc_time(2026, 5, 12, 12, minute_offset as u32),
            session_id: format!("trend-session-{minute_offset}"),
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: Some("fixture-trend-change".to_string()),
            window: "5h".to_string(),
            window_minutes: 300,
            used_percent,
            remaining_percent: 100.0 - used_percent,
            resets_at,
            source: None,
        }
    }

    fn trend_window_sample(
        source: &str,
        source_line: usize,
        minute_offset: i64,
        used_percent: f64,
        resets_at: DateTime<Utc>,
        window_minutes: i64,
    ) -> RateLimitSample {
        let window = match window_minutes {
            300 => "5h",
            10_080 => "7d",
            _ => "primary",
        };
        RateLimitSample {
            timestamp: utc_time(2026, 5, 18, 0, minute_offset as u32),
            session_id: source.to_string(),
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: Some("fixture-trend-vector".to_string()),
            window: window.to_string(),
            window_minutes,
            used_percent,
            remaining_percent: 100.0 - used_percent,
            resets_at,
            source: Some(SourceSpan {
                path: format!("/tmp/{source}.jsonl"),
                line_number: source_line,
            }),
        }
    }

    fn usage_window(limit_id: &str, reset_at: DateTime<Utc>) -> LimitWindow {
        LimitWindow {
            id: format!("{limit_id}-window"),
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: Some(limit_id.to_string()),
            window: "7d".to_string(),
            window_minutes: 10_080,
            estimated_start: utc_time(2026, 5, 12, 0, 0),
            reset_at,
            first_seen: utc_time(2026, 5, 12, 0, 0),
            last_seen: utc_time(2026, 5, 12, 0, 1),
            min_used_percent: 1.0,
            max_used_percent: 1.0,
            last_used_percent: 1.0,
            sample_count: 1,
            reset_kind: RESET_KIND_FIRST_OBSERVED.to_string(),
            total_tokens: 0,
            credits: 0.0,
            usd: 0.0,
        }
    }

    fn usage_record_with_rate_limit(limit_id: &str, reset_at: DateTime<Utc>) -> UsageRecord {
        let mut record = usage_record_without_rate_limits();
        record.rate_limits = vec![UsageRateLimit {
            plan_type: Some("pro".to_string()),
            limit_id: Some(limit_id.to_string()),
            window: "7d".to_string(),
            window_minutes: 10_080,
            resets_at: reset_at + Duration::seconds(1),
        }];
        record
    }

    fn usage_record_without_rate_limits() -> UsageRecord {
        UsageRecord {
            timestamp: utc_time(2026, 5, 12, 0, 30),
            session_id: "usage-session".to_string(),
            model: "gpt-5.5".to_string(),
            usage_mode: crate::stats::UsageMode::Normal,
            reasoning_effort: None,
            cwd: "/workspace/usage".to_string(),
            account_id: Some("account-fixture".to_string()),
            file_path: "/tmp/usage.jsonl".to_string(),
            rate_limits: Vec::new(),
            usage: crate::stats::TokenUsage {
                input_tokens: 40,
                cached_input_tokens: 0,
                output_tokens: 2,
                reasoning_output_tokens: 0,
                total_tokens: 42,
            },
        }
    }

    fn utc_time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid UTC time")
    }

    fn assert_has_keys(value: &Value, keys: &[&str]) {
        let object = value.as_object().expect("json object");
        for key in keys {
            assert!(object.contains_key(*key), "missing key {key}");
        }
    }

    fn assert_no_source_evidence(value: &Value) {
        match value {
            Value::Object(object) => {
                for key in object.keys() {
                    assert!(
                        !matches!(
                            key.as_str(),
                            "source"
                                | "sourcePath"
                                | "sourceLine"
                                | "sourceEvidence"
                                | "filePath"
                                | "line"
                                | "lineNumber"
                        ),
                        "default report leaked source key {key}"
                    );
                }
                for child in object.values() {
                    assert_no_source_evidence(child);
                }
            }
            Value::Array(items) => {
                for item in items {
                    assert_no_source_evidence(item);
                }
            }
            _ => {}
        }
    }
}
