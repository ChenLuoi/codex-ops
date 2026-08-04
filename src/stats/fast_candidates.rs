use crate::format::round_credits;
use crate::limits::{RateLimitSample, RateLimitSamplesReport};
use crate::pricing::{calculate_credit_cost_with_context_at, PricingContext};
use crate::stats::{UsageRecord, UsageRecordsReport};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const FIVE_HOURS_WINDOW_MINUTES: i64 = 300;
const MIN_SEGMENT_USAGE_CALLS: i64 = 3;
const MIN_SEGMENT_DELTA_USED_PERCENT: f64 = 1.0;
const PERCENT_EPSILON: f64 = 0.000_001;
const RESET_JITTER_TOLERANCE_SECONDS: i64 = 60;

pub(crate) const FAST_CANDIDATE_WINDOW: &str = "5h";
pub(crate) const CONFIDENCE_HIGH: &str = "high";
pub(crate) const CONFIDENCE_MEDIUM: &str = "medium";
pub(crate) const CONFIDENCE_LOW: &str = "low";
pub(crate) const CONFIDENCE_INSUFFICIENT: &str = "insufficient";
pub(crate) const REASON_EXPECTED_FAST_MULTIPLIER: &str = "matchesExpectedFastMultiplier";
pub(crate) const REASON_MIXED_MODELS: &str = "mixedModelsWeightedExpectedMultiplier";
pub(crate) const REASON_NO_FIVE_HOUR_SAMPLES: &str = "noFiveHourSamples";
pub(crate) const REASON_NO_USAGE: &str = "noUsageInSegment";
pub(crate) const REASON_NO_RISING_USAGE: &str = "noRisingUsageInSegment";
pub(crate) const REASON_NO_PRICED_USAGE: &str = "noPricedUsageInSegment";
pub(crate) const REASON_NO_FAST_MULTIPLIER_MODEL: &str = "noFastMultiplierModel";
pub(crate) const REASON_INSUFFICIENT_BASELINE: &str = "insufficientBaseline";
pub(crate) const REASON_NORMAL_MULTIPLIER: &str = "normalMultiplier";
pub(crate) const REASON_MINIMUM_PERCENT_STEP: &str = "minimumPercentStep";
pub(crate) const REASON_TOO_FEW_USAGE_CALLS: &str = "tooFewUsageCallsInSegment";

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FastCandidateReport {
    pub(crate) detection_only: bool,
    pub(crate) window: &'static str,
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: DateTime<Utc>,
    pub(crate) sessions_dir: String,
    pub(crate) candidates: Vec<FastCandidateRow>,
    pub(crate) diagnostics: FastCandidateDiagnostics,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FastCandidateRow {
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) segment_start: DateTime<Utc>,
    pub(crate) segment_end: DateTime<Utc>,
    pub(crate) session_id: String,
    pub(crate) model: String,
    pub(crate) file_path: String,
    pub(crate) account_id: Option<String>,
    pub(crate) plan_type: Option<String>,
    pub(crate) limit_id: Option<String>,
    pub(crate) resets_at: DateTime<Utc>,
    pub(crate) sample_pairs: i64,
    pub(crate) calls: i64,
    pub(crate) total_tokens: i64,
    pub(crate) delta_used_percent: f64,
    pub(crate) normal_credits: f64,
    pub(crate) percent_per_credit: f64,
    pub(crate) baseline_percent_per_credit: f64,
    pub(crate) effective_multiplier: f64,
    pub(crate) expected_fast_multiplier: f64,
    pub(crate) confidence: &'static str,
    pub(crate) reason: &'static str,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FastCandidateDiagnostics {
    pub(crate) no_five_hour_samples: bool,
    pub(crate) five_hour_samples: i64,
    pub(crate) sample_pairs: i64,
    pub(crate) active_sample_pairs: i64,
    pub(crate) rising_sample_pairs: i64,
    pub(crate) exact_usage_matches: i64,
    pub(crate) legacy_usage_matches: i64,
    pub(crate) ambiguous_legacy_usage_records: i64,
    pub(crate) segments_with_usage: i64,
    pub(crate) candidate_segments: i64,
    pub(crate) normal_segments: i64,
    pub(crate) insufficient_segments: i64,
    pub(crate) mixed_model_segments: i64,
    pub(crate) reason_counts: Vec<FastCandidateReasonCount>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FastCandidateReasonCount {
    pub(crate) reason: &'static str,
    pub(crate) count: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct StreamKey {
    account_id: Option<String>,
    plan_type: Option<String>,
    limit_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SampleIdentity {
    stream: StreamKey,
    timestamp: DateTime<Utc>,
    resets_at: DateTime<Utc>,
    session_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct GlobalStreamKey {
    stream: StreamKey,
    resets_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SegmentKey {
    stream: StreamKey,
    session_id: String,
    resets_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
struct SampleObservation {
    timestamp: DateTime<Utc>,
    used_percent: f64,
}

#[derive(Clone, Debug)]
struct GlobalSamplePair {
    stream: StreamKey,
    interval_start: DateTime<Utc>,
    interval_end: DateTime<Utc>,
    delta_used_percent: f64,
    resets_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UsageMatchKind {
    Exact,
    Legacy,
}

#[derive(Clone, Debug)]
struct CandidateInterval {
    stream: StreamKey,
    session_id: String,
    interval_start: DateTime<Utc>,
    interval_end: DateTime<Utc>,
    delta_used_percent: f64,
    resets_at: DateTime<Utc>,
    record_indexes: Vec<usize>,
}

#[derive(Clone, Debug)]
struct CandidateSegment {
    stream: StreamKey,
    session_id: String,
    segment_start: DateTime<Utc>,
    segment_end: DateTime<Utc>,
    delta_used_percent: f64,
    resets_at: DateTime<Utc>,
    sample_pairs: i64,
    record_indexes: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
struct IntervalUsageSummary {
    calls: i64,
    total_tokens: i64,
    normal_credits: f64,
    expected_fast_credit_weight: f64,
    sessions: BTreeSet<String>,
    file_paths: BTreeSet<String>,
    model_credits: BTreeMap<String, f64>,
    model_calls: BTreeMap<String, i64>,
}

impl IntervalUsageSummary {
    fn add(&mut self, record: &UsageRecord) {
        let normal_cost = calculate_credit_cost_with_context_at(
            &record.model,
            record.usage.pricing_usage(),
            PricingContext::normal(),
            record.timestamp,
        );
        let fast_cost = calculate_credit_cost_with_context_at(
            &record.model,
            record.usage.pricing_usage(),
            PricingContext::fast(),
            record.timestamp,
        );
        let expected_multiplier = if normal_cost.credits > PERCENT_EPSILON {
            fast_cost.credits / normal_cost.credits
        } else {
            1.0
        };

        self.calls += 1;
        self.total_tokens += record.usage.total_tokens;
        self.normal_credits += normal_cost.credits;
        self.expected_fast_credit_weight += normal_cost.credits * expected_multiplier;
        self.sessions.insert(record.session_id.clone());
        self.file_paths.insert(record.file_path.clone());
        *self.model_credits.entry(record.model.clone()).or_default() += normal_cost.credits;
        *self.model_calls.entry(record.model.clone()).or_default() += 1;
    }

    fn expected_fast_multiplier(&self) -> f64 {
        if self.normal_credits <= PERCENT_EPSILON {
            return 1.0;
        }
        self.expected_fast_credit_weight / self.normal_credits
    }

    fn dominant_model(&self) -> String {
        self.model_credits
            .iter()
            .max_by(|(left_model, left_credits), (right_model, right_credits)| {
                left_credits
                    .partial_cmp(right_credits)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        self.model_calls
                            .get(*left_model)
                            .unwrap_or(&0)
                            .cmp(self.model_calls.get(*right_model).unwrap_or(&0))
                    })
                    .then_with(|| right_model.cmp(left_model))
            })
            .map(|(model, _)| model.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn output_session_id(&self) -> String {
        single_or_mixed(&self.sessions)
    }

    fn output_file_path(&self) -> String {
        single_or_mixed(&self.file_paths)
    }

    fn is_mixed_model(&self) -> bool {
        self.model_credits.len() > 1
    }
}

#[derive(Clone, Debug)]
struct ScoredSegment {
    segment: CandidateSegment,
    summary: IntervalUsageSummary,
    percent_per_credit: f64,
    expected_fast_multiplier: f64,
}

pub(crate) fn build_fast_candidate_report(
    samples_report: &RateLimitSamplesReport,
    usage_report: &UsageRecordsReport,
) -> FastCandidateReport {
    let mut diagnostics = FastCandidateDiagnostics::default();
    let samples = normalized_five_hour_samples(&samples_report.samples, &mut diagnostics);
    diagnostics.no_five_hour_samples = samples.is_empty();
    if diagnostics.no_five_hour_samples {
        diagnostics.increment_reason(REASON_NO_FIVE_HOUR_SAMPLES);
    }

    let intervals = build_candidate_intervals(&samples, &usage_report.records, &mut diagnostics);
    let segments = build_candidate_segments(intervals);

    let candidates = score_candidate_segments(segments, &usage_report.records, &mut diagnostics);
    diagnostics.finalize_reason_counts();

    FastCandidateReport {
        detection_only: true,
        window: FAST_CANDIDATE_WINDOW,
        start: samples_report.start,
        end: samples_report.end,
        sessions_dir: samples_report.sessions_dir.clone(),
        candidates,
        diagnostics,
    }
}

impl FastCandidateDiagnostics {
    fn increment_reason(&mut self, reason: &'static str) {
        if let Some(row) = self
            .reason_counts
            .iter_mut()
            .find(|row| row.reason == reason)
        {
            row.count += 1;
        } else {
            self.reason_counts
                .push(FastCandidateReasonCount { reason, count: 1 });
        }
    }

    fn finalize_reason_counts(&mut self) {
        self.reason_counts
            .sort_by(|left, right| left.reason.cmp(right.reason));
    }
}

fn normalized_five_hour_samples(
    samples: &[RateLimitSample],
    diagnostics: &mut FastCandidateDiagnostics,
) -> Vec<RateLimitSample> {
    let mut selected = samples
        .iter()
        .filter(|sample| sample.window_minutes == FIVE_HOURS_WINDOW_MINUTES)
        .cloned()
        .collect::<Vec<_>>();
    diagnostics.five_hour_samples = selected.len() as i64;

    selected.sort_by(compare_sample_order);
    let mut seen = BTreeSet::new();
    selected
        .into_iter()
        .filter(|sample| seen.insert(sample_identity(sample)))
        .collect()
}

fn build_candidate_intervals(
    samples: &[RateLimitSample],
    records: &[UsageRecord],
    diagnostics: &mut FastCandidateDiagnostics,
) -> Vec<CandidateInterval> {
    let mut pairs = Vec::new();
    for (key, stream_samples) in samples_by_global_stream(samples) {
        for pair in stream_samples.windows(2) {
            let previous = pair[0];
            let next = pair[1];
            diagnostics.sample_pairs += 1;

            if key.resets_at <= next.timestamp || key.resets_at <= previous.timestamp {
                continue;
            }
            diagnostics.active_sample_pairs += 1;

            let delta_used_percent = next.used_percent - previous.used_percent;
            if delta_used_percent < -PERCENT_EPSILON {
                continue;
            }
            if delta_used_percent > PERCENT_EPSILON {
                diagnostics.rising_sample_pairs += 1;
            }

            let global_pair = GlobalSamplePair {
                stream: key.stream.clone(),
                interval_start: previous.timestamp,
                interval_end: next.timestamp,
                delta_used_percent: delta_used_percent.max(0.0),
                resets_at: key.resets_at,
            };
            pairs.push(global_pair);
        }
    }

    let matches_by_pair = usage_matches_by_global_pair(&pairs, records, diagnostics);
    let mut intervals = Vec::new();
    for (pair_index, pair) in pairs.iter().enumerate() {
        intervals.extend(intervals_for_global_pair(
            pair,
            records,
            matches_by_pair.get(&pair_index).map(Vec::as_slice),
            diagnostics,
        ));
    }

    intervals.sort_by(|left, right| {
        left.interval_end
            .cmp(&right.interval_end)
            .then_with(|| left.interval_start.cmp(&right.interval_start))
            .then_with(|| left.stream.cmp(&right.stream))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    intervals
}

fn intervals_for_global_pair(
    pair: &GlobalSamplePair,
    records: &[UsageRecord],
    matches: Option<&[(usize, UsageMatchKind)]>,
    diagnostics: &mut FastCandidateDiagnostics,
) -> Vec<CandidateInterval> {
    let mut by_session = BTreeMap::<String, Vec<usize>>::new();
    for (record_index, _kind) in matches.unwrap_or(&[]) {
        let record = records
            .get(*record_index)
            .expect("record index comes from records");

        by_session
            .entry(record.session_id.clone())
            .or_default()
            .push(*record_index);
    }

    if by_session.is_empty() {
        if pair.delta_used_percent > PERCENT_EPSILON {
            diagnostics.increment_reason(REASON_NO_USAGE);
        }
        return Vec::new();
    }

    let mut weights = BTreeMap::<String, f64>::new();
    let mut total_weight = 0.0;
    for (session_id, indexes) in &by_session {
        let weight = indexes
            .iter()
            .map(|index| normal_credits_for_record(&records[*index]))
            .sum::<f64>();
        weights.insert(session_id.clone(), weight);
        total_weight += weight;
    }

    if total_weight <= PERCENT_EPSILON {
        total_weight = by_session
            .values()
            .map(|indexes| indexes.len() as f64)
            .sum::<f64>();
        for (session_id, indexes) in &by_session {
            weights.insert(session_id.clone(), indexes.len() as f64);
        }
    }

    by_session
        .into_iter()
        .map(|(session_id, record_indexes)| {
            let share = weights.get(&session_id).copied().unwrap_or_default()
                / total_weight.max(PERCENT_EPSILON);
            CandidateInterval {
                stream: pair.stream.clone(),
                session_id,
                interval_start: pair.interval_start,
                interval_end: pair.interval_end,
                delta_used_percent: pair.delta_used_percent * share,
                resets_at: pair.resets_at,
                record_indexes,
            }
        })
        .collect()
}

fn build_candidate_segments(intervals: Vec<CandidateInterval>) -> Vec<CandidateSegment> {
    let mut intervals_by_segment = BTreeMap::<SegmentKey, Vec<CandidateInterval>>::new();
    for interval in intervals {
        intervals_by_segment
            .entry(SegmentKey {
                stream: interval.stream.clone(),
                session_id: interval.session_id.clone(),
                resets_at: interval.resets_at,
            })
            .or_default()
            .push(interval);
    }

    let mut segments = Vec::new();
    for (key, mut intervals) in intervals_by_segment {
        intervals.sort_by(|left, right| {
            left.interval_start
                .cmp(&right.interval_start)
                .then_with(|| left.interval_end.cmp(&right.interval_end))
        });

        let mut current: Option<CandidateSegment> = None;
        for interval in intervals {
            let has_segment_signal = interval.delta_used_percent > PERCENT_EPSILON
                || !interval.record_indexes.is_empty();
            if !has_segment_signal {
                if let Some(segment) = current.take() {
                    push_segment(&mut segments, segment);
                }
                continue;
            }

            match &mut current {
                Some(segment) => {
                    segment.segment_end = interval.interval_end;
                    segment.delta_used_percent += interval.delta_used_percent;
                    segment.sample_pairs += 1;
                    segment.record_indexes.extend(interval.record_indexes);
                }
                None => {
                    current = Some(CandidateSegment {
                        stream: key.stream.clone(),
                        session_id: key.session_id.clone(),
                        segment_start: interval.interval_start,
                        segment_end: interval.interval_end,
                        delta_used_percent: interval.delta_used_percent,
                        resets_at: key.resets_at,
                        sample_pairs: 1,
                        record_indexes: interval.record_indexes,
                    });
                }
            }
        }

        if let Some(segment) = current {
            push_segment(&mut segments, segment);
        }
    }

    segments.sort_by(|left, right| {
        left.segment_end
            .cmp(&right.segment_end)
            .then_with(|| left.segment_start.cmp(&right.segment_start))
            .then_with(|| left.stream.cmp(&right.stream))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    segments
}

fn push_segment(segments: &mut Vec<CandidateSegment>, mut segment: CandidateSegment) {
    segment.record_indexes.sort_unstable();
    segment.record_indexes.dedup();
    segments.push(segment);
}

fn score_candidate_segments(
    segments: Vec<CandidateSegment>,
    records: &[UsageRecord],
    diagnostics: &mut FastCandidateDiagnostics,
) -> Vec<FastCandidateRow> {
    let mut scored_by_stream = BTreeMap::<StreamKey, Vec<ScoredSegment>>::new();

    for segment in segments {
        let summary = summarize_segment(&segment, records);
        if summary.calls == 0 {
            diagnostics.increment_reason(REASON_NO_USAGE);
            continue;
        }
        diagnostics.segments_with_usage += 1;

        if segment.delta_used_percent <= PERCENT_EPSILON {
            diagnostics.insufficient_segments += 1;
            diagnostics.increment_reason(REASON_NO_RISING_USAGE);
            continue;
        }

        if segment.delta_used_percent <= MIN_SEGMENT_DELTA_USED_PERCENT + PERCENT_EPSILON {
            diagnostics.insufficient_segments += 1;
            diagnostics.increment_reason(REASON_MINIMUM_PERCENT_STEP);
            continue;
        }

        if summary.calls < MIN_SEGMENT_USAGE_CALLS {
            diagnostics.insufficient_segments += 1;
            diagnostics.increment_reason(REASON_TOO_FEW_USAGE_CALLS);
            continue;
        }

        if summary.normal_credits <= PERCENT_EPSILON {
            diagnostics.increment_reason(REASON_NO_PRICED_USAGE);
            continue;
        }

        let expected_fast_multiplier = summary.expected_fast_multiplier();
        if expected_fast_multiplier <= 1.0 + PERCENT_EPSILON {
            diagnostics.increment_reason(REASON_NO_FAST_MULTIPLIER_MODEL);
            continue;
        }
        if summary.is_mixed_model() {
            diagnostics.mixed_model_segments += 1;
        }

        let percent_per_credit = segment.delta_used_percent / summary.normal_credits;
        scored_by_stream
            .entry(segment.stream.clone())
            .or_default()
            .push(ScoredSegment {
                segment,
                summary,
                percent_per_credit,
                expected_fast_multiplier,
            });
    }

    let mut candidates = Vec::new();
    for (_, mut stream_segments) in scored_by_stream {
        stream_segments.sort_by(|left, right| {
            left.percent_per_credit
                .partial_cmp(&right.percent_per_credit)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.segment.segment_end.cmp(&right.segment.segment_end))
        });

        if stream_segments.len() < 2 {
            diagnostics.insufficient_segments += stream_segments.len() as i64;
            for _ in &stream_segments {
                diagnostics.increment_reason(REASON_INSUFFICIENT_BASELINE);
            }
            continue;
        }

        let baseline = low_percentile_baseline(&stream_segments);
        for scored in stream_segments {
            let effective_multiplier = scored.percent_per_credit / baseline;
            let confidence =
                confidence_for_multiplier(effective_multiplier, scored.expected_fast_multiplier);
            if confidence == CONFIDENCE_INSUFFICIENT {
                diagnostics.normal_segments += 1;
                diagnostics.increment_reason(REASON_NORMAL_MULTIPLIER);
                continue;
            }

            diagnostics.candidate_segments += 1;
            let reason = if scored.summary.is_mixed_model() {
                REASON_MIXED_MODELS
            } else {
                REASON_EXPECTED_FAST_MULTIPLIER
            };
            diagnostics.increment_reason(reason);
            candidates.push(candidate_row(
                scored,
                baseline,
                effective_multiplier,
                confidence,
            ));
        }
    }

    candidates.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.account_id.cmp(&right.account_id))
            .then_with(|| left.plan_type.cmp(&right.plan_type))
            .then_with(|| left.limit_id.cmp(&right.limit_id))
            .then_with(|| left.model.cmp(&right.model))
    });
    candidates
}

fn summarize_segment(segment: &CandidateSegment, records: &[UsageRecord]) -> IntervalUsageSummary {
    let mut summary = IntervalUsageSummary::default();
    for index in &segment.record_indexes {
        let record = records
            .get(*index)
            .expect("record index comes from records");
        summary.add(record);
    }
    summary
}

fn normal_credits_for_record(record: &UsageRecord) -> f64 {
    calculate_credit_cost_with_context_at(
        &record.model,
        record.usage.pricing_usage(),
        PricingContext::normal(),
        record.timestamp,
    )
    .credits
}

fn candidate_row(
    scored: ScoredSegment,
    baseline_percent_per_credit: f64,
    effective_multiplier: f64,
    confidence: &'static str,
) -> FastCandidateRow {
    FastCandidateRow {
        timestamp: scored.segment.segment_end,
        segment_start: scored.segment.segment_start,
        segment_end: scored.segment.segment_end,
        session_id: scored.summary.output_session_id(),
        model: scored.summary.dominant_model(),
        file_path: scored.summary.output_file_path(),
        account_id: scored.segment.stream.account_id,
        plan_type: scored.segment.stream.plan_type,
        limit_id: scored.segment.stream.limit_id,
        resets_at: scored.segment.resets_at,
        sample_pairs: scored.segment.sample_pairs,
        calls: scored.summary.calls,
        total_tokens: scored.summary.total_tokens,
        delta_used_percent: round_metric(scored.segment.delta_used_percent),
        normal_credits: round_credits(scored.summary.normal_credits),
        percent_per_credit: round_metric(scored.percent_per_credit),
        baseline_percent_per_credit: round_metric(baseline_percent_per_credit),
        effective_multiplier: round_metric(effective_multiplier),
        expected_fast_multiplier: round_metric(scored.expected_fast_multiplier),
        confidence,
        reason: if scored.summary.is_mixed_model() {
            REASON_MIXED_MODELS
        } else {
            REASON_EXPECTED_FAST_MULTIPLIER
        },
    }
}

fn low_percentile_baseline(segments: &[ScoredSegment]) -> f64 {
    segments
        .first()
        .expect("baseline requires non-empty segments")
        .percent_per_credit
        .max(PERCENT_EPSILON)
}

fn confidence_for_multiplier(
    effective_multiplier: f64,
    expected_fast_multiplier: f64,
) -> &'static str {
    if effective_multiplier < 1.5 || effective_multiplier < expected_fast_multiplier * 0.65 {
        return CONFIDENCE_INSUFFICIENT;
    }
    let distance = (effective_multiplier - expected_fast_multiplier).abs();
    if distance <= 0.25 {
        CONFIDENCE_HIGH
    } else if distance <= 0.5 {
        CONFIDENCE_MEDIUM
    } else {
        CONFIDENCE_LOW
    }
}

fn usage_matches_by_global_pair(
    pairs: &[GlobalSamplePair],
    records: &[UsageRecord],
    diagnostics: &mut FastCandidateDiagnostics,
) -> BTreeMap<usize, Vec<(usize, UsageMatchKind)>> {
    let mut matches = BTreeMap::<usize, Vec<(usize, UsageMatchKind)>>::new();

    for (record_index, record) in records.iter().enumerate() {
        let time_account_matches = pairs
            .iter()
            .enumerate()
            .filter(|(_, pair)| record_time_account_matches_global_pair(pair, record))
            .map(|(pair_index, _)| pair_index)
            .collect::<Vec<_>>();

        if time_account_matches.is_empty() {
            continue;
        }

        if !record.rate_limits.is_empty() {
            let exact_matches = time_account_matches
                .iter()
                .copied()
                .filter(|pair_index| {
                    record_rate_limits_match_global_pair(&pairs[*pair_index], record)
                })
                .collect::<Vec<_>>();
            if !exact_matches.is_empty() {
                for pair_index in exact_matches {
                    diagnostics.exact_usage_matches += 1;
                    matches
                        .entry(pair_index)
                        .or_default()
                        .push((record_index, UsageMatchKind::Exact));
                }
            }
            continue;
        }

        let legacy_matches = non_ambiguous_legacy_pair_indexes(&time_account_matches, pairs);
        if legacy_matches.is_empty() {
            diagnostics.ambiguous_legacy_usage_records += 1;
            continue;
        }

        for pair_index in legacy_matches {
            diagnostics.legacy_usage_matches += 1;
            matches
                .entry(pair_index)
                .or_default()
                .push((record_index, UsageMatchKind::Legacy));
        }
    }

    matches
}

fn record_time_account_matches_global_pair(pair: &GlobalSamplePair, record: &UsageRecord) -> bool {
    record.timestamp > pair.interval_start
        && record.timestamp <= pair.interval_end
        && usage_account_matches(
            pair.stream.account_id.as_deref(),
            record.account_id.as_deref(),
        )
}

fn record_rate_limits_match_global_pair(pair: &GlobalSamplePair, record: &UsageRecord) -> bool {
    record.rate_limits.iter().any(|rate_limit| {
        rate_limit.window_minutes == FIVE_HOURS_WINDOW_MINUTES
            && same_reset(rate_limit.resets_at, pair.resets_at)
            && rate_limit.plan_type.as_deref() == pair.stream.plan_type.as_deref()
            && rate_limit.limit_id.as_deref() == pair.stream.limit_id.as_deref()
    })
}

fn non_ambiguous_legacy_pair_indexes(
    candidate_indexes: &[usize],
    pairs: &[GlobalSamplePair],
) -> Vec<usize> {
    let mut selected = Vec::new();
    for index in candidate_indexes {
        let pair = &pairs[*index];
        let equivalent_candidates = candidate_indexes
            .iter()
            .filter(|candidate_index| {
                let candidate = &pairs[**candidate_index];
                candidate.stream.account_id == pair.stream.account_id
                    && candidate.stream.plan_type == pair.stream.plan_type
            })
            .count();
        if equivalent_candidates == 1 {
            selected.push(*index);
        }
    }
    selected
}

fn usage_account_matches(window_account_id: Option<&str>, record_account_id: Option<&str>) -> bool {
    match record_account_id {
        Some(account_id) => {
            window_account_id.is_none_or(|window_account| window_account == account_id)
        }
        None => window_account_id.is_none(),
    }
}

fn samples_by_global_stream(
    samples: &[RateLimitSample],
) -> BTreeMap<GlobalStreamKey, Vec<SampleObservation>> {
    let mut by_stream = BTreeMap::<GlobalStreamKey, BTreeMap<DateTime<Utc>, f64>>::new();
    for sample in samples {
        by_stream
            .entry(GlobalStreamKey {
                stream: stream_key(sample),
                resets_at: sample.resets_at,
            })
            .or_default()
            .entry(sample.timestamp)
            .and_modify(|used_percent| *used_percent = used_percent.max(sample.used_percent))
            .or_insert(sample.used_percent);
    }

    let mut observations_by_stream = BTreeMap::new();
    for (key, observations) in by_stream {
        observations_by_stream.insert(
            key,
            observations
                .into_iter()
                .map(|(timestamp, used_percent)| SampleObservation {
                    timestamp,
                    used_percent,
                })
                .collect(),
        );
    }
    observations_by_stream
}

fn sample_identity(sample: &RateLimitSample) -> SampleIdentity {
    SampleIdentity {
        stream: stream_key(sample),
        timestamp: sample.timestamp,
        resets_at: sample.resets_at,
        session_id: sample.session_id.clone(),
    }
}

fn stream_key(sample: &RateLimitSample) -> StreamKey {
    StreamKey {
        account_id: sample.account_id.clone(),
        plan_type: sample.plan_type.clone(),
        limit_id: sample.limit_id.clone(),
    }
}

fn compare_sample_order(left: &RateLimitSample, right: &RateLimitSample) -> std::cmp::Ordering {
    left.timestamp
        .cmp(&right.timestamp)
        .then_with(|| left.resets_at.cmp(&right.resets_at))
        .then_with(|| left.session_id.cmp(&right.session_id))
}

fn same_reset(left: DateTime<Utc>, right: DateTime<Utc>) -> bool {
    (right - left).num_seconds().abs() <= RESET_JITTER_TOLERANCE_SECONDS
}

fn single_or_mixed(values: &BTreeSet<String>) -> String {
    if values.len() == 1 {
        values.iter().next().expect("one value").clone()
    } else if values.is_empty() {
        "unknown".to_string()
    } else {
        "mixed".to_string()
    }
}

fn round_metric(value: f64) -> f64 {
    ((value + f64::EPSILON) * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::RateLimitDiagnostics;
    use crate::stats::{TokenUsage, UsageMode, UsageRateLimit};
    use chrono::TimeZone;

    #[test]
    fn fast_candidates_score_session_segments_and_absorb_deferred_percent_changes() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for("normal-54", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:15:00Z", 1.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:30:00Z", 2.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:45:00Z", 3.0, reset_at),
            sample_for("fast-54", "2026-05-10T01:00:00Z", 3.0, reset_at),
            sample_for("fast-54", "2026-05-10T01:15:00Z", 3.0, reset_at),
            sample_for("fast-54", "2026-05-10T01:30:00Z", 3.0, reset_at),
            sample_for("fast-54", "2026-05-10T01:45:00Z", 9.0, reset_at),
            sample_for("fast-55", "2026-05-10T02:00:00Z", 9.0, reset_at),
            sample_for("fast-55", "2026-05-10T02:15:00Z", 9.0, reset_at),
            sample_for("fast-55", "2026-05-10T02:30:00Z", 9.0, reset_at),
            sample_for("fast-55", "2026-05-10T02:45:00Z", 16.5, reset_at),
        ]);
        let usage = usage_report(vec![
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:45:00Z",
                16_000,
                reset_at,
            ),
            record(
                "fast-54",
                "gpt-5.4",
                "2026-05-10T01:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "fast-54",
                "gpt-5.4",
                "2026-05-10T01:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "fast-54",
                "gpt-5.4",
                "2026-05-10T01:45:00Z",
                16_000,
                reset_at,
            ),
            record(
                "fast-55",
                "gpt-5.5",
                "2026-05-10T02:15:00Z",
                8_000,
                reset_at,
            ),
            record(
                "fast-55",
                "gpt-5.5",
                "2026-05-10T02:30:00Z",
                8_000,
                reset_at,
            ),
            record(
                "fast-55",
                "gpt-5.5",
                "2026-05-10T02:45:00Z",
                8_000,
                reset_at,
            ),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.detection_only);
        assert_eq!(report.window, FAST_CANDIDATE_WINDOW);
        assert_eq!(report.candidates.len(), 2);
        assert_eq!(report.diagnostics.five_hour_samples, 12);
        assert_eq!(report.diagnostics.rising_sample_pairs, 5);
        assert_eq!(report.diagnostics.segments_with_usage, 3);
        assert_eq!(report.diagnostics.normal_segments, 1);
        assert_eq!(report.diagnostics.candidate_segments, 2);
        assert!(report
            .candidates
            .iter()
            .all(|candidate| candidate.confidence == CONFIDENCE_HIGH));

        let gpt54 = report
            .candidates
            .iter()
            .find(|candidate| candidate.model == "gpt-5.4")
            .expect("gpt-5.4 candidate");
        assert_eq!(gpt54.calls, 3);
        assert_eq!(gpt54.sample_pairs, 3);
        assert_close(gpt54.normal_credits, 3.0, "gpt-5.4 normal credits");
        assert_close(gpt54.effective_multiplier, 2.0, "gpt-5.4 multiplier");
        assert_close(
            gpt54.expected_fast_multiplier,
            2.0,
            "gpt-5.4 expected multiplier",
        );

        let gpt55 = report
            .candidates
            .iter()
            .find(|candidate| candidate.model == "gpt-5.5")
            .expect("gpt-5.5 candidate");
        assert_eq!(gpt55.calls, 3);
        assert_eq!(gpt55.sample_pairs, 3);
        assert_close(gpt55.normal_credits, 3.0, "gpt-5.5 normal credits");
        assert_close(gpt55.effective_multiplier, 2.5, "gpt-5.5 multiplier");
        assert_close(
            gpt55.expected_fast_multiplier,
            2.5,
            "gpt-5.5 expected multiplier",
        );
    }

    #[test]
    fn fast_candidates_report_no_five_hour_samples_for_7d_only_input() {
        let reset_at = utc(2026, 5, 17, 0, 0);
        let mut samples = samples_report(vec![
            sample("2026-05-10T00:00:00Z", 1.0, reset_at),
            sample("2026-05-10T01:00:00Z", 2.0, reset_at),
        ]);
        for sample in &mut samples.samples {
            sample.window = "7d".to_string();
            sample.window_minutes = 10_080;
        }
        let usage = usage_report(Vec::new());

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.candidates.is_empty());
        assert!(report.diagnostics.no_five_hour_samples);
        assert_eq!(report.diagnostics.five_hour_samples, 0);
        assert_reason_count(&report, REASON_NO_FIVE_HOUR_SAMPLES, 1);
    }

    #[test]
    fn fast_candidates_report_insufficient_baseline_without_failing() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for("single-54", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for("single-54", "2026-05-10T00:15:00Z", 0.0, reset_at),
            sample_for("single-54", "2026-05-10T00:30:00Z", 0.0, reset_at),
            sample_for("single-54", "2026-05-10T00:45:00Z", 6.0, reset_at),
        ]);
        let usage = usage_report(vec![
            record(
                "single-54",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "single-54",
                "gpt-5.4",
                "2026-05-10T00:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "single-54",
                "gpt-5.4",
                "2026-05-10T00:45:00Z",
                16_000,
                reset_at,
            ),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.candidates.is_empty());
        assert_eq!(report.diagnostics.insufficient_segments, 1);
        assert_reason_count(&report, REASON_INSUFFICIENT_BASELINE, 1);
    }

    #[test]
    fn fast_candidates_reject_short_segments_even_when_multiplier_matches() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for("normal-54", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:15:00Z", 1.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:30:00Z", 2.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:45:00Z", 3.0, reset_at),
            sample_for("short-fast-54", "2026-05-10T01:00:00Z", 3.0, reset_at),
            sample_for("short-fast-54", "2026-05-10T01:15:00Z", 3.0, reset_at),
            sample_for("short-fast-54", "2026-05-10T01:30:00Z", 7.0, reset_at),
        ]);
        let usage = usage_report(vec![
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:45:00Z",
                16_000,
                reset_at,
            ),
            record(
                "short-fast-54",
                "gpt-5.4",
                "2026-05-10T01:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "short-fast-54",
                "gpt-5.4",
                "2026-05-10T01:30:00Z",
                16_000,
                reset_at,
            ),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.candidates.is_empty());
        assert_eq!(report.diagnostics.insufficient_segments, 2);
        assert_reason_count(&report, REASON_TOO_FEW_USAGE_CALLS, 1);
        assert_reason_count(&report, REASON_INSUFFICIENT_BASELINE, 1);
    }

    #[test]
    fn fast_candidates_explain_mixed_model_weighted_multiplier() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for("normal-54", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:15:00Z", 1.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:30:00Z", 2.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:45:00Z", 3.0, reset_at),
            sample_for("mixed", "2026-05-10T01:00:00Z", 3.0, reset_at),
            sample_for("mixed", "2026-05-10T01:15:00Z", 3.0, reset_at),
            sample_for("mixed", "2026-05-10T01:30:00Z", 3.0, reset_at),
            sample_for("mixed", "2026-05-10T01:45:00Z", 9.5, reset_at),
        ]);
        let usage = usage_report(vec![
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:45:00Z",
                16_000,
                reset_at,
            ),
            record("mixed", "gpt-5.4", "2026-05-10T01:15:00Z", 16_000, reset_at),
            record("mixed", "gpt-5.5", "2026-05-10T01:30:00Z", 8_000, reset_at),
            record("mixed", "gpt-5.4", "2026-05-10T01:45:00Z", 16_000, reset_at),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert_eq!(report.candidates.len(), 1);
        let candidate = &report.candidates[0];
        assert_eq!(candidate.reason, REASON_MIXED_MODELS);
        assert_eq!(candidate.session_id, "mixed");
        assert_close(candidate.normal_credits, 3.0, "mixed normal credits");
        assert_close(candidate.effective_multiplier, 2.166667, "mixed multiplier");
        assert_close(
            candidate.expected_fast_multiplier,
            2.166667,
            "mixed expected multiplier",
        );
        assert_eq!(candidate.calls, 3);
        assert_eq!(candidate.sample_pairs, 3);
        assert_eq!(report.diagnostics.mixed_model_segments, 1);
        assert_reason_count(&report, REASON_MIXED_MODELS, 1);
    }

    #[test]
    fn fast_candidates_split_concurrent_session_delta_by_credit_share() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for("normal-54", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:15:00Z", 1.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:30:00Z", 2.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:45:00Z", 3.0, reset_at),
            sample_for("parallel-a", "2026-05-10T01:00:00Z", 3.0, reset_at),
            sample_for("parallel-b", "2026-05-10T01:00:00Z", 3.0, reset_at),
            sample_for("parallel-a", "2026-05-10T01:15:00Z", 5.0, reset_at),
            sample_for("parallel-b", "2026-05-10T01:15:00Z", 5.0, reset_at),
            sample_for("parallel-a", "2026-05-10T01:30:00Z", 7.0, reset_at),
            sample_for("parallel-b", "2026-05-10T01:30:00Z", 7.0, reset_at),
            sample_for("parallel-a", "2026-05-10T01:45:00Z", 9.0, reset_at),
            sample_for("parallel-b", "2026-05-10T01:45:00Z", 9.0, reset_at),
        ]);
        let usage = usage_report(vec![
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:45:00Z",
                16_000,
                reset_at,
            ),
            record(
                "parallel-a",
                "gpt-5.4",
                "2026-05-10T01:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "parallel-b",
                "gpt-5.4",
                "2026-05-10T01:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "parallel-a",
                "gpt-5.4",
                "2026-05-10T01:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "parallel-b",
                "gpt-5.4",
                "2026-05-10T01:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "parallel-a",
                "gpt-5.4",
                "2026-05-10T01:45:00Z",
                16_000,
                reset_at,
            ),
            record(
                "parallel-b",
                "gpt-5.4",
                "2026-05-10T01:45:00Z",
                16_000,
                reset_at,
            ),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.candidates.is_empty());
        assert_eq!(report.diagnostics.normal_segments, 3);
        assert_eq!(report.diagnostics.candidate_segments, 0);
    }

    #[test]
    fn fast_candidates_ignore_single_percent_step_segments() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for("normal-54", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:15:00Z", 1.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:30:00Z", 2.0, reset_at),
            sample_for("normal-54", "2026-05-10T00:45:00Z", 3.0, reset_at),
            sample_for("minimum-step", "2026-05-10T01:00:00Z", 3.0, reset_at),
            sample_for("minimum-step", "2026-05-10T01:15:00Z", 3.0, reset_at),
            sample_for("minimum-step", "2026-05-10T01:30:00Z", 3.0, reset_at),
            sample_for("minimum-step", "2026-05-10T01:45:00Z", 4.0, reset_at),
        ]);
        let usage = usage_report(vec![
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "normal-54",
                "gpt-5.4",
                "2026-05-10T00:45:00Z",
                16_000,
                reset_at,
            ),
            record(
                "minimum-step",
                "gpt-5.4",
                "2026-05-10T01:15:00Z",
                16_000,
                reset_at,
            ),
            record(
                "minimum-step",
                "gpt-5.4",
                "2026-05-10T01:30:00Z",
                16_000,
                reset_at,
            ),
            record(
                "minimum-step",
                "gpt-5.4",
                "2026-05-10T01:45:00Z",
                16_000,
                reset_at,
            ),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.candidates.is_empty());
        assert_reason_count(&report, REASON_MINIMUM_PERCENT_STEP, 1);
    }

    #[test]
    fn fast_candidates_reject_ambiguous_legacy_usage_across_streams() {
        let reset_at = utc(2026, 5, 10, 5, 0);
        let samples = samples_report(vec![
            sample_for_limit_id("primary", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for_limit_id("primary", "2026-05-10T00:15:00Z", 2.0, reset_at),
            sample_for_limit_id("primary", "2026-05-10T00:30:00Z", 4.0, reset_at),
            sample_for_limit_id("secondary", "2026-05-10T00:00:00Z", 0.0, reset_at),
            sample_for_limit_id("secondary", "2026-05-10T00:15:00Z", 2.0, reset_at),
            sample_for_limit_id("secondary", "2026-05-10T00:30:00Z", 4.0, reset_at),
        ]);
        let usage = usage_report(vec![
            legacy_record(
                "legacy",
                "gpt-5.4",
                "2026-05-10T00:15:00Z",
                16_000,
                reset_at,
            ),
            record("exact", "gpt-5.4", "2026-05-10T00:30:00Z", 16_000, reset_at),
        ]);

        let report = build_fast_candidate_report(&samples, &usage);

        assert!(report.candidates.is_empty());
        assert_eq!(report.diagnostics.exact_usage_matches, 1);
        assert_eq!(report.diagnostics.legacy_usage_matches, 0);
        assert_eq!(report.diagnostics.ambiguous_legacy_usage_records, 1);
        assert_eq!(report.diagnostics.segments_with_usage, 1);
    }

    #[test]
    fn candidate_pricing_uses_record_timestamp_across_rate_card_cutoff() {
        let reset_at = parse_time("2026-07-30T22:17:05.167Z");
        let old = record(
            "old",
            "gpt-5.6-terra",
            "2026-07-30T17:17:05.166Z",
            1_000_000,
            reset_at,
        );
        let new = record(
            "new",
            "gpt-5.6-terra",
            "2026-07-30T17:17:05.167Z",
            1_000_000,
            reset_at,
        );
        let mut summary = IntervalUsageSummary::default();

        summary.add(&old);
        summary.add(&new);

        assert_close(normal_credits_for_record(&old), 62.5, "old normal credits");
        assert_close(normal_credits_for_record(&new), 50.0, "new normal credits");
        assert_close(summary.normal_credits, 112.5, "combined normal credits");
        assert_close(
            summary.expected_fast_multiplier(),
            2.5,
            "combined fast multiplier",
        );
    }

    #[test]
    fn confidence_thresholds_are_fixed() {
        assert_eq!(confidence_for_multiplier(2.0, 2.0), CONFIDENCE_HIGH);
        assert_eq!(confidence_for_multiplier(1.6, 2.0), CONFIDENCE_MEDIUM);
        assert_eq!(confidence_for_multiplier(2.8, 2.0), CONFIDENCE_LOW);
        assert_eq!(confidence_for_multiplier(1.2, 2.0), CONFIDENCE_INSUFFICIENT);
    }

    fn samples_report(samples: Vec<RateLimitSample>) -> RateLimitSamplesReport {
        RateLimitSamplesReport {
            start: utc(2026, 5, 10, 0, 0),
            end: utc(2026, 5, 10, 6, 0),
            sessions_dir: "/sessions".to_string(),
            samples,
            diagnostics: RateLimitDiagnostics::new(0, false),
        }
    }

    fn usage_report(records: Vec<UsageRecord>) -> UsageRecordsReport {
        UsageRecordsReport {
            start: utc(2026, 5, 10, 0, 0),
            end: utc(2026, 5, 10, 6, 0),
            sessions_dir: "/sessions".to_string(),
            records,
            diagnostics: crate::stats::UsageDiagnostics::new(0, false),
        }
    }

    fn sample(timestamp: &str, used_percent: f64, resets_at: DateTime<Utc>) -> RateLimitSample {
        sample_for("sample-session", timestamp, used_percent, resets_at)
    }

    fn sample_for(
        session_id: &str,
        timestamp: &str,
        used_percent: f64,
        resets_at: DateTime<Utc>,
    ) -> RateLimitSample {
        RateLimitSample {
            timestamp: parse_time(timestamp),
            session_id: session_id.to_string(),
            account_id: Some("account-fixture".to_string()),
            plan_type: Some("pro".to_string()),
            limit_id: Some("primary".to_string()),
            window: FAST_CANDIDATE_WINDOW.to_string(),
            window_minutes: FIVE_HOURS_WINDOW_MINUTES,
            used_percent,
            remaining_percent: 100.0 - used_percent,
            resets_at,
            source: None,
        }
    }

    fn sample_for_limit_id(
        limit_id: &str,
        timestamp: &str,
        used_percent: f64,
        resets_at: DateTime<Utc>,
    ) -> RateLimitSample {
        let mut sample = sample_for(limit_id, timestamp, used_percent, resets_at);
        sample.limit_id = Some(limit_id.to_string());
        sample
    }

    fn record(
        session_id: &str,
        model: &str,
        timestamp: &str,
        input_tokens: i64,
        resets_at: DateTime<Utc>,
    ) -> UsageRecord {
        UsageRecord {
            timestamp: parse_time(timestamp),
            session_id: session_id.to_string(),
            model: model.to_string(),
            usage_mode: UsageMode::Normal,
            reasoning_effort: None,
            cwd: "/workspace/fast-candidate".to_string(),
            account_id: Some("account-fixture".to_string()),
            file_path: format!("/tmp/{session_id}.jsonl"),
            rate_limits: vec![UsageRateLimit {
                plan_type: Some("pro".to_string()),
                limit_id: Some("primary".to_string()),
                window: FAST_CANDIDATE_WINDOW.to_string(),
                window_minutes: FIVE_HOURS_WINDOW_MINUTES,
                resets_at,
            }],
            usage: TokenUsage {
                input_tokens,
                cached_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: input_tokens,
            },
        }
    }

    fn legacy_record(
        session_id: &str,
        model: &str,
        timestamp: &str,
        input_tokens: i64,
        resets_at: DateTime<Utc>,
    ) -> UsageRecord {
        let mut record = record(session_id, model, timestamp, input_tokens, resets_at);
        record.rate_limits.clear();
        record
    }

    fn assert_reason_count(report: &FastCandidateReport, reason: &'static str, expected: i64) {
        let actual = report
            .diagnostics
            .reason_counts
            .iter()
            .find(|row| row.reason == reason)
            .map(|row| row.count)
            .unwrap_or(0);
        assert_eq!(actual, expected, "reason count for {reason}");
    }

    fn assert_close(actual: f64, expected: f64, label: &str) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "{label}: expected {expected}, got {actual}"
        );
    }

    fn parse_time(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("utc time")
    }
}
