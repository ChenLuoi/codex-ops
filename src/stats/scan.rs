use super::cli::ResolvedStatOptions;
use super::events::{parse_usage_json_event, UsageJsonPayload};
use super::reports::{TokenUsage, UsageDiagnostics, UsageMode, UsageRateLimit, UsageRecordView};
use crate::account_history::UsageAccountHistory;
use crate::error::AppError;
use crate::limits::{parse_rate_limit_line, RateLimitLineContext, RateLimitParseDiagnostics};
use crate::session_scan::{
    partition_items_for_workers, prepare_session_scan, resolve_session_file_scan_worker_count,
    session_id_from_path, PreparedSessionFile, SessionScanOptions, DEFAULT_FILE_READ_CONCURRENCY,
    SESSION_READ_BUFFER_SIZE,
};
use crate::storage::path_to_string;
use crate::time::DateRange;
use crate::usage_mode_history::UsageModeHistory;
use chrono::{DateTime, Utc};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::thread;

pub(super) trait UsageRecordAccumulator: Send {
    fn add_record(&mut self, record: UsageRecordView<'_>);
    fn empty_like(&self) -> Self;
    fn merge(&mut self, other: Self);
}

trait UsageRecordSink {
    fn on_record(&mut self, record: UsageRecordView<'_>);
}

impl<F> UsageRecordSink for F
where
    F: for<'a> FnMut(UsageRecordView<'a>),
{
    fn on_record(&mut self, record: UsageRecordView<'_>) {
        self(record);
    }
}

struct AccumulatorRecordSink<'a, A> {
    accumulator: &'a mut A,
}

impl<A> UsageRecordSink for AccumulatorRecordSink<'_, A>
where
    A: UsageRecordAccumulator,
{
    fn on_record(&mut self, record: UsageRecordView<'_>) {
        self.accumulator.add_record(record);
    }
}

struct PreparedUsageScan {
    range: DateRange,
    files: Vec<PreparedSessionFile>,
    diagnostics: UsageDiagnostics,
}

pub(super) fn process_usage_records<F>(
    options: &ResolvedStatOptions,
    mut on_record: F,
) -> Result<UsageDiagnostics, AppError>
where
    F: for<'a> FnMut(UsageRecordView<'a>),
{
    let mut prepared = prepare_usage_scan(options)?;

    for file in &prepared.files {
        let scan_diagnostics = read_usage_records_from_file(
            file,
            prepared.range,
            options.account_history.as_ref(),
            options.usage_mode_history.as_ref(),
            options.account_id.as_deref(),
            &mut on_record,
        )?;
        prepared.diagnostics.merge_file_scan(&scan_diagnostics);
    }

    Ok(prepared.diagnostics)
}

pub(super) fn process_usage_records_parallel<A>(
    options: &ResolvedStatOptions,
    mut accumulator: A,
) -> Result<(A, UsageDiagnostics), AppError>
where
    A: UsageRecordAccumulator,
{
    let mut prepared = prepare_usage_scan(options)?;
    let worker_count = resolve_session_file_scan_worker_count(prepared.files.len())?;

    if worker_count <= 1 {
        let scan_diagnostics = scan_usage_files_into_accumulator(
            &prepared.files,
            prepared.range,
            options.account_history.as_ref(),
            options.usage_mode_history.as_ref(),
            options.account_id.as_deref(),
            &mut accumulator,
        )?;
        prepared.diagnostics.merge_file_scan(&scan_diagnostics);
        return Ok((accumulator, prepared.diagnostics));
    }

    let partitions = partition_items_for_workers(&prepared.files, worker_count);
    let range = prepared.range;
    let account_history = options.account_history.as_ref();
    let usage_mode_history = options.usage_mode_history.as_ref();
    let account_id = options.account_id.as_deref();
    let mut partial_results = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(partitions.len());

        for partition in partitions {
            let mut partial_accumulator = accumulator.empty_like();
            handles.push(scope.spawn(move || {
                let diagnostics = scan_usage_files_into_accumulator(
                    &partition,
                    range,
                    account_history,
                    usage_mode_history,
                    account_id,
                    &mut partial_accumulator,
                )?;
                Ok::<_, AppError>((partial_accumulator, diagnostics))
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let result = handle
                .join()
                .map_err(|_| AppError::new("Rust stat file scan worker panicked."))??;
            results.push(result);
        }
        Ok::<_, AppError>(results)
    })?;

    for (partial_accumulator, diagnostics) in partial_results.drain(..) {
        prepared.diagnostics.merge_file_scan(&diagnostics);
        accumulator.merge(partial_accumulator);
    }

    Ok((accumulator, prepared.diagnostics))
}

fn prepare_usage_scan(options: &ResolvedStatOptions) -> Result<PreparedUsageScan, AppError> {
    let range = DateRange {
        start: options.start,
        end: options.end,
    };
    let mut diagnostics =
        UsageDiagnostics::new(DEFAULT_FILE_READ_CONCURRENCY, options.scan_all_files);
    diagnostics.mode_history = options.usage_mode_history_file.as_ref().map(|path| {
        super::reports::UsageModeHistoryDiagnostics {
            history_file: if options.usage_mode_history_include_path {
                Some(path_to_string(path))
            } else {
                None
            },
            history_present: options.usage_mode_history_present,
            switch_count: options.usage_mode_history_switch_count,
            fast_attributed_calls: 0,
            fast_attributed_credits: 0.0,
        }
    });
    let prepared = prepare_session_scan(
        SessionScanOptions {
            sessions_dir: &options.sessions_dir,
            range,
        },
        read_last_token_count_timestamp,
    )?;
    diagnostics.merge_session_scan(&prepared.diagnostics);

    Ok(PreparedUsageScan {
        range,
        files: prepared.files,
        diagnostics,
    })
}

fn scan_usage_files_into_accumulator<A>(
    files: &[PreparedSessionFile],
    range: DateRange,
    account_history: Option<&UsageAccountHistory>,
    usage_mode_history: Option<&UsageModeHistory>,
    account_id: Option<&str>,
    accumulator: &mut A,
) -> Result<UsageDiagnostics, AppError>
where
    A: UsageRecordAccumulator,
{
    let mut diagnostics = UsageDiagnostics::new(0, false);

    for file in files {
        let mut sink = AccumulatorRecordSink {
            accumulator: &mut *accumulator,
        };
        let scan_diagnostics = read_usage_records_from_file(
            file,
            range,
            account_history,
            usage_mode_history,
            account_id,
            &mut sink,
        )?;
        diagnostics.merge_file_scan(&scan_diagnostics);
    }

    Ok(diagnostics)
}

fn read_last_token_count_timestamp(path: &Path) -> Result<Option<DateTime<Utc>>, AppError> {
    let mut file = File::open(path).map_err(|error| AppError::new(error.to_string()))?;
    let mut position = file
        .seek(SeekFrom::End(0))
        .map_err(|error| AppError::new(error.to_string()))?;
    let mut buffer = vec![0_u8; SESSION_READ_BUFFER_SIZE];
    let mut carry = Vec::new();

    while position > 0 {
        let read_len = (position as usize).min(buffer.len());
        position -= read_len as u64;
        file.seek(SeekFrom::Start(position))
            .map_err(|error| AppError::new(error.to_string()))?;
        file.read_exact(&mut buffer[..read_len])
            .map_err(|error| AppError::new(error.to_string()))?;

        let mut combined = Vec::with_capacity(read_len + carry.len());
        combined.extend_from_slice(&buffer[..read_len]);
        combined.extend_from_slice(&carry);

        if position > 0 {
            let Some(newline_index) = combined.iter().position(|byte| *byte == b'\n') else {
                carry = combined;
                continue;
            };

            if let Some(timestamp) = last_token_count_timestamp_in_lines(
                combined
                    .get(newline_index + 1..)
                    .expect("newline index is within combined bytes"),
            ) {
                return Ok(Some(timestamp));
            }

            carry.clear();
            carry.extend_from_slice(
                combined
                    .get(..newline_index)
                    .expect("newline index is within combined bytes"),
            );
        } else if let Some(timestamp) = last_token_count_timestamp_in_lines(&combined) {
            return Ok(Some(timestamp));
        }
    }

    Ok(None)
}

fn last_token_count_timestamp_in_lines(bytes: &[u8]) -> Option<DateTime<Utc>> {
    for line in bytes.split(|byte| *byte == b'\n').rev() {
        let line = trim_line_end_bytes(line);
        if line.is_empty() || !line_contains_bytes(line, b"\"token_count\"") {
            continue;
        };

        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(event) = parse_usage_json_event(line) else {
            continue;
        };
        let Some(event) = event else {
            continue;
        };
        if event.event_type() == Some("event_msg")
            && event.payload().and_then(UsageJsonPayload::payload_type) == Some("token_count")
        {
            if let Some(timestamp) = event.timestamp() {
                return Some(timestamp);
            }
        }
    }

    None
}

fn trim_line_end_bytes(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn line_contains_bytes(line: &[u8], needle: &[u8]) -> bool {
    line.windows(needle.len()).any(|window| window == needle)
}

fn read_usage_records_from_file<F>(
    usage_file: &PreparedSessionFile,
    range: DateRange,
    account_history: Option<&UsageAccountHistory>,
    usage_mode_history: Option<&UsageModeHistory>,
    account_id_filter: Option<&str>,
    on_record: &mut F,
) -> Result<UsageDiagnostics, AppError>
where
    F: UsageRecordSink + ?Sized,
{
    let path = &usage_file.path;
    let file_handle = File::open(path).map_err(|error| AppError::new(error.to_string()))?;
    let mut reader = BufReader::with_capacity(SESSION_READ_BUFFER_SIZE, file_handle);
    let mut line = String::new();
    let mut diagnostics = UsageDiagnostics::new(0, false);
    let mut session_id = usage_file
        .current_session_id
        .clone()
        .unwrap_or_else(|| session_id_from_path(path));
    let mut model = String::from("unknown");
    let mut reasoning_effort: Option<String> = None;
    let mut cwd = String::from("unknown");
    let mut previous_total: Option<TokenUsage> = None;
    let file_path = path_to_string(path);
    let mut line_number = 0_usize;

    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|error| AppError::new(error.to_string()))?;
        if bytes_read == 0 {
            break;
        }

        line_number += 1;
        diagnostics.read_lines += 1;

        if !line.contains("\"token_count\"")
            && !line.contains("\"session_meta\"")
            && !line.contains("\"turn_context\"")
        {
            continue;
        }

        let event = match parse_usage_json_event(&line) {
            Ok(value) => value,
            Err(_) => {
                diagnostics.invalid_json_lines += 1;
                continue;
            }
        };
        let Some(event) = event else {
            continue;
        };

        let event_type = event.event_type();
        let is_fork_replay_line = usage_file.replay_prefix_lines > 0
            && line_number > 1
            && line_number <= usage_file.replay_prefix_lines;
        if event_type == Some("session_meta") {
            if is_fork_replay_line {
                continue;
            }
            if let Some(payload) = event.payload() {
                if usage_file.current_session_id.is_none() {
                    if let Some(id) = payload.id() {
                        session_id = id.to_string();
                    }
                }
                if let Some(next_model) = payload.model() {
                    model = next_model.to_string();
                }
                if let Some(next_effort) = payload.reasoning_effort() {
                    reasoning_effort = Some(next_effort.to_string());
                }
                if let Some(next_cwd) = payload.cwd() {
                    cwd = next_cwd.to_string();
                }
            }
            continue;
        }

        if event_type == Some("turn_context") {
            if is_fork_replay_line {
                continue;
            }
            if let Some(payload) = event.payload() {
                if let Some(next_model) = payload.model() {
                    model = next_model.to_string();
                }
                if let Some(next_effort) = payload.reasoning_effort() {
                    reasoning_effort = Some(next_effort.to_string());
                }
                if let Some(next_cwd) = payload.cwd() {
                    cwd = next_cwd.to_string();
                }
            }
            continue;
        }

        let Some(payload) = event.payload() else {
            continue;
        };

        if event_type != Some("event_msg") || payload.payload_type() != Some("token_count") {
            continue;
        }

        diagnostics.token_count_events += 1;
        let timestamp = event.timestamp();
        let info = payload.info();

        if is_fork_replay_line {
            if let Some(info) = info {
                if let Some(total_usage) = info.total_token_usage() {
                    previous_total = Some(total_usage);
                }
            }
            diagnostics.skipped_events.fork_replay += 1;
            continue;
        }

        let Some(info) = info else {
            diagnostics.skipped_events.missing_metadata += 1;
            continue;
        };

        let total_usage = info.total_token_usage();
        let Some(timestamp) = timestamp else {
            if let Some(total_usage) = total_usage {
                previous_total = Some(total_usage);
            }
            diagnostics.skipped_events.missing_metadata += 1;
            continue;
        };

        let usage = diff_usage(total_usage.as_ref(), previous_total.as_ref())
            .or_else(|| info.last_token_usage());

        if let Some(total_usage) = total_usage {
            previous_total = Some(total_usage);
        }

        let Some(usage) = usage else {
            diagnostics.skipped_events.missing_usage += 1;
            continue;
        };

        if usage.is_empty() {
            diagnostics.skipped_events.empty_usage += 1;
            continue;
        }

        if timestamp < range.start || timestamp > range.end {
            diagnostics.skipped_events.out_of_range += 1;
            continue;
        }

        let account_id = resolve_usage_account_id(timestamp, account_history);
        if let Some(filter) = account_id_filter {
            if account_id.as_deref() != Some(filter) {
                diagnostics.skipped_events.account_mismatch += 1;
                continue;
            }
        }

        diagnostics.included_usage_events += 1;
        let rate_limits = usage_rate_limits_from_line(&line, &session_id, account_id.as_deref());
        let usage_mode = resolve_usage_mode(timestamp, usage_mode_history);
        let record = UsageRecordView {
            timestamp,
            session_id: &session_id,
            model: &model,
            usage_mode,
            reasoning_effort: reasoning_effort.as_deref(),
            cwd: &cwd,
            account_id: account_id.as_deref(),
            file_path: &file_path,
            rate_limits: &rate_limits,
            usage: &usage,
        };
        on_record.on_record(record);
    }

    Ok(diagnostics)
}

fn usage_rate_limits_from_line(
    line: &str,
    session_id: &str,
    account_id: Option<&str>,
) -> Vec<UsageRateLimit> {
    if !line.contains("\"rate_limits\"") {
        return Vec::new();
    }

    let mut diagnostics = RateLimitParseDiagnostics::default();
    parse_rate_limit_line(
        line,
        RateLimitLineContext {
            session_id,
            account_id,
            source: None,
        },
        &mut diagnostics,
    )
    .into_iter()
    .map(|sample| UsageRateLimit {
        plan_type: sample.plan_type,
        limit_id: sample.limit_id,
        window: sample.window,
        window_minutes: sample.window_minutes,
        resets_at: sample.resets_at,
    })
    .collect()
}

fn resolve_usage_account_id(
    timestamp: DateTime<Utc>,
    history: Option<&UsageAccountHistory>,
) -> Option<String> {
    history.and_then(|history| history.account_id_at(timestamp))
}

fn resolve_usage_mode(timestamp: DateTime<Utc>, history: Option<&UsageModeHistory>) -> UsageMode {
    UsageMode::from_fast(history.and_then(|history| history.fast_at(timestamp)))
}

fn diff_usage(current: Option<&TokenUsage>, previous: Option<&TokenUsage>) -> Option<TokenUsage> {
    let current = current?;
    let Some(previous) = previous else {
        return Some(current.clone());
    };

    Some(TokenUsage {
        input_tokens: (current.input_tokens - previous.input_tokens).max(0),
        cached_input_tokens: (current.cached_input_tokens - previous.cached_input_tokens).max(0),
        output_tokens: (current.output_tokens - previous.output_tokens).max(0),
        reasoning_output_tokens: (current.reasoning_output_tokens
            - previous.reasoning_output_tokens)
            .max(0),
        total_tokens: (current.total_tokens - previous.total_tokens).max(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{read_usage_records_report, UsageRecord, UsageRecordsReadOptions};
    use chrono::TimeZone;
    use serde_json::Value;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[test]
    fn usage_scan_includes_flat_archived_sessions_sibling() {
        let temp = TempDir::new().expect("tempdir");
        let codex_home = temp.path().join("codex-home");
        let sessions_dir = codex_home.join("sessions");
        let archived_dir = codex_home.join("archived_sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let archived_file = write_flat_session_file(
            &archived_dir,
            "rollout-2026-05-21T01-00-00-archived-session.jsonl",
            &[token_count_line("2026-05-21T01:00:01.000Z")],
        );

        let report = read_usage_records_report(&UsageRecordsReadOptions {
            start: utc_time(2026, 5, 21, 0),
            end: utc_time(2026, 5, 21, 2),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            usage_mode_history_file: None,
            account_id: None,
        })
        .expect("read usage report");

        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].file_path, path_to_string(archived_file));
        assert_eq!(report.records[0].usage.total_tokens, 2);
        assert_eq!(report.diagnostics.read_files, 1);
    }

    #[test]
    fn usage_scan_reads_old_in_range_usage_without_full_scan() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        write_session_file(
            &sessions_dir,
            2020,
            1,
            1,
            "rollout-2020-01-01T00-00-00-long-lived-session.jsonl",
            &[token_count_line("2999-01-01T00:00:01.000Z")],
        );

        let read = |scan_all_files| {
            read_usage_records_report(&UsageRecordsReadOptions {
                start: utc_time(2999, 1, 1, 0),
                end: utc_time(2999, 1, 1, 1),
                sessions_dir: sessions_dir.clone(),
                scan_all_files,
                account_history_file: None,
                usage_mode_history_file: None,
                account_id: None,
            })
            .expect("read usage report")
        };

        let default_report = read(false);
        let compatibility_report = read(true);

        assert_eq!(default_report.records.len(), 1);
        assert_eq!(default_report.records[0].usage.total_tokens, 2);
        assert_eq!(default_report.diagnostics.tail_read_files, 1);
        assert_eq!(default_report.diagnostics.tail_read_hits, 1);
        assert_eq!(default_report.diagnostics.read_files, 1);
        assert_eq!(
            default_report.records.len(),
            compatibility_report.records.len()
        );
        assert_eq!(
            default_report.records[0].usage.total_tokens,
            compatibility_report.records[0].usage.total_tokens
        );
        assert_eq!(
            default_report.diagnostics.included_usage_events,
            compatibility_report.diagnostics.included_usage_events
        );
    }

    #[test]
    fn usage_scan_captures_rate_limits_from_token_count_line() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            "rollout-2026-05-21T00-00-00-token-count-rate-limits.jsonl",
            &[
                session_meta_line("2026-05-21T00:00:00.000Z", "session-with-limits"),
                token_count_with_rate_limits_line("2026-05-21T00:00:01.000Z"),
            ],
        );

        let report = read_usage_records_report(&UsageRecordsReadOptions {
            start: utc_time(2026, 5, 21, 0),
            end: utc_time(2026, 5, 21, 1),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            usage_mode_history_file: None,
            account_id: None,
        })
        .expect("read usage report");

        assert_eq!(report.records.len(), 1);
        let rate_limits = &report.records[0].rate_limits;
        assert_eq!(rate_limits.len(), 2);
        assert_eq!(rate_limits[0].window, "5h");
        assert_eq!(rate_limits[0].limit_id.as_deref(), Some("token-line-limit"));
        assert_eq!(rate_limits[1].window, "7d");
    }

    #[test]
    fn account_filter_without_history_does_not_include_unattributed_usage() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            "rollout-2026-05-21T00-00-00-account-filter.jsonl",
            &[token_count_line("2026-05-21T00:00:01.000Z")],
        );

        let report = read_usage_records_report(&UsageRecordsReadOptions {
            start: utc_time(2026, 5, 21, 0),
            end: utc_time(2026, 5, 21, 1),
            sessions_dir,
            scan_all_files: false,
            account_history_file: Some(temp.path().join("missing-history.json")),
            usage_mode_history_file: None,
            account_id: Some("account-fixture".to_string()),
        })
        .expect("read usage report");

        assert!(report.records.is_empty());
        assert_eq!(report.diagnostics.included_usage_events, 0);
        assert_eq!(report.diagnostics.skipped_events.account_mismatch, 1);
    }

    #[test]
    fn usage_scan_deduplicates_recursive_and_sibling_fork_replay() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        let session_a = "019e48e7-545e-7253-a8aa-cbd9fec62ce3";
        let session_b = "019e48e8-13cb-7242-9e25-744399563084";
        let session_c = "019e48ee-61c3-7f52-9a56-97e7543a0fdc";
        let session_d = "019e48ee-sibling-7253-a8aa-cbd9fec62ce3";

        let session_a_lines = vec![
            session_meta_line("2026-05-21T00:00:00.000Z", session_a),
            token_count_total_line("2026-05-21T00:00:01.000Z", 10, 10),
            token_count_total_line("2026-05-21T00:00:02.000Z", 10, 20),
        ];
        let mut session_b_lines = vec![session_meta_line("2026-05-21T00:10:00.000Z", session_b)];
        session_b_lines.extend(retimestamp_lines(
            &session_a_lines,
            "2026-05-21T00:10:00.001Z",
        ));
        session_b_lines.push(token_count_total_line("2026-05-21T00:10:01.000Z", 20, 20));
        session_b_lines.push(token_count_total_line("2026-05-21T00:10:02.000Z", 15, 35));

        let mut session_c_lines = vec![session_meta_line("2026-05-21T00:20:00.000Z", session_c)];
        session_c_lines.extend(retimestamp_lines(
            &session_b_lines,
            "2026-05-21T00:20:00.001Z",
        ));
        session_c_lines.push(token_count_total_line("2026-05-21T00:20:01.000Z", 35, 35));
        session_c_lines.push(token_count_total_line("2026-05-21T00:20:02.000Z", 7, 42));

        let mut session_d_lines = vec![session_meta_line("2026-05-21T00:30:00.000Z", session_d)];
        session_d_lines.extend(retimestamp_lines(
            &session_b_lines,
            "2026-05-21T00:30:00.001Z",
        ));
        session_d_lines.push(token_count_total_line("2026-05-21T00:30:01.000Z", 35, 35));
        session_d_lines.push(token_count_total_line("2026-05-21T00:30:02.000Z", 4, 39));

        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-00-00-{session_a}.jsonl"),
            &session_a_lines,
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-10-00-{session_b}.jsonl"),
            &session_b_lines,
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-20-00-{session_c}.jsonl"),
            &session_c_lines,
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-30-00-{session_d}.jsonl"),
            &session_d_lines,
        );

        let report = read_usage_records_report(&UsageRecordsReadOptions {
            start: utc_time(2026, 5, 21, 0),
            end: utc_time(2026, 5, 21, 1),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            usage_mode_history_file: None,
            account_id: None,
        })
        .expect("read usage report");

        assert_eq!(usage_total_for_session(&report.records, session_a), 20);
        assert_eq!(usage_total_for_session(&report.records, session_b), 15);
        assert_eq!(usage_total_for_session(&report.records, session_c), 7);
        assert_eq!(usage_total_for_session(&report.records, session_d), 4);
        assert_eq!(
            report
                .records
                .iter()
                .map(|record| record.usage.total_tokens)
                .sum::<i64>(),
            46
        );
        assert_eq!(report.diagnostics.fork_files, 3);
        assert_eq!(report.diagnostics.fork_parent_missing, 0);
        assert_eq!(report.diagnostics.fork_replay_lines, 15);
        assert_eq!(report.diagnostics.skipped_events.fork_replay, 10);
        assert_eq!(report.diagnostics.skipped_events.empty_usage, 3);
    }

    #[test]
    fn usage_scan_advances_total_for_timestampless_token_count() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        let session_id = "019e50f5-0e02-78c7-a834-df3ad725e25f";

        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-00-00-{session_id}.jsonl"),
            &[
                session_meta_line("2026-05-21T00:00:00.000Z", session_id),
                token_count_total_line("2026-05-21T00:00:01.000Z", 10, 10),
                token_count_total_line_without_timestamp(20, 30),
                token_count_total_line("2026-05-21T00:00:03.000Z", 15, 45),
            ],
        );

        let report = read_usage_records_report(&UsageRecordsReadOptions {
            start: utc_time(2026, 5, 21, 0),
            end: utc_time(2026, 5, 21, 1),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            usage_mode_history_file: None,
            account_id: None,
        })
        .expect("read usage report");

        let session_usage = report
            .records
            .iter()
            .filter(|record| record.session_id == session_id)
            .map(|record| record.usage.total_tokens)
            .collect::<Vec<_>>();

        assert_eq!(session_usage, vec![10, 15]);
        assert_eq!(usage_total_for_session(&report.records, session_id), 25);
        assert_eq!(report.diagnostics.skipped_events.missing_metadata, 1);
    }

    #[test]
    fn usage_scan_ignores_fork_replayed_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        let parent_session = "019e5110-1f92-7280-b02a-0876af32b81f";
        let child_session = "019e5111-2720-73d0-8519-4c80dffbe80e";

        let parent_lines = vec![
            session_meta_line_with_metadata(
                "2026-05-21T00:00:00.000Z",
                parent_session,
                "parent-session-model",
                "/parent-session",
            ),
            turn_context_line(
                "2026-05-21T00:00:00.500Z",
                "parent-context-model",
                "/parent-context",
            ),
            token_count_total_line("2026-05-21T00:00:01.000Z", 10, 10),
        ];
        let mut child_lines = vec![session_meta_line_with_metadata(
            "2026-05-21T00:10:00.000Z",
            child_session,
            "child-model",
            "/child",
        )];
        child_lines.extend(retimestamp_lines(&parent_lines, "2026-05-21T00:10:00.001Z"));
        child_lines.push(token_count_total_line("2026-05-21T00:10:01.000Z", 15, 25));

        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-00-00-{parent_session}.jsonl"),
            &parent_lines,
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-10-00-{child_session}.jsonl"),
            &child_lines,
        );

        let report = read_usage_records_report(&UsageRecordsReadOptions {
            start: utc_time(2026, 5, 21, 0),
            end: utc_time(2026, 5, 21, 1),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            usage_mode_history_file: None,
            account_id: None,
        })
        .expect("read usage report");

        let child_records = report
            .records
            .iter()
            .filter(|record| record.session_id == child_session)
            .collect::<Vec<_>>();

        assert_eq!(child_records.len(), 1);
        assert_eq!(child_records[0].usage.total_tokens, 15);
        assert_eq!(child_records[0].model, "child-model");
        assert_eq!(child_records[0].cwd, "/child");
        assert_eq!(report.diagnostics.skipped_events.fork_replay, 1);
    }

    fn write_session_file(
        root: &Path,
        year: i32,
        month: u32,
        day: u32,
        file_name: &str,
        lines: &[String],
    ) -> PathBuf {
        let dir = root
            .join(format!("{year:04}"))
            .join(format!("{month:02}"))
            .join(format!("{day:02}"));
        std::fs::create_dir_all(&dir).expect("create session dir");
        let path = dir.join(file_name);
        let mut file = std::fs::File::create(&path).expect("create session file");
        for line in lines {
            writeln!(file, "{line}").expect("write session line");
        }
        path
    }

    fn write_flat_session_file(root: &Path, file_name: &str, lines: &[String]) -> PathBuf {
        std::fs::create_dir_all(root).expect("create session dir");
        let path = root.join(file_name);
        let mut file = std::fs::File::create(&path).expect("create session file");
        for line in lines {
            writeln!(file, "{line}").expect("write session line");
        }
        path
    }

    fn usage_total_for_session(records: &[UsageRecord], session_id: &str) -> i64 {
        records
            .iter()
            .filter(|record| record.session_id == session_id)
            .map(|record| record.usage.total_tokens)
            .sum()
    }

    fn session_meta_line(timestamp: &str, session_id: &str) -> String {
        session_meta_line_with_metadata(timestamp, session_id, "gpt-5.5", "/workspace/fork-test")
    }

    fn session_meta_line_with_metadata(
        timestamp: &str,
        session_id: &str,
        model: &str,
        cwd: &str,
    ) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "model": model,
                "cwd": cwd
            }
        })
        .to_string()
    }

    fn turn_context_line(timestamp: &str, model: &str, cwd: &str) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "turn_context",
            "payload": {
                "model": model,
                "cwd": cwd
            }
        })
        .to_string()
    }

    fn token_count_line(timestamp: &str) -> String {
        format!(
            r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1,"reasoning_output_tokens":0,"total_tokens":2}}}}}}}}"#
        )
    }

    fn token_count_with_rate_limits_line(timestamp: &str) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 1,
                        "cached_input_tokens": 0,
                        "output_tokens": 1,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 2
                    }
                },
                "rate_limits": {
                    "primary": {
                        "window_minutes": 300,
                        "used_percent": 10.0,
                        "resets_at": 1779343200
                    },
                    "secondary": {
                        "window_minutes": 10080,
                        "used_percent": 20.0,
                        "resets_at": 1779948000
                    },
                    "plan_type": "pro",
                    "limit_id": "token-line-limit"
                }
            }
        })
        .to_string()
    }

    fn token_count_total_line(timestamp: &str, last_total: i64, total: i64) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": last_total,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": last_total
                    },
                    "total_token_usage": {
                        "input_tokens": total,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": total
                    }
                }
            }
        })
        .to_string()
    }

    fn token_count_total_line_without_timestamp(last_total: i64, total: i64) -> String {
        serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": last_total,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": last_total
                    },
                    "total_token_usage": {
                        "input_tokens": total,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": total
                    }
                }
            }
        })
        .to_string()
    }

    fn retimestamp_lines(lines: &[String], timestamp: &str) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                let mut value = serde_json::from_str::<Value>(line).expect("json line");
                if let Value::Object(fields) = &mut value {
                    fields.insert(
                        "timestamp".to_string(),
                        Value::String(timestamp.to_string()),
                    );
                }
                serde_json::to_string(&value).expect("json string")
            })
            .collect()
    }

    fn utc_time(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .expect("utc time")
    }
}
