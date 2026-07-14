use super::StatSort;
use crate::limits::RateLimitDiagnostics;
use crate::pricing::TokenUsage as PricingTokenUsage;
use crate::session_scan::SessionScanDiagnostics;
use crate::time::{local_to_utc, StatGroupBy};
use chrono::{DateTime, Datelike, Local, SecondsFormat, Timelike, Utc};
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct UsageRecordsReadOptions {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: std::path::PathBuf,
    pub scan_all_files: bool,
    pub account_history_file: Option<std::path::PathBuf>,
    pub usage_mode_history_file: Option<std::path::PathBuf>,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UsageRecordsReport {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sessions_dir: String,
    pub records: Vec<UsageRecord>,
    pub diagnostics: UsageDiagnostics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LimitUsageGroupBy {
    Window,
    Model,
    Cwd,
    Account,
}

impl LimitUsageGroupBy {
    pub(super) fn from_stat(value: Option<StatGroupBy>) -> Self {
        match value {
            Some(StatGroupBy::Model) => Self::Model,
            Some(StatGroupBy::Cwd) => Self::Cwd,
            Some(StatGroupBy::Account) => Self::Account,
            _ => Self::Window,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Model => "model",
            Self::Cwd => "cwd",
            Self::Account => "account",
        }
    }

    pub(super) fn as_stat(self) -> Option<StatGroupBy> {
        match self {
            Self::Window => None,
            Self::Model => Some(StatGroupBy::Model),
            Self::Cwd => Some(StatGroupBy::Cwd),
            Self::Account => Some(StatGroupBy::Account),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl TokenUsage {
    pub(super) fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
        self.total_tokens += other.total_tokens;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.cached_input_tokens == 0
            && self.output_tokens == 0
            && self.reasoning_output_tokens == 0
            && self.total_tokens == 0
    }

    pub(super) fn pricing_usage(&self) -> PricingTokenUsage {
        PricingTokenUsage {
            input_tokens: self.input_tokens.max(0) as u64,
            cached_input_tokens: self.cached_input_tokens.max(0) as u64,
            output_tokens: self.output_tokens.max(0) as u64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UsageRecord {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub model: String,
    pub usage_mode: UsageMode,
    pub reasoning_effort: Option<String>,
    pub cwd: String,
    pub account_id: Option<String>,
    pub file_path: String,
    pub rate_limits: Vec<UsageRateLimit>,
    pub usage: TokenUsage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageRateLimit {
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub window: String,
    pub window_minutes: i64,
    pub resets_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageMode {
    Unknown,
    Normal,
    Fast,
}

impl UsageMode {
    pub(super) fn from_fast(fast: Option<bool>) -> Self {
        match fast {
            Some(true) => Self::Fast,
            Some(false) => Self::Normal,
            None => Self::Unknown,
        }
    }

    pub fn is_fast(self) -> bool {
        self == Self::Fast
    }
}

#[derive(Clone, Copy)]
pub(super) struct UsageRecordView<'a> {
    pub(super) timestamp: DateTime<Utc>,
    pub(super) session_id: &'a str,
    pub(super) model: &'a str,
    pub(super) usage_mode: UsageMode,
    pub(super) reasoning_effort: Option<&'a str>,
    pub(super) cwd: &'a str,
    pub(super) account_id: Option<&'a str>,
    pub(super) file_path: &'a str,
    pub(super) rate_limits: &'a [UsageRateLimit],
    pub(super) usage: &'a TokenUsage,
}

impl UsageRecordView<'_> {
    pub(super) fn to_owned_record(self) -> UsageRecord {
        UsageRecord {
            timestamp: self.timestamp,
            session_id: self.session_id.to_string(),
            model: self.model.to_string(),
            usage_mode: self.usage_mode,
            reasoning_effort: self.reasoning_effort.map(str::to_string),
            cwd: self.cwd.to_string(),
            account_id: self.account_id.map(str::to_string),
            file_path: self.file_path.to_string(),
            rate_limits: self.rate_limits.to_vec(),
            usage: self.usage.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatRow {
    pub(super) key: String,
    pub(super) sessions: usize,
    pub(super) calls: i64,
    pub(super) usage: TokenUsage,
    pub(super) credits: f64,
    pub(super) usd: f64,
    pub(super) priced_calls: i64,
    pub(super) unpriced_calls: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageUnpricedModelRow {
    pub(super) model: String,
    pub(super) pricing_key: String,
    pub(super) calls: i64,
    pub(super) total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
    pub(super) pricing_stub: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageDiagnostics {
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
    pub token_count_events: i64,
    pub included_usage_events: i64,
    pub skipped_events: SkippedEvents,
    pub file_read_concurrency: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_history: Option<UsageModeHistoryDiagnostics>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedEvents {
    pub missing_metadata: i64,
    pub missing_usage: i64,
    pub empty_usage: i64,
    pub out_of_range: i64,
    pub account_mismatch: i64,
    pub fork_replay: i64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageModeHistoryDiagnostics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_file: Option<String>,
    pub history_present: bool,
    pub switch_count: i64,
    pub fast_attributed_calls: i64,
    pub fast_attributed_credits: f64,
}

impl UsageDiagnostics {
    pub(super) fn new(file_read_concurrency: i64, scan_all_files: bool) -> Self {
        Self {
            scan_all_files,
            scanned_directories: 0,
            skipped_directories: 0,
            read_files: 0,
            skipped_files: 0,
            prefiltered_files: 0,
            tail_read_files: 0,
            tail_read_hits: 0,
            mtime_read_files: 0,
            mtime_tail_hits: 0,
            mtime_read_hits: 0,
            fork_files: 0,
            fork_parent_missing: 0,
            fork_replay_lines: 0,
            read_lines: 0,
            invalid_json_lines: 0,
            token_count_events: 0,
            included_usage_events: 0,
            skipped_events: SkippedEvents::default(),
            file_read_concurrency,
            mode_history: None,
        }
    }

    pub(super) fn record_fast_attribution(&mut self, calls: i64, credits: f64) {
        if let Some(mode_history) = &mut self.mode_history {
            mode_history.fast_attributed_calls += calls;
            mode_history.fast_attributed_credits =
                crate::format::round_credits(mode_history.fast_attributed_credits + credits);
        }
    }

    pub(super) fn merge_file_scan(&mut self, other: &UsageDiagnostics) {
        self.prefiltered_files += other.prefiltered_files;
        self.tail_read_files += other.tail_read_files;
        self.tail_read_hits += other.tail_read_hits;
        self.mtime_read_files += other.mtime_read_files;
        self.mtime_tail_hits += other.mtime_tail_hits;
        self.mtime_read_hits += other.mtime_read_hits;
        self.fork_files += other.fork_files;
        self.fork_parent_missing += other.fork_parent_missing;
        self.fork_replay_lines += other.fork_replay_lines;
        self.read_lines += other.read_lines;
        self.invalid_json_lines += other.invalid_json_lines;
        self.token_count_events += other.token_count_events;
        self.included_usage_events += other.included_usage_events;
        self.skipped_events.missing_metadata += other.skipped_events.missing_metadata;
        self.skipped_events.missing_usage += other.skipped_events.missing_usage;
        self.skipped_events.empty_usage += other.skipped_events.empty_usage;
        self.skipped_events.out_of_range += other.skipped_events.out_of_range;
        self.skipped_events.account_mismatch += other.skipped_events.account_mismatch;
        self.skipped_events.fork_replay += other.skipped_events.fork_replay;
    }

    pub(crate) fn merge_session_scan(&mut self, other: &SessionScanDiagnostics) {
        self.scanned_directories += other.scanned_directories;
        self.skipped_directories += other.skipped_directories;
        self.read_files += other.read_files;
        self.skipped_files += other.skipped_files;
        self.prefiltered_files += other.prefiltered_files;
        self.tail_read_files += other.tail_read_files;
        self.tail_read_hits += other.tail_read_hits;
        self.mtime_read_files += other.mtime_read_files;
        self.mtime_tail_hits += other.mtime_tail_hits;
        self.mtime_read_hits += other.mtime_read_hits;
        self.fork_files += other.fork_files;
        self.fork_parent_missing += other.fork_parent_missing;
        self.fork_replay_lines += other.fork_replay_lines;
    }
}

#[derive(Clone, Debug)]
pub(super) struct UsageStatsReport {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) group_by: StatGroupBy,
    pub(super) include_reasoning_effort: bool,
    pub(super) sort_by: Option<StatSort>,
    pub(super) limit: Option<usize>,
    pub(super) sessions_dir: String,
    pub(super) rows: Vec<UsageStatRow>,
    pub(super) totals: UsageStatRow,
    pub(super) unpriced_models: Vec<UsageUnpricedModelRow>,
    pub(super) diagnostics: Option<UsageDiagnostics>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct LimitUsageRow {
    pub(super) window_id: String,
    pub(super) account_id: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) limit_id: Option<String>,
    pub(super) window: String,
    pub(super) window_minutes: i64,
    pub(super) window_start: Option<DateTime<Utc>>,
    pub(super) reset_at: Option<DateTime<Utc>>,
    pub(super) observed: bool,
    pub(super) group_by: &'static str,
    pub(super) group_key: String,
    pub(super) sessions: usize,
    pub(super) calls: i64,
    pub(super) usage: TokenUsage,
    pub(super) credits: f64,
    pub(super) usd: f64,
    pub(super) priced_calls: i64,
    pub(super) unpriced_calls: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct LimitUsageDiagnostics {
    pub(super) observed_windows: i64,
    pub(super) unobserved_usage_events: i64,
    pub(super) usage: UsageDiagnostics,
    pub(super) rate_limits: RateLimitDiagnostics,
}

#[derive(Clone, Debug)]
pub(super) struct LimitUsageReport {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) limit_window: &'static str,
    pub(super) window_minutes: i64,
    pub(super) group_by: LimitUsageGroupBy,
    pub(super) include_reasoning_effort: bool,
    pub(super) sort_by: Option<StatSort>,
    pub(super) limit: Option<usize>,
    pub(super) sessions_dir: String,
    pub(super) rows: Vec<LimitUsageRow>,
    pub(super) totals: UsageStatRow,
    pub(super) unpriced_models: Vec<UsageUnpricedModelRow>,
    pub(super) diagnostics: Option<LimitUsageDiagnostics>,
}

#[derive(Clone, Debug)]
pub(super) struct UsageSessionRow {
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) cwd: String,
    pub(super) first_seen: DateTime<Utc>,
    pub(super) last_seen: DateTime<Utc>,
    pub(super) calls: i64,
    pub(super) usage: TokenUsage,
    pub(super) credits: f64,
    pub(super) usd: f64,
    pub(super) priced_calls: i64,
    pub(super) unpriced_calls: i64,
    pub(super) file_path: String,
}

#[derive(Clone, Debug)]
pub(super) struct UsageSessionEventRow {
    pub(super) timestamp: DateTime<Utc>,
    pub(super) model: String,
    pub(super) usage_mode: UsageMode,
    pub(super) reasoning_effort: Option<String>,
    pub(super) cwd: String,
    pub(super) usage: TokenUsage,
    pub(super) credits: f64,
    pub(super) usd: f64,
    pub(super) priced: bool,
    pub(super) file_path: String,
}

#[derive(Clone, Debug)]
pub(super) struct UsageSessionCompactRow {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) events: usize,
    pub(super) model: String,
    pub(super) reasoning_effort: Option<String>,
    pub(super) usage: TokenUsage,
    pub(super) credits: f64,
    pub(super) usd: f64,
    pub(super) unpriced_calls: i64,
}

#[derive(Clone, Debug)]
pub(super) struct UsageSessionsReport {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) sort_by: Option<StatSort>,
    pub(super) limit: usize,
    pub(super) sessions_dir: String,
    pub(super) rows: Vec<UsageSessionRow>,
    pub(super) totals: UsageStatRow,
    pub(super) unpriced_models: Vec<UsageUnpricedModelRow>,
    pub(super) diagnostics: Option<UsageDiagnostics>,
}

#[derive(Clone, Debug)]
pub(super) struct UsageSessionDetailReport {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) session_id: String,
    pub(super) limit: Option<usize>,
    pub(super) sessions_dir: String,
    pub(super) summary: Option<UsageSessionRow>,
    pub(super) rows: Vec<UsageSessionEventRow>,
    pub(super) by_model: Vec<UsageStatRow>,
    pub(super) by_cwd: Vec<UsageStatRow>,
    pub(super) by_reasoning_effort: Vec<UsageStatRow>,
    pub(super) model_switches: i64,
    pub(super) cwd_switches: i64,
    pub(super) reasoning_effort_switches: i64,
    pub(super) totals: UsageStatRow,
    pub(super) unpriced_models: Vec<UsageUnpricedModelRow>,
    pub(super) diagnostics: Option<UsageDiagnostics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageStatsJson<'a> {
    start: String,
    end: String,
    group_by: &'static str,
    include_reasoning_effort: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_by: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    sessions_dir: &'a str,
    rows: &'a [UsageStatRow],
    totals: &'a UsageStatRow,
    unpriced_models: &'a [UsageUnpricedModelRow],
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<&'a UsageDiagnostics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LimitUsageJson<'a> {
    start: String,
    end: String,
    limit_window: &'static str,
    window_minutes: i64,
    group_by: &'static str,
    include_reasoning_effort: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_by: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    sessions_dir: &'a str,
    rows: &'a [LimitUsageRow],
    totals: &'a UsageStatRow,
    unpriced_models: &'a [UsageUnpricedModelRow],
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<&'a LimitUsageDiagnostics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSessionsJson<'a> {
    start: String,
    end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_by: Option<&'static str>,
    limit: usize,
    sessions_dir: &'a str,
    rows: Vec<UsageSessionRowJson<'a>>,
    totals: &'a UsageStatRow,
    unpriced_models: &'a [UsageUnpricedModelRow],
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<&'a UsageDiagnostics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSessionRowJson<'a> {
    session_id: &'a str,
    model: &'a str,
    cwd: &'a str,
    first_seen: String,
    last_seen: String,
    calls: i64,
    usage: &'a TokenUsage,
    credits: f64,
    usd: f64,
    priced_calls: i64,
    unpriced_calls: i64,
    file_path: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSessionDetailJson<'a> {
    start: String,
    end: String,
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    sessions_dir: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<UsageSessionRowJson<'a>>,
    rows: Vec<UsageSessionEventRowJson<'a>>,
    by_model: &'a [UsageStatRow],
    by_cwd: &'a [UsageStatRow],
    by_reasoning_effort: &'a [UsageStatRow],
    model_switches: i64,
    cwd_switches: i64,
    reasoning_effort_switches: i64,
    totals: &'a UsageStatRow,
    unpriced_models: &'a [UsageUnpricedModelRow],
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<&'a UsageDiagnostics>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSessionEventRowJson<'a> {
    timestamp: String,
    model: &'a str,
    usage_mode: UsageMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    cwd: &'a str,
    usage: &'a TokenUsage,
    credits: f64,
    usd: f64,
    priced: bool,
    file_path: &'a str,
}

pub(super) fn to_usage_stats_json(report: &UsageStatsReport) -> UsageStatsJson<'_> {
    UsageStatsJson {
        start: iso_string(report.start),
        end: iso_string(report.end),
        group_by: report.group_by.as_str(),
        include_reasoning_effort: report.include_reasoning_effort,
        sort_by: report.sort_by.map(StatSort::as_str),
        limit: report.limit,
        sessions_dir: &report.sessions_dir,
        rows: &report.rows,
        totals: &report.totals,
        unpriced_models: &report.unpriced_models,
        warnings: usage_warnings(
            report.start,
            report.end,
            report.diagnostics.as_ref(),
            &report.unpriced_models,
        ),
        diagnostics: report.diagnostics.as_ref(),
    }
}

pub(super) fn to_limit_usage_json(report: &LimitUsageReport) -> LimitUsageJson<'_> {
    LimitUsageJson {
        start: iso_string(report.start),
        end: iso_string(report.end),
        limit_window: report.limit_window,
        window_minutes: report.window_minutes,
        group_by: report.group_by.as_str(),
        include_reasoning_effort: report.include_reasoning_effort,
        sort_by: report.sort_by.map(StatSort::as_str),
        limit: report.limit,
        sessions_dir: &report.sessions_dir,
        rows: &report.rows,
        totals: &report.totals,
        unpriced_models: &report.unpriced_models,
        warnings: limit_usage_warnings(report),
        diagnostics: report.diagnostics.as_ref(),
    }
}

pub(super) fn to_usage_sessions_json(report: &UsageSessionsReport) -> UsageSessionsJson<'_> {
    UsageSessionsJson {
        start: iso_string(report.start),
        end: iso_string(report.end),
        sort_by: report.sort_by.map(StatSort::as_str),
        limit: report.limit,
        sessions_dir: &report.sessions_dir,
        rows: report.rows.iter().map(to_session_row_json).collect(),
        totals: &report.totals,
        unpriced_models: &report.unpriced_models,
        warnings: usage_warnings(
            report.start,
            report.end,
            report.diagnostics.as_ref(),
            &report.unpriced_models,
        ),
        diagnostics: report.diagnostics.as_ref(),
    }
}

pub(super) fn to_usage_session_detail_json(
    report: &UsageSessionDetailReport,
) -> UsageSessionDetailJson<'_> {
    UsageSessionDetailJson {
        start: iso_string(report.start),
        end: iso_string(report.end),
        session_id: &report.session_id,
        limit: report.limit,
        sessions_dir: &report.sessions_dir,
        summary: report.summary.as_ref().map(to_session_row_json),
        rows: report.rows.iter().map(to_session_event_row_json).collect(),
        by_model: &report.by_model,
        by_cwd: &report.by_cwd,
        by_reasoning_effort: &report.by_reasoning_effort,
        model_switches: report.model_switches,
        cwd_switches: report.cwd_switches,
        reasoning_effort_switches: report.reasoning_effort_switches,
        totals: &report.totals,
        unpriced_models: &report.unpriced_models,
        warnings: usage_warnings(
            report.start,
            report.end,
            report.diagnostics.as_ref(),
            &report.unpriced_models,
        ),
        diagnostics: report.diagnostics.as_ref(),
    }
}

fn to_session_row_json(row: &UsageSessionRow) -> UsageSessionRowJson<'_> {
    UsageSessionRowJson {
        session_id: &row.session_id,
        model: &row.model,
        cwd: &row.cwd,
        first_seen: iso_string(row.first_seen),
        last_seen: iso_string(row.last_seen),
        calls: row.calls,
        usage: &row.usage,
        credits: row.credits,
        usd: row.usd,
        priced_calls: row.priced_calls,
        unpriced_calls: row.unpriced_calls,
        file_path: &row.file_path,
    }
}

fn to_session_event_row_json(row: &UsageSessionEventRow) -> UsageSessionEventRowJson<'_> {
    UsageSessionEventRowJson {
        timestamp: iso_string(row.timestamp),
        model: &row.model,
        usage_mode: row.usage_mode,
        reasoning_effort: row.reasoning_effort.as_deref(),
        cwd: &row.cwd,
        usage: &row.usage,
        credits: row.credits,
        usd: row.usd,
        priced: row.priced,
        file_path: &row.file_path,
    }
}

pub(super) fn usage_warnings(
    _start: DateTime<Utc>,
    _end: DateTime<Utc>,
    diagnostics: Option<&UsageDiagnostics>,
    unpriced_models: &[UsageUnpricedModelRow],
) -> Vec<String> {
    let mut warnings = Vec::new();

    if let Some(diagnostics) = diagnostics {
        if diagnostics.skipped_events.missing_metadata > 0 {
            warnings.push(format!(
                "{} token count event(s) were skipped due to missing metadata.",
                diagnostics.skipped_events.missing_metadata
            ));
        }

        if diagnostics.skipped_events.missing_usage > 0 {
            warnings.push(format!(
                "{} token count event(s) were skipped due to missing usage data.",
                diagnostics.skipped_events.missing_usage
            ));
        }

        if diagnostics.skipped_events.out_of_range > 0 {
            warnings.push(format!(
                "{} token count event(s) were outside the requested time range.",
                diagnostics.skipped_events.out_of_range
            ));
        }

        if diagnostics.skipped_events.account_mismatch > 0 {
            warnings.push(format!(
                "{} token count event(s) were filtered by account mismatch.",
                diagnostics.skipped_events.account_mismatch
            ));
        }

        if diagnostics.skipped_events.fork_replay > 0 {
            warnings.push(format!(
                "{} token count event(s) were skipped as fork replay.",
                diagnostics.skipped_events.fork_replay
            ));
        }

        if diagnostics.invalid_json_lines > 0 {
            warnings.push(format!(
                "{} line(s) contained invalid JSON and were skipped.",
                diagnostics.invalid_json_lines
            ));
        }
    }

    if !unpriced_models.is_empty() {
        let total_unpriced_calls: i64 = unpriced_models.iter().map(|m| m.calls).sum();
        let model_names = unpriced_models
            .iter()
            .map(|m| m.model.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "{} call(s) with unpriced model(s): {model_names}",
            total_unpriced_calls
        ));
    }

    warnings
}

pub(super) fn limit_usage_warnings(report: &LimitUsageReport) -> Vec<String> {
    let mut warnings = Vec::new();
    if report
        .diagnostics
        .as_ref()
        .is_some_and(|diagnostics| diagnostics.unobserved_usage_events > 0)
    {
        warnings.push(
            "Some usage events were not inside an observed rate-limit window and are grouped as unobserved.".to_string(),
        );
    }
    warnings
}

pub(super) fn is_all_usage_range(start: DateTime<Utc>, end: DateTime<Utc>) -> bool {
    start == local_to_utc(1900, 1, 1, 0, 0, 0, 0)
        && end == local_to_utc(9999, 12, 31, 23, 59, 59, 999)
}

pub(super) fn format_report_range(start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    if is_all_usage_range(start, end) {
        "all".to_string()
    } else {
        format!("{} to {}", format_date_time(start), format_date_time(end))
    }
}

pub(super) fn format_group_by(report: &UsageStatsReport) -> String {
    if report.group_by == StatGroupBy::Model && report.include_reasoning_effort {
        "model + reasoning_effort".to_string()
    } else {
        report.group_by.as_str().to_string()
    }
}

pub(super) fn format_date_time(date: DateTime<Utc>) -> String {
    let local = date.with_timezone(&Local);
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second()
    )
}

fn iso_string(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn usage_warnings_include_diagnostics_and_unpriced_models() {
        let mut diagnostics = UsageDiagnostics::new(1, false);
        diagnostics.prefiltered_files = 2;
        diagnostics.skipped_events.missing_metadata = 1;
        diagnostics.skipped_events.account_mismatch = 3;
        diagnostics.invalid_json_lines = 4;
        let unpriced_models = vec![UsageUnpricedModelRow {
            model: "future-model".to_string(),
            pricing_key: "future-model".to_string(),
            calls: 5,
            total_tokens: 100,
            note: None,
            pricing_stub: "{}".to_string(),
        }];

        let warnings = usage_warnings(
            utc(2026, 5, 1),
            utc(2026, 5, 2),
            Some(&diagnostics),
            &unpriced_models,
        );

        assert!(!warnings.iter().any(|line| line.contains("--full-scan")));
        assert!(warnings
            .iter()
            .any(|line| line.contains("missing metadata")));
        assert!(warnings
            .iter()
            .any(|line| line.contains("account mismatch")));
        assert!(warnings.iter().any(|line| line.contains("invalid JSON")));
        assert!(warnings
            .iter()
            .any(|line| line.contains("5 call(s) with unpriced model(s): future-model")));
    }

    #[test]
    fn usage_warnings_include_unpriced_models_without_diagnostics() {
        let unpriced_models = vec![UsageUnpricedModelRow {
            model: "future-model".to_string(),
            pricing_key: "future-model".to_string(),
            calls: 1,
            total_tokens: 10,
            note: None,
            pricing_stub: "{}".to_string(),
        }];

        let warnings = usage_warnings(utc(2026, 5, 1), utc(2026, 5, 2), None, &unpriced_models);

        assert_eq!(
            warnings,
            vec!["1 call(s) with unpriced model(s): future-model".to_string()]
        );
    }

    fn utc(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("valid test time")
    }
}
