use super::reports::{
    LimitUsageDiagnostics, LimitUsageGroupBy, LimitUsageReport, LimitUsageRow, TokenUsage,
    UsageDiagnostics, UsageMode, UsageRecordView, UsageSessionDetailReport, UsageSessionEventRow,
    UsageSessionRow, UsageSessionsReport, UsageStatRow, UsageStatsReport, UsageUnpricedModelRow,
};
use super::scan::UsageRecordAccumulator;
use super::StatSort;
use crate::format::{credits_to_usd, round_credits};
use crate::limits::{LimitWindow, LimitWindowSelector, RateLimitDiagnostics};
use crate::pricing::{
    calculate_credit_cost_with_context_at, normalize_model_name, CreditCost, PricingContext,
};
use crate::time::StatGroupBy;
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct MutableStatRow {
    sessions: HashSet<String>,
    calls: i64,
    usage: TokenUsage,
    credits: f64,
    priced_calls: i64,
    unpriced_calls: i64,
}

struct MutableLimitUsageRow {
    window_id: String,
    account_id: Option<String>,
    plan_type: Option<String>,
    limit_id: Option<String>,
    window: String,
    window_minutes: i64,
    window_start: Option<DateTime<Utc>>,
    reset_at: Option<DateTime<Utc>>,
    observed: bool,
    group_by: LimitUsageGroupBy,
    group_key: String,
    sessions: HashSet<String>,
    calls: i64,
    usage: TokenUsage,
    credits: f64,
    priced_calls: i64,
    unpriced_calls: i64,
}

#[derive(Default)]
struct MutableSession {
    session_id: String,
    model: String,
    cwd: String,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
    calls: i64,
    usage: TokenUsage,
    credits: f64,
    priced_calls: i64,
    unpriced_calls: i64,
    file_path: String,
}

pub(super) struct UsageStatsAccumulator {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    group_by: StatGroupBy,
    sessions_dir: String,
    include_reasoning_effort: bool,
    sort_by: Option<StatSort>,
    limit: Option<usize>,
    rows: HashMap<String, MutableStatRow>,
    total_sessions: HashSet<String>,
    totals: TokenUsage,
    calls: i64,
    unpriced_models: HashMap<String, UsageUnpricedModelRow>,
    fast_attributed_calls: i64,
    fast_attributed_credits: f64,
}

impl UsageStatsAccumulator {
    pub(super) fn new(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        group_by: StatGroupBy,
        sessions_dir: String,
        include_reasoning_effort: bool,
        sort_by: Option<StatSort>,
        limit: Option<usize>,
    ) -> Self {
        Self {
            start,
            end,
            group_by,
            sessions_dir,
            include_reasoning_effort,
            sort_by,
            limit,
            rows: HashMap::new(),
            total_sessions: HashSet::new(),
            totals: TokenUsage::default(),
            calls: 0,
            unpriced_models: HashMap::new(),
            fast_attributed_calls: 0,
            fast_attributed_credits: 0.0,
        }
    }

    pub(super) fn add(&mut self, record: UsageRecordView<'_>) {
        let key = group_key(&record, self.group_by, self.include_reasoning_effort);
        let row = self.rows.entry(key).or_default();
        let cost = record_credit_cost(&record);
        add_fast_attribution(
            &mut self.fast_attributed_calls,
            &mut self.fast_attributed_credits,
            &record,
            &cost,
        );

        if !row.sessions.contains(record.session_id) {
            row.sessions.insert(record.session_id.to_string());
        }
        row.calls += 1;
        row.usage.add(record.usage);
        row.credits += cost.credits;

        if cost.priced {
            row.priced_calls += 1;
        } else {
            row.unpriced_calls += 1;
            add_unpriced_model(
                &mut self.unpriced_models,
                record.model,
                record.usage,
                cost.unpriced_reason,
            );
        }

        if !self.total_sessions.contains(record.session_id) {
            self.total_sessions.insert(record.session_id.to_string());
        }
        self.totals.add(record.usage);
        self.calls += 1;
    }

    pub(super) fn finish(self, diagnostics: Option<UsageDiagnostics>) -> UsageStatsReport {
        let mut formatted_rows = self
            .rows
            .into_iter()
            .map(|(key, row)| UsageStatRow {
                key,
                sessions: row.sessions.len(),
                calls: row.calls,
                usage: row.usage,
                credits: round_credits(row.credits),
                usd: credits_to_usd(row.credits),
                priced_calls: row.priced_calls,
                unpriced_calls: row.unpriced_calls,
            })
            .collect::<Vec<_>>();
        formatted_rows
            .sort_by(|left, right| compare_stat_rows(left, right, self.sort_by, self.group_by));

        let total_credits = formatted_rows.iter().map(|row| row.credits).sum::<f64>();
        let total_priced_calls = formatted_rows.iter().map(|row| row.priced_calls).sum();
        let total_unpriced_calls = formatted_rows.iter().map(|row| row.unpriced_calls).sum();
        let rows = match self.limit {
            Some(limit) => formatted_rows.into_iter().take(limit).collect(),
            None => formatted_rows,
        };

        UsageStatsReport {
            start: self.start,
            end: self.end,
            group_by: self.group_by,
            include_reasoning_effort: self.include_reasoning_effort,
            sort_by: self.sort_by,
            limit: self.limit,
            sessions_dir: self.sessions_dir,
            rows,
            totals: UsageStatRow {
                key: "Total".to_string(),
                sessions: self.total_sessions.len(),
                calls: self.calls,
                usage: self.totals,
                credits: round_credits(total_credits),
                usd: credits_to_usd(total_credits),
                priced_calls: total_priced_calls,
                unpriced_calls: total_unpriced_calls,
            },
            unpriced_models: format_unpriced_models(self.unpriced_models),
            diagnostics: finish_usage_diagnostics(
                diagnostics,
                self.fast_attributed_calls,
                self.fast_attributed_credits,
            ),
        }
    }
}

impl UsageRecordAccumulator for UsageStatsAccumulator {
    fn add_record(&mut self, record: UsageRecordView<'_>) {
        self.add(record);
    }

    fn empty_like(&self) -> Self {
        Self::new(
            self.start,
            self.end,
            self.group_by,
            self.sessions_dir.clone(),
            self.include_reasoning_effort,
            self.sort_by,
            self.limit,
        )
    }

    fn merge(&mut self, other: Self) {
        for (key, other_row) in other.rows {
            let row = self.rows.entry(key).or_default();
            row.sessions.extend(other_row.sessions);
            row.calls += other_row.calls;
            row.usage.add(&other_row.usage);
            row.credits += other_row.credits;
            row.priced_calls += other_row.priced_calls;
            row.unpriced_calls += other_row.unpriced_calls;
        }

        self.total_sessions.extend(other.total_sessions);
        self.totals.add(&other.totals);
        self.calls += other.calls;
        merge_unpriced_models(&mut self.unpriced_models, other.unpriced_models);
        self.fast_attributed_calls += other.fast_attributed_calls;
        self.fast_attributed_credits += other.fast_attributed_credits;
    }
}

pub(super) struct LimitUsageAccumulator {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    selector: LimitWindowSelector,
    group_by: LimitUsageGroupBy,
    sessions_dir: String,
    include_reasoning_effort: bool,
    sort_by: Option<StatSort>,
    limit: Option<usize>,
    windows: Vec<LimitWindow>,
    rows: HashMap<String, MutableLimitUsageRow>,
    total_sessions: HashSet<String>,
    totals: TokenUsage,
    calls: i64,
    credits: f64,
    priced_calls: i64,
    unpriced_calls: i64,
    unpriced_models: HashMap<String, UsageUnpricedModelRow>,
    unobserved_usage_events: i64,
    fast_attributed_calls: i64,
    fast_attributed_credits: f64,
}

pub(super) struct LimitUsageAccumulatorConfig {
    pub(super) start: DateTime<Utc>,
    pub(super) end: DateTime<Utc>,
    pub(super) selector: LimitWindowSelector,
    pub(super) group_by: LimitUsageGroupBy,
    pub(super) sessions_dir: String,
    pub(super) include_reasoning_effort: bool,
    pub(super) sort_by: Option<StatSort>,
    pub(super) limit: Option<usize>,
    pub(super) windows: Vec<LimitWindow>,
}

impl LimitUsageAccumulator {
    pub(super) fn new(config: LimitUsageAccumulatorConfig) -> Self {
        let LimitUsageAccumulatorConfig {
            start,
            end,
            selector,
            group_by,
            sessions_dir,
            include_reasoning_effort,
            sort_by,
            limit,
            mut windows,
        } = config;

        windows.sort_by(|left, right| {
            left.reset_at
                .cmp(&right.reset_at)
                .then_with(|| left.estimated_start.cmp(&right.estimated_start))
                .then_with(|| left.id.cmp(&right.id))
        });

        Self {
            start,
            end,
            selector,
            group_by,
            sessions_dir,
            include_reasoning_effort,
            sort_by,
            limit,
            windows,
            rows: HashMap::new(),
            total_sessions: HashSet::new(),
            totals: TokenUsage::default(),
            calls: 0,
            credits: 0.0,
            priced_calls: 0,
            unpriced_calls: 0,
            unpriced_models: HashMap::new(),
            unobserved_usage_events: 0,
            fast_attributed_calls: 0,
            fast_attributed_credits: 0.0,
        }
    }

    pub(super) fn add(&mut self, record: UsageRecordView<'_>) {
        let window = self.window_for_record(&record).cloned();
        if window.is_none() {
            self.unobserved_usage_events += 1;
        }

        let group_key = limit_usage_group_key(
            &record,
            self.group_by,
            self.include_reasoning_effort,
            window.as_ref(),
        );
        let row_key = limit_usage_row_key(self.selector, window.as_ref(), &group_key);
        let row = self.rows.entry(row_key).or_insert_with(|| {
            mutable_limit_usage_row(self.selector, self.group_by, window.as_ref(), group_key)
        });
        let cost = record_credit_cost(&record);
        add_fast_attribution(
            &mut self.fast_attributed_calls,
            &mut self.fast_attributed_credits,
            &record,
            &cost,
        );

        row.sessions.insert(record.session_id.to_string());
        row.calls += 1;
        row.usage.add(record.usage);
        row.credits += cost.credits;
        if cost.priced {
            row.priced_calls += 1;
        } else {
            row.unpriced_calls += 1;
            add_unpriced_model(
                &mut self.unpriced_models,
                record.model,
                record.usage,
                cost.unpriced_reason,
            );
        }

        self.total_sessions.insert(record.session_id.to_string());
        self.totals.add(record.usage);
        self.calls += 1;
        self.credits += cost.credits;
        if cost.priced {
            self.priced_calls += 1;
        } else {
            self.unpriced_calls += 1;
        }
    }

    pub(super) fn finish(
        mut self,
        usage_diagnostics: UsageDiagnostics,
        rate_limit_diagnostics: RateLimitDiagnostics,
    ) -> LimitUsageReport {
        if self.group_by == LimitUsageGroupBy::Window {
            for window in &self.windows {
                let group_key = window.id.clone();
                let row_key = limit_usage_row_key(self.selector, Some(window), &group_key);
                self.rows.entry(row_key).or_insert_with(|| {
                    mutable_limit_usage_row(self.selector, self.group_by, Some(window), group_key)
                });
            }

            if self.rows.is_empty() {
                let group_key = "unobserved".to_string();
                let row_key = limit_usage_row_key(self.selector, None, &group_key);
                self.rows.entry(row_key).or_insert_with(|| {
                    mutable_limit_usage_row(self.selector, self.group_by, None, group_key)
                });
            }
        }

        let mut rows = self
            .rows
            .into_values()
            .map(limit_usage_row)
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| compare_limit_usage_rows(left, right, self.sort_by));
        if let Some(limit) = self.limit {
            rows.truncate(limit);
        }

        let mut usage_diagnostics = usage_diagnostics;
        usage_diagnostics
            .record_fast_attribution(self.fast_attributed_calls, self.fast_attributed_credits);

        let diagnostics = LimitUsageDiagnostics {
            observed_windows: self.windows.len() as i64,
            unobserved_usage_events: self.unobserved_usage_events,
            usage: usage_diagnostics,
            rate_limits: rate_limit_diagnostics,
        };

        LimitUsageReport {
            start: self.start,
            end: self.end,
            limit_window: self.selector.as_str(),
            window_minutes: self.selector.window_minutes(),
            group_by: self.group_by,
            include_reasoning_effort: self.include_reasoning_effort,
            sort_by: self.sort_by,
            limit: self.limit,
            sessions_dir: self.sessions_dir,
            rows,
            totals: UsageStatRow {
                key: "Total".to_string(),
                sessions: self.total_sessions.len(),
                calls: self.calls,
                usage: self.totals,
                credits: round_credits(self.credits),
                usd: credits_to_usd(self.credits),
                priced_calls: self.priced_calls,
                unpriced_calls: self.unpriced_calls,
            },
            unpriced_models: format_unpriced_models(self.unpriced_models),
            diagnostics: Some(diagnostics),
        }
    }

    fn window_for_record(&self, record: &UsageRecordView<'_>) -> Option<&LimitWindow> {
        let candidates = self
            .windows
            .iter()
            .filter(|window| {
                record.timestamp >= window.estimated_start && record.timestamp < window.reset_at
            })
            .filter(|window| match record.account_id {
                Some(account_id) => window
                    .account_id
                    .as_deref()
                    .is_none_or(|window_account| window_account == account_id),
                None => window.account_id.is_none(),
            })
            .collect::<Vec<_>>();

        if !record.rate_limits.is_empty() {
            return candidates
                .into_iter()
                .filter(|window| {
                    record.rate_limits.iter().any(|rate_limit| {
                        rate_limit.window_minutes == window.window_minutes
                            && (rate_limit.resets_at - window.reset_at).num_seconds().abs() <= 60
                            && rate_limit.plan_type.as_deref() == window.plan_type.as_deref()
                            && rate_limit.limit_id.as_deref() == window.limit_id.as_deref()
                    })
                })
                .max_by(|left, right| {
                    left.estimated_start
                        .cmp(&right.estimated_start)
                        .then_with(|| left.reset_at.cmp(&right.reset_at))
                        .then_with(|| left.id.cmp(&right.id))
                });
        }

        if candidates.len() != 1 {
            return None;
        }

        candidates.into_iter().max_by(|left, right| {
            left.estimated_start
                .cmp(&right.estimated_start)
                .then_with(|| left.reset_at.cmp(&right.reset_at))
                .then_with(|| left.id.cmp(&right.id))
        })
    }
}

impl UsageRecordAccumulator for LimitUsageAccumulator {
    fn add_record(&mut self, record: UsageRecordView<'_>) {
        self.add(record);
    }

    fn empty_like(&self) -> Self {
        Self::new(LimitUsageAccumulatorConfig {
            start: self.start,
            end: self.end,
            selector: self.selector,
            group_by: self.group_by,
            sessions_dir: self.sessions_dir.clone(),
            include_reasoning_effort: self.include_reasoning_effort,
            sort_by: self.sort_by,
            limit: self.limit,
            windows: self.windows.clone(),
        })
    }

    fn merge(&mut self, other: Self) {
        for (key, other_row) in other.rows {
            if let Some(row) = self.rows.get_mut(&key) {
                merge_mutable_limit_usage_row(row, other_row);
            } else {
                self.rows.insert(key, other_row);
            }
        }

        self.total_sessions.extend(other.total_sessions);
        self.totals.add(&other.totals);
        self.calls += other.calls;
        self.credits += other.credits;
        self.priced_calls += other.priced_calls;
        self.unpriced_calls += other.unpriced_calls;
        merge_unpriced_models(&mut self.unpriced_models, other.unpriced_models);
        self.unobserved_usage_events += other.unobserved_usage_events;
        self.fast_attributed_calls += other.fast_attributed_calls;
        self.fast_attributed_credits += other.fast_attributed_credits;
    }
}

pub(super) struct UsageSessionsAccumulator {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    sessions_dir: String,
    sort_by: Option<StatSort>,
    limit: usize,
    sessions: HashMap<String, MutableSession>,
    totals: TokenUsage,
    calls: i64,
    unpriced_models: HashMap<String, UsageUnpricedModelRow>,
    fast_attributed_calls: i64,
    fast_attributed_credits: f64,
}

impl UsageSessionsAccumulator {
    pub(super) fn new(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        sessions_dir: String,
        sort_by: Option<StatSort>,
        limit: usize,
    ) -> Self {
        Self {
            start,
            end,
            sessions_dir,
            sort_by,
            limit,
            sessions: HashMap::new(),
            totals: TokenUsage::default(),
            calls: 0,
            unpriced_models: HashMap::new(),
            fast_attributed_calls: 0,
            fast_attributed_credits: 0.0,
        }
    }

    pub(super) fn add(&mut self, record: UsageRecordView<'_>) {
        let session = if self.sessions.contains_key(record.session_id) {
            self.sessions
                .get_mut(record.session_id)
                .expect("session key was checked above")
        } else {
            self.sessions.insert(
                record.session_id.to_string(),
                MutableSession {
                    session_id: record.session_id.to_string(),
                    model: record.model.to_string(),
                    cwd: record.cwd.to_string(),
                    first_seen: Some(record.timestamp),
                    last_seen: Some(record.timestamp),
                    calls: 0,
                    usage: TokenUsage::default(),
                    credits: 0.0,
                    priced_calls: 0,
                    unpriced_calls: 0,
                    file_path: record.file_path.to_string(),
                },
            );
            self.sessions
                .get_mut(record.session_id)
                .expect("session was inserted above")
        };
        let cost = record_credit_cost(&record);
        add_fast_attribution(
            &mut self.fast_attributed_calls,
            &mut self.fast_attributed_credits,
            &record,
            &cost,
        );

        if record.model != "unknown" && session.model != record.model {
            session.model = record.model.to_string();
        }
        if record.cwd != "unknown" && session.cwd != record.cwd {
            session.cwd = record.cwd.to_string();
        }
        session.first_seen = Some(
            session
                .first_seen
                .unwrap_or(record.timestamp)
                .min(record.timestamp),
        );
        session.last_seen = Some(
            session
                .last_seen
                .unwrap_or(record.timestamp)
                .max(record.timestamp),
        );
        session.calls += 1;
        session.usage.add(record.usage);
        session.credits += cost.credits;

        if cost.priced {
            session.priced_calls += 1;
        } else {
            session.unpriced_calls += 1;
            add_unpriced_model(
                &mut self.unpriced_models,
                record.model,
                record.usage,
                cost.unpriced_reason,
            );
        }

        self.totals.add(record.usage);
        self.calls += 1;
    }

    pub(super) fn finish(self, diagnostics: Option<UsageDiagnostics>) -> UsageSessionsReport {
        let total_sessions = self.sessions.len();
        let total_credits = self.sessions.values().map(|row| row.credits).sum::<f64>();
        let total_priced_calls = self.sessions.values().map(|row| row.priced_calls).sum();
        let total_unpriced_calls = self.sessions.values().map(|row| row.unpriced_calls).sum();
        let mut session_rows = self
            .sessions
            .into_values()
            .filter_map(|session| {
                Some(UsageSessionRow {
                    session_id: session.session_id,
                    model: session.model,
                    cwd: session.cwd,
                    first_seen: session.first_seen?,
                    last_seen: session.last_seen?,
                    calls: session.calls,
                    usage: session.usage,
                    credits: round_credits(session.credits),
                    usd: credits_to_usd(session.credits),
                    priced_calls: session.priced_calls,
                    unpriced_calls: session.unpriced_calls,
                    file_path: session.file_path,
                })
            })
            .collect::<Vec<_>>();
        session_rows.sort_by(|left, right| compare_session_rows(left, right, self.sort_by));
        let rows = session_rows
            .into_iter()
            .take(self.limit)
            .collect::<Vec<_>>();

        UsageSessionsReport {
            start: self.start,
            end: self.end,
            sort_by: self.sort_by,
            limit: self.limit,
            sessions_dir: self.sessions_dir,
            rows,
            totals: UsageStatRow {
                key: "Total".to_string(),
                sessions: total_sessions,
                calls: self.calls,
                usage: self.totals,
                credits: round_credits(total_credits),
                usd: credits_to_usd(total_credits),
                priced_calls: total_priced_calls,
                unpriced_calls: total_unpriced_calls,
            },
            unpriced_models: format_unpriced_models(self.unpriced_models),
            diagnostics: finish_usage_diagnostics(
                diagnostics,
                self.fast_attributed_calls,
                self.fast_attributed_credits,
            ),
        }
    }
}

impl UsageRecordAccumulator for UsageSessionsAccumulator {
    fn add_record(&mut self, record: UsageRecordView<'_>) {
        self.add(record);
    }

    fn empty_like(&self) -> Self {
        Self::new(
            self.start,
            self.end,
            self.sessions_dir.clone(),
            self.sort_by,
            self.limit,
        )
    }

    fn merge(&mut self, other: Self) {
        for (session_id, other_session) in other.sessions {
            if let Some(session) = self.sessions.get_mut(&session_id) {
                merge_mutable_session(session, other_session);
            } else {
                self.sessions.insert(session_id, other_session);
            }
        }

        self.totals.add(&other.totals);
        self.calls += other.calls;
        merge_unpriced_models(&mut self.unpriced_models, other.unpriced_models);
        self.fast_attributed_calls += other.fast_attributed_calls;
        self.fast_attributed_credits += other.fast_attributed_credits;
    }
}

pub(super) struct UsageSessionDetailAccumulator {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    sessions_dir: String,
    limit: Option<usize>,
    session_id: String,
    rows: Vec<UsageSessionEventRow>,
    summary: Option<MutableSession>,
    totals: TokenUsage,
    calls: i64,
    credits: f64,
    priced_calls: i64,
    unpriced_calls: i64,
    unpriced_models: HashMap<String, UsageUnpricedModelRow>,
    fast_attributed_calls: i64,
    fast_attributed_credits: f64,
}

impl UsageSessionDetailAccumulator {
    pub(super) fn new(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        sessions_dir: String,
        limit: Option<usize>,
        session_id: String,
    ) -> Self {
        Self {
            start,
            end,
            sessions_dir,
            limit,
            session_id,
            rows: Vec::new(),
            summary: None,
            totals: TokenUsage::default(),
            calls: 0,
            credits: 0.0,
            priced_calls: 0,
            unpriced_calls: 0,
            unpriced_models: HashMap::new(),
            fast_attributed_calls: 0,
            fast_attributed_credits: 0.0,
        }
    }

    pub(super) fn add(&mut self, record: UsageRecordView<'_>) {
        if record.session_id != self.session_id {
            return;
        }

        let cost = record_credit_cost(&record);
        add_fast_attribution(
            &mut self.fast_attributed_calls,
            &mut self.fast_attributed_credits,
            &record,
            &cost,
        );
        let summary = self.summary.get_or_insert_with(|| MutableSession {
            session_id: record.session_id.to_string(),
            model: record.model.to_string(),
            cwd: record.cwd.to_string(),
            first_seen: Some(record.timestamp),
            last_seen: Some(record.timestamp),
            calls: 0,
            usage: TokenUsage::default(),
            credits: 0.0,
            priced_calls: 0,
            unpriced_calls: 0,
            file_path: record.file_path.to_string(),
        });

        if record.model != "unknown" && summary.model != record.model {
            summary.model = record.model.to_string();
        }
        if record.cwd != "unknown" && summary.cwd != record.cwd {
            summary.cwd = record.cwd.to_string();
        }
        summary.first_seen = Some(
            summary
                .first_seen
                .unwrap_or(record.timestamp)
                .min(record.timestamp),
        );
        summary.last_seen = Some(
            summary
                .last_seen
                .unwrap_or(record.timestamp)
                .max(record.timestamp),
        );
        summary.calls += 1;
        summary.usage.add(record.usage);
        summary.credits += cost.credits;

        self.calls += 1;
        self.credits += cost.credits;
        self.totals.add(record.usage);

        if cost.priced {
            self.priced_calls += 1;
            summary.priced_calls += 1;
        } else {
            self.unpriced_calls += 1;
            summary.unpriced_calls += 1;
            add_unpriced_model(
                &mut self.unpriced_models,
                record.model,
                record.usage,
                cost.unpriced_reason.clone(),
            );
        }

        self.rows.push(UsageSessionEventRow {
            timestamp: record.timestamp,
            model: record.model.to_string(),
            usage_mode: record.usage_mode,
            reasoning_effort: record.reasoning_effort.map(str::to_string),
            cwd: record.cwd.to_string(),
            usage: record.usage.clone(),
            credits: round_credits(cost.credits),
            usd: credits_to_usd(cost.credits),
            priced: cost.priced,
            file_path: record.file_path.to_string(),
        });
    }

    pub(super) fn finish(
        mut self,
        diagnostics: Option<UsageDiagnostics>,
    ) -> UsageSessionDetailReport {
        self.rows.sort_by(|left, right| {
            left.timestamp
                .cmp(&right.timestamp)
                .then_with(|| left.model.cmp(&right.model))
                .then_with(|| left.file_path.cmp(&right.file_path))
        });
        let all_rows = self.rows;
        let output_rows = match self.limit {
            Some(limit) => all_rows.iter().take(limit).cloned().collect(),
            None => all_rows.clone(),
        };
        let by_model = build_session_event_breakdown(&all_rows, session_event_model_group_key);
        let by_cwd = build_session_event_breakdown(&all_rows, |row| row.cwd.clone());
        let by_reasoning_effort = build_session_event_breakdown(&all_rows, |row| {
            row.reasoning_effort
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        });
        let summary = self.summary.and_then(|summary| {
            Some(UsageSessionRow {
                session_id: summary.session_id,
                model: summary.model,
                cwd: summary.cwd,
                first_seen: summary.first_seen?,
                last_seen: summary.last_seen?,
                calls: summary.calls,
                usage: summary.usage,
                credits: round_credits(summary.credits),
                usd: credits_to_usd(summary.credits),
                priced_calls: summary.priced_calls,
                unpriced_calls: summary.unpriced_calls,
                file_path: summary.file_path,
            })
        });

        UsageSessionDetailReport {
            start: self.start,
            end: self.end,
            session_id: self.session_id,
            limit: self.limit,
            sessions_dir: self.sessions_dir,
            summary,
            rows: output_rows,
            by_model,
            by_cwd,
            by_reasoning_effort,
            model_switches: count_value_switches(&all_rows, |row| row.model.as_str()),
            cwd_switches: count_value_switches(&all_rows, |row| row.cwd.as_str()),
            reasoning_effort_switches: count_value_switches(&all_rows, |row| {
                row.reasoning_effort.as_deref().unwrap_or("unknown")
            }),
            totals: UsageStatRow {
                key: "Total".to_string(),
                sessions: if self.calls == 0 { 0 } else { 1 },
                calls: self.calls,
                usage: self.totals,
                credits: round_credits(self.credits),
                usd: credits_to_usd(self.credits),
                priced_calls: self.priced_calls,
                unpriced_calls: self.unpriced_calls,
            },
            unpriced_models: format_unpriced_models(self.unpriced_models),
            diagnostics: finish_usage_diagnostics(
                diagnostics,
                self.fast_attributed_calls,
                self.fast_attributed_credits,
            ),
        }
    }
}

impl UsageRecordAccumulator for UsageSessionDetailAccumulator {
    fn add_record(&mut self, record: UsageRecordView<'_>) {
        self.add(record);
    }

    fn empty_like(&self) -> Self {
        Self::new(
            self.start,
            self.end,
            self.sessions_dir.clone(),
            self.limit,
            self.session_id.clone(),
        )
    }

    fn merge(&mut self, other: Self) {
        if let Some(other_summary) = other.summary {
            if let Some(summary) = self.summary.as_mut() {
                merge_mutable_session(summary, other_summary);
            } else {
                self.summary = Some(other_summary);
            }
        }

        self.rows.extend(other.rows);
        self.totals.add(&other.totals);
        self.calls += other.calls;
        self.credits += other.credits;
        self.priced_calls += other.priced_calls;
        self.unpriced_calls += other.unpriced_calls;
        merge_unpriced_models(&mut self.unpriced_models, other.unpriced_models);
        self.fast_attributed_calls += other.fast_attributed_calls;
        self.fast_attributed_credits += other.fast_attributed_credits;
    }
}

fn mutable_limit_usage_row(
    selector: LimitWindowSelector,
    group_by: LimitUsageGroupBy,
    window: Option<&LimitWindow>,
    group_key: String,
) -> MutableLimitUsageRow {
    match window {
        Some(window) => MutableLimitUsageRow {
            window_id: window.id.clone(),
            account_id: window.account_id.clone(),
            plan_type: window.plan_type.clone(),
            limit_id: window.limit_id.clone(),
            window: window.window.clone(),
            window_minutes: window.window_minutes,
            window_start: Some(window.estimated_start),
            reset_at: Some(window.reset_at),
            observed: true,
            group_by,
            group_key,
            sessions: HashSet::new(),
            calls: 0,
            usage: TokenUsage::default(),
            credits: 0.0,
            priced_calls: 0,
            unpriced_calls: 0,
        },
        None => MutableLimitUsageRow {
            window_id: format!("unobserved:{}", selector.as_str()),
            account_id: None,
            plan_type: None,
            limit_id: None,
            window: selector.as_str().to_string(),
            window_minutes: selector.window_minutes(),
            window_start: None,
            reset_at: None,
            observed: false,
            group_by,
            group_key,
            sessions: HashSet::new(),
            calls: 0,
            usage: TokenUsage::default(),
            credits: 0.0,
            priced_calls: 0,
            unpriced_calls: 0,
        },
    }
}

fn limit_usage_row(row: MutableLimitUsageRow) -> LimitUsageRow {
    LimitUsageRow {
        window_id: row.window_id,
        account_id: row.account_id,
        plan_type: row.plan_type,
        limit_id: row.limit_id,
        window: row.window,
        window_minutes: row.window_minutes,
        window_start: row.window_start,
        reset_at: row.reset_at,
        observed: row.observed,
        group_by: row.group_by.as_str(),
        group_key: row.group_key,
        sessions: row.sessions.len(),
        calls: row.calls,
        usage: row.usage,
        credits: round_credits(row.credits),
        usd: credits_to_usd(row.credits),
        priced_calls: row.priced_calls,
        unpriced_calls: row.unpriced_calls,
    }
}

fn limit_usage_group_key(
    record: &UsageRecordView<'_>,
    group_by: LimitUsageGroupBy,
    include_reasoning_effort: bool,
    window: Option<&LimitWindow>,
) -> String {
    match group_by.as_stat() {
        Some(stat_group_by) => group_key(record, stat_group_by, include_reasoning_effort),
        None => window
            .map(|window| window.id.clone())
            .unwrap_or_else(|| "unobserved".to_string()),
    }
}

fn limit_usage_row_key(
    selector: LimitWindowSelector,
    window: Option<&LimitWindow>,
    group_key: &str,
) -> String {
    match window {
        Some(window) => format!("{}|{group_key}", window.id),
        None => format!("unobserved:{}|{group_key}", selector.as_str()),
    }
}

fn compare_limit_usage_rows(
    left: &LimitUsageRow,
    right: &LimitUsageRow,
    sort_by: Option<StatSort>,
) -> Ordering {
    match sort_by {
        None | Some(StatSort::Time) => compare_limit_usage_rows_by_window(left, right),
        Some(StatSort::Tokens) => by_limit_usage_tokens_desc(left, right)
            .then_with(|| compare_limit_usage_rows_by_window(left, right)),
        Some(StatSort::Credits) => by_credits_desc(left.credits, right.credits)
            .then_with(|| compare_limit_usage_rows_by_window(left, right)),
        Some(StatSort::Calls) => right
            .calls
            .cmp(&left.calls)
            .then_with(|| compare_limit_usage_rows_by_window(left, right)),
        Some(StatSort::Sessions) => right
            .sessions
            .cmp(&left.sessions)
            .then_with(|| compare_limit_usage_rows_by_window(left, right)),
    }
}

fn compare_limit_usage_rows_by_window(left: &LimitUsageRow, right: &LimitUsageRow) -> Ordering {
    match (&left.reset_at, &right.reset_at) {
        (Some(left_reset), Some(right_reset)) => left_reset
            .cmp(right_reset)
            .then_with(|| left.window_start.cmp(&right.window_start))
            .then_with(|| left.group_key.cmp(&right.group_key))
            .then_with(|| left.window_id.cmp(&right.window_id)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left
            .group_key
            .cmp(&right.group_key)
            .then_with(|| left.window_id.cmp(&right.window_id)),
    }
}

fn by_limit_usage_tokens_desc(left: &LimitUsageRow, right: &LimitUsageRow) -> Ordering {
    right.usage.total_tokens.cmp(&left.usage.total_tokens)
}

fn merge_mutable_limit_usage_row(row: &mut MutableLimitUsageRow, other: MutableLimitUsageRow) {
    row.sessions.extend(other.sessions);
    row.calls += other.calls;
    row.usage.add(&other.usage);
    row.credits += other.credits;
    row.priced_calls += other.priced_calls;
    row.unpriced_calls += other.unpriced_calls;
}

fn record_credit_cost(record: &UsageRecordView<'_>) -> CreditCost {
    let context = if record.usage_mode.is_fast() {
        PricingContext::fast()
    } else {
        PricingContext::normal()
    };
    calculate_credit_cost_with_context_at(
        record.model,
        record.usage.pricing_usage(),
        context,
        record.timestamp,
    )
}

fn add_fast_attribution(
    calls: &mut i64,
    credits: &mut f64,
    record: &UsageRecordView<'_>,
    cost: &CreditCost,
) {
    if record.usage_mode.is_fast() {
        *calls += 1;
        *credits += cost.credits;
    }
}

fn finish_usage_diagnostics(
    mut diagnostics: Option<UsageDiagnostics>,
    fast_attributed_calls: i64,
    fast_attributed_credits: f64,
) -> Option<UsageDiagnostics> {
    if let Some(diagnostics) = &mut diagnostics {
        diagnostics.record_fast_attribution(fast_attributed_calls, fast_attributed_credits);
    }
    diagnostics
}

fn group_key(
    record: &UsageRecordView<'_>,
    group_by: StatGroupBy,
    include_reasoning_effort: bool,
) -> String {
    match group_by {
        StatGroupBy::Model => model_group_key(record, include_reasoning_effort),
        StatGroupBy::Cwd => record.cwd.to_string(),
        StatGroupBy::Account => record
            .account_id
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string()),
        StatGroupBy::Week => {
            let local = record.timestamp.with_timezone(&Local);
            let week = local.iso_week();
            format!("{}-W{:02}", week.year(), week.week())
        }
        StatGroupBy::Month => {
            let local = record.timestamp.with_timezone(&Local);
            format!("{}-{:02}", local.year(), local.month())
        }
        StatGroupBy::Hour => {
            let local = record.timestamp.with_timezone(&Local);
            format!(
                "{}-{:02}-{:02} {:02}:00",
                local.year(),
                local.month(),
                local.day(),
                local.hour()
            )
        }
        StatGroupBy::Day => {
            let local = record.timestamp.with_timezone(&Local);
            format!("{}-{:02}-{:02}", local.year(), local.month(), local.day())
        }
    }
}

fn model_group_key(record: &UsageRecordView<'_>, include_reasoning_effort: bool) -> String {
    model_key(
        record.model,
        record.usage_mode,
        record.reasoning_effort,
        include_reasoning_effort,
    )
}

fn session_event_model_group_key(row: &UsageSessionEventRow) -> String {
    model_key(&row.model, row.usage_mode, None, false)
}

fn model_key(
    model: &str,
    usage_mode: UsageMode,
    reasoning_effort: Option<&str>,
    include_reasoning_effort: bool,
) -> String {
    let mut key = model.to_string();
    if model != "unknown" && usage_mode.is_fast() {
        key.push_str("-fast");
    }

    if include_reasoning_effort && model != "unknown" {
        if let Some(effort) = reasoning_effort.and_then(normalize_reasoning_effort) {
            key.push('-');
            key.push_str(&effort);
        }
    }

    key
}

fn normalize_reasoning_effort(value: &str) -> Option<String> {
    let mut output = String::new();
    let mut previous_dash = false;

    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            output.push('-');
            previous_dash = true;
        }
    }

    let normalized = output.trim_matches('-').to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn compare_stat_rows(
    left: &UsageStatRow,
    right: &UsageStatRow,
    sort_by: Option<StatSort>,
    group_by: StatGroupBy,
) -> Ordering {
    match sort_by {
        None if group_by == StatGroupBy::Model => {
            by_tokens_desc(left, right).then_with(|| left.key.cmp(&right.key))
        }
        None => left.key.cmp(&right.key),
        Some(StatSort::Time) => left.key.cmp(&right.key),
        Some(StatSort::Tokens) => {
            by_tokens_desc(left, right).then_with(|| left.key.cmp(&right.key))
        }
        Some(StatSort::Credits) => {
            by_credits_desc(left.credits, right.credits).then_with(|| left.key.cmp(&right.key))
        }
        Some(StatSort::Calls) => right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.key.cmp(&right.key)),
        Some(StatSort::Sessions) => right
            .sessions
            .cmp(&left.sessions)
            .then_with(|| left.key.cmp(&right.key)),
    }
}

fn compare_session_rows(
    left: &UsageSessionRow,
    right: &UsageSessionRow,
    sort_by: Option<StatSort>,
) -> Ordering {
    match sort_by {
        Some(StatSort::Time) => right
            .last_seen
            .cmp(&left.last_seen)
            .then_with(|| left.session_id.cmp(&right.session_id)),
        Some(StatSort::Tokens) => {
            by_session_tokens_desc(left, right).then_with(|| left.session_id.cmp(&right.session_id))
        }
        Some(StatSort::Credits) | None => by_credits_desc(left.credits, right.credits)
            .then_with(|| by_session_tokens_desc(left, right))
            .then_with(|| left.session_id.cmp(&right.session_id)),
        Some(StatSort::Calls) => right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.session_id.cmp(&right.session_id)),
        Some(StatSort::Sessions) => left.session_id.cmp(&right.session_id),
    }
}

fn by_tokens_desc(left: &UsageStatRow, right: &UsageStatRow) -> Ordering {
    right.usage.total_tokens.cmp(&left.usage.total_tokens)
}

fn by_session_tokens_desc(left: &UsageSessionRow, right: &UsageSessionRow) -> Ordering {
    right.usage.total_tokens.cmp(&left.usage.total_tokens)
}

fn by_credits_desc(left: f64, right: f64) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

fn build_session_event_breakdown(
    rows: &[UsageSessionEventRow],
    key_for_row: impl Fn(&UsageSessionEventRow) -> String,
) -> Vec<UsageStatRow> {
    let mut grouped: HashMap<String, Vec<&UsageSessionEventRow>> = HashMap::new();
    for row in rows {
        grouped.entry(key_for_row(row)).or_default().push(row);
    }

    let mut output = grouped
        .into_iter()
        .map(|(key, group_rows)| {
            let mut usage = TokenUsage::default();
            let mut credits = 0.0;
            let mut priced_calls = 0;
            let mut unpriced_calls = 0;
            for row in group_rows.iter() {
                usage.add(&row.usage);
                credits += row.credits;
                if row.priced {
                    priced_calls += 1;
                } else {
                    unpriced_calls += 1;
                }
            }
            UsageStatRow {
                key,
                sessions: 1,
                calls: group_rows.len() as i64,
                usage,
                credits: round_credits(credits),
                usd: credits_to_usd(credits),
                priced_calls,
                unpriced_calls,
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        by_credits_desc(left.credits, right.credits)
            .then_with(|| by_tokens_desc(left, right))
            .then_with(|| left.key.cmp(&right.key))
    });
    output
}

fn count_value_switches<'a, T>(rows: &'a [T], value_for_row: impl Fn(&'a T) -> &'a str) -> i64 {
    let mut switches = 0;
    let mut previous: Option<&str> = None;
    for row in rows {
        let value = value_for_row(row);
        if previous.is_some_and(|previous| previous != value) {
            switches += 1;
        }
        previous = Some(value);
    }
    switches
}

fn merge_mutable_session(session: &mut MutableSession, other: MutableSession) {
    if other.model != "unknown" {
        session.model = other.model;
    }
    if other.cwd != "unknown" {
        session.cwd = other.cwd;
    }

    session.first_seen = match (session.first_seen, other.first_seen) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (None, Some(right)) => Some(right),
        (left, None) => left,
    };
    session.last_seen = match (session.last_seen, other.last_seen) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (None, Some(right)) => Some(right),
        (left, None) => left,
    };
    session.calls += other.calls;
    session.usage.add(&other.usage);
    session.credits += other.credits;
    session.priced_calls += other.priced_calls;
    session.unpriced_calls += other.unpriced_calls;
}

fn merge_unpriced_models(
    target: &mut HashMap<String, UsageUnpricedModelRow>,
    source: HashMap<String, UsageUnpricedModelRow>,
) {
    for (key, source_row) in source {
        if let Some(target_row) = target.get_mut(&key) {
            target_row.calls += source_row.calls;
            target_row.total_tokens += source_row.total_tokens;
        } else {
            target.insert(key, source_row);
        }
    }
}

fn add_unpriced_model(
    unpriced_models: &mut HashMap<String, UsageUnpricedModelRow>,
    model: &str,
    usage: &TokenUsage,
    note: Option<String>,
) {
    let pricing_key = normalize_model_name(model);
    let row = unpriced_models
        .entry(pricing_key.clone())
        .or_insert_with(|| UsageUnpricedModelRow {
            model: model.to_string(),
            pricing_key,
            calls: 0,
            total_tokens: 0,
            note,
            pricing_stub: format_pricing_stub(model),
        });

    row.calls += 1;
    row.total_tokens += usage.total_tokens;
}

fn format_unpriced_models(
    unpriced_models: HashMap<String, UsageUnpricedModelRow>,
) -> Vec<UsageUnpricedModelRow> {
    let mut rows = unpriced_models.into_values().collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .calls
            .cmp(&left.calls)
            .then_with(|| right.total_tokens.cmp(&left.total_tokens))
            .then_with(|| left.pricing_key.cmp(&right.pricing_key))
    });
    rows
}

fn format_pricing_stub(model: &str) -> String {
    let key = normalize_model_name(model);
    format!(
        "{{\n  \"key\": \"{key}\",\n  \"label\": \"{}\",\n  \"versions\": [\n    {{\n      \"effective_at\": null,\n      \"input_credits_per_million\": 0,\n      \"cached_input_credits_per_million\": 0,\n      \"output_credits_per_million\": 0,\n      \"fast_credit_multiplier\": 1\n    }}\n  ]\n}}",
        escape_double_quoted(model)
    )
}

fn escape_double_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::super::reports::UsageMode;
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn merges_stats_accumulators_without_losing_totals() {
        let start = utc_time(2026, 5, 10, 0);
        let end = utc_time(2026, 5, 10, 2);
        let mut left = UsageStatsAccumulator::new(
            start,
            end,
            StatGroupBy::Model,
            "/sessions".to_string(),
            false,
            None,
            None,
        );
        let mut right = left.empty_like();
        let left_usage = usage(10, 2, 12);
        let right_usage = usage(20, 3, 23);

        left.add(test_record(
            utc_time(2026, 5, 10, 0),
            "session-a",
            "gpt-5.5",
            "/repo-a",
            "/tmp/a.jsonl",
            &left_usage,
        ));
        right.add(test_record(
            utc_time(2026, 5, 10, 1),
            "session-b",
            "gpt-5.4",
            "/repo-b",
            "/tmp/b.jsonl",
            &right_usage,
        ));

        left.merge(right);
        let report = left.finish(None);

        assert_eq!(report.totals.calls, 2);
        assert_eq!(report.totals.sessions, 2);
        assert_eq!(report.totals.usage.input_tokens, 30);
        assert_eq!(report.totals.usage.output_tokens, 5);
        assert_eq!(report.totals.usage.total_tokens, 35);
        assert_eq!(report.rows.len(), 2);
    }

    #[test]
    fn merges_session_accumulators_in_file_partition_order() {
        let start = utc_time(2026, 5, 10, 0);
        let end = utc_time(2026, 5, 10, 2);
        let mut left = UsageSessionsAccumulator::new(start, end, "/sessions".to_string(), None, 10);
        let mut right = left.empty_like();
        let left_usage = usage(10, 2, 12);
        let right_usage = usage(20, 3, 23);

        left.add(test_record(
            utc_time(2026, 5, 10, 1),
            "session-a",
            "gpt-5.5",
            "/repo-a",
            "/tmp/a.jsonl",
            &left_usage,
        ));
        right.add(test_record(
            utc_time(2026, 5, 10, 0),
            "session-a",
            "gpt-5.4",
            "/repo-b",
            "/tmp/b.jsonl",
            &right_usage,
        ));

        left.merge(right);
        let report = left.finish(None);
        let row = report.rows.first().expect("merged session row");

        assert_eq!(report.totals.calls, 2);
        assert_eq!(report.totals.sessions, 1);
        assert_eq!(row.session_id, "session-a");
        assert_eq!(row.model, "gpt-5.4");
        assert_eq!(row.cwd, "/repo-b");
        assert_eq!(row.file_path, "/tmp/a.jsonl");
        assert_eq!(row.first_seen, utc_time(2026, 5, 10, 0));
        assert_eq!(row.last_seen, utc_time(2026, 5, 10, 1));
    }

    #[test]
    fn records_unpriced_model_notes_and_pricing_stub() {
        let start = utc_time(2026, 5, 10, 0);
        let end = utc_time(2026, 5, 10, 2);
        let mut accumulator = UsageStatsAccumulator::new(
            start,
            end,
            StatGroupBy::Model,
            "/sessions".to_string(),
            false,
            None,
            None,
        );
        let usage = usage(10, 2, 12);

        accumulator.add(test_record(
            utc_time(2026, 5, 10, 0),
            "session-a",
            "brand-new-model",
            "/repo-a",
            "/tmp/a.jsonl",
            &usage,
        ));

        let report = accumulator.finish(None);
        let row = report.unpriced_models.first().expect("unpriced model row");

        assert_eq!(report.totals.unpriced_calls, 1);
        assert_eq!(row.model, "brand-new-model");
        assert_eq!(row.calls, 1);
        assert_eq!(row.total_tokens, 12);
        assert!(row.pricing_stub.contains("\"brand-new-model\""));
    }

    #[test]
    fn groups_fast_usage_as_distinct_model_key() {
        let start = utc_time(2026, 5, 10, 0);
        let end = utc_time(2026, 5, 10, 2);
        let mut accumulator = UsageStatsAccumulator::new(
            start,
            end,
            StatGroupBy::Model,
            "/sessions".to_string(),
            false,
            None,
            None,
        );
        let normal_usage = usage(10, 2, 12);
        let fast_usage = usage(20, 4, 24);

        accumulator.add(test_record_with_mode(
            utc_time(2026, 5, 10, 0),
            "session-a",
            "gpt-5.5",
            UsageMode::Normal,
            "/repo-a",
            "/tmp/a.jsonl",
            &normal_usage,
        ));
        accumulator.add(test_record_with_mode(
            utc_time(2026, 5, 10, 1),
            "session-b",
            "gpt-5.5",
            UsageMode::Fast,
            "/repo-b",
            "/tmp/b.jsonl",
            &fast_usage,
        ));

        let report = accumulator.finish(None);

        assert_eq!(report.rows.len(), 2);
        assert!(report.rows.iter().any(|row| row.key == "gpt-5.5"));
        assert!(report.rows.iter().any(|row| row.key == "gpt-5.5-fast"));
        assert_eq!(
            model_key("gpt-5.5", UsageMode::Fast, Some("high"), true),
            "gpt-5.5-fast-high"
        );
    }

    #[test]
    fn prices_standard_and_fast_usage_across_rate_card_cutoff() {
        let start = parse_time("2026-07-30T17:17:05.000Z");
        let end = parse_time("2026-07-30T17:17:06.000Z");
        let cutoff = parse_time("2026-07-30T17:17:05.167Z");
        let mut accumulator = UsageStatsAccumulator::new(
            start,
            end,
            StatGroupBy::Model,
            "/sessions".to_string(),
            false,
            None,
            None,
        );
        let old_usage = usage(1_000_000, 0, 1_000_000);
        let new_usage = usage(1_000_000, 0, 1_000_000);

        accumulator.add(test_record_with_mode(
            cutoff - chrono::Duration::milliseconds(1),
            "old-standard",
            "gpt-5.6-terra",
            UsageMode::Normal,
            "/repo",
            "/tmp/old.jsonl",
            &old_usage,
        ));
        accumulator.add(test_record_with_mode(
            cutoff,
            "new-fast",
            "gpt-5.6-terra",
            UsageMode::Fast,
            "/repo",
            "/tmp/new.jsonl",
            &new_usage,
        ));

        let report = accumulator.finish(None);
        let standard = report
            .rows
            .iter()
            .find(|row| row.key == "gpt-5.6-terra")
            .expect("standard row");
        let fast = report
            .rows
            .iter()
            .find(|row| row.key == "gpt-5.6-terra-fast")
            .expect("fast row");

        assert_eq!(standard.credits, 62.5);
        assert_eq!(fast.credits, 125.0);
        assert_eq!(report.totals.credits, 187.5);
    }

    fn test_record<'a>(
        timestamp: DateTime<Utc>,
        session_id: &'a str,
        model: &'a str,
        cwd: &'a str,
        file_path: &'a str,
        usage: &'a TokenUsage,
    ) -> UsageRecordView<'a> {
        test_record_with_mode(
            timestamp,
            session_id,
            model,
            UsageMode::Normal,
            cwd,
            file_path,
            usage,
        )
    }

    fn test_record_with_mode<'a>(
        timestamp: DateTime<Utc>,
        session_id: &'a str,
        model: &'a str,
        usage_mode: UsageMode,
        cwd: &'a str,
        file_path: &'a str,
        usage: &'a TokenUsage,
    ) -> UsageRecordView<'a> {
        UsageRecordView {
            timestamp,
            session_id,
            model,
            usage_mode,
            reasoning_effort: None,
            cwd,
            account_id: None,
            file_path,
            rate_limits: &[],
            usage,
        }
    }

    fn usage(input_tokens: i64, output_tokens: i64, total_tokens: i64) -> TokenUsage {
        TokenUsage {
            input_tokens,
            cached_input_tokens: 0,
            output_tokens,
            reasoning_output_tokens: 0,
            total_tokens,
        }
    }

    fn utc_time(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .expect("utc time")
    }

    fn parse_time(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("timestamp")
            .with_timezone(&Utc)
    }
}
