use super::events::{parse_rate_limit_line, RateLimitLineContext};
use super::reports::{
    RateLimitDiagnostics, RateLimitParseDiagnostics, RateLimitSample, RateLimitSamplesReadOptions,
    RateLimitSamplesReport, SourceSpan,
};
use crate::account_history::{self, UsageAccountHistory};
use crate::error::AppError;
use crate::session_scan::{
    partition_items_for_workers, prepare_session_scan, resolve_session_file_scan_worker_count,
    session_id_from_path, PreparedSessionFile, SessionScanDiagnostics, SessionScanOptions,
    SESSION_READ_BUFFER_SIZE,
};
use crate::storage::path_to_string;
use crate::time::DateRange;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::thread;

const RATE_LIMIT_FILE_READ_CONCURRENCY: i64 = 1;

pub fn read_rate_limit_samples_report(
    options: &RateLimitSamplesReadOptions,
) -> Result<RateLimitSamplesReport, AppError> {
    let account_history = match &options.account_history_file {
        Some(path) => account_history::read_optional_usage_account_history(path)?,
        None => None,
    };
    let range = DateRange {
        start: options.start,
        end: options.end,
    };
    let mut diagnostics =
        RateLimitDiagnostics::new(RATE_LIMIT_FILE_READ_CONCURRENCY, options.scan_all_files);
    let prepared = prepare_session_scan(
        SessionScanOptions {
            sessions_dir: &options.sessions_dir,
            range,
        },
        read_last_rate_limit_timestamp,
    )?;
    merge_session_scan(&mut diagnostics, &prepared.diagnostics);

    let worker_count = resolve_session_file_scan_worker_count(prepared.files.len())?;
    diagnostics.file_read_concurrency = worker_count as i64;
    let mut samples = read_rate_limit_samples_from_files(
        &prepared.files,
        range,
        account_history.as_ref(),
        options,
        worker_count,
        &mut diagnostics,
    )?;

    samples.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| left.window_minutes.cmp(&right.window_minutes))
            .then_with(|| left.limit_id.cmp(&right.limit_id))
    });

    Ok(RateLimitSamplesReport {
        start: options.start,
        end: options.end,
        sessions_dir: path_to_string(&options.sessions_dir),
        samples,
        diagnostics,
    })
}

fn read_rate_limit_samples_from_files(
    files: &[PreparedSessionFile],
    range: DateRange,
    account_history: Option<&UsageAccountHistory>,
    options: &RateLimitSamplesReadOptions,
    worker_count: usize,
    diagnostics: &mut RateLimitDiagnostics,
) -> Result<Vec<RateLimitSample>, AppError> {
    if worker_count <= 1 {
        let (samples, file_diagnostics) =
            scan_rate_limit_files(files, range, account_history, options)?;
        diagnostics.merge_file_scan(&file_diagnostics);
        return Ok(samples);
    }

    let partitions = partition_items_for_workers(files, worker_count);
    let mut partial_results =
        thread::scope(|scope| {
            let mut handles = Vec::with_capacity(partitions.len());

            for partition in partitions {
                handles.push(scope.spawn(move || {
                    scan_rate_limit_files(&partition, range, account_history, options)
                }));
            }

            let mut results = Vec::with_capacity(handles.len());
            for handle in handles {
                let result = handle
                    .join()
                    .map_err(|_| AppError::new("Rust limit file scan worker panicked."))??;
                results.push(result);
            }
            Ok::<_, AppError>(results)
        })?;

    let mut samples = Vec::new();
    for (mut partial_samples, file_diagnostics) in partial_results.drain(..) {
        diagnostics.merge_file_scan(&file_diagnostics);
        samples.append(&mut partial_samples);
    }
    Ok(samples)
}

fn scan_rate_limit_files(
    files: &[PreparedSessionFile],
    range: DateRange,
    account_history: Option<&UsageAccountHistory>,
    options: &RateLimitSamplesReadOptions,
) -> Result<(Vec<RateLimitSample>, RateLimitDiagnostics), AppError> {
    let mut diagnostics = RateLimitDiagnostics::default();
    let mut samples = Vec::new();
    for file in files {
        let mut file_samples = read_rate_limit_samples_from_file(
            file,
            range,
            account_history,
            options,
            &mut diagnostics,
        )?;
        samples.append(&mut file_samples);
    }
    Ok((samples, diagnostics))
}

fn merge_session_scan(diagnostics: &mut RateLimitDiagnostics, session: &SessionScanDiagnostics) {
    diagnostics.scanned_directories += session.scanned_directories;
    diagnostics.skipped_directories += session.skipped_directories;
    diagnostics.read_files += session.read_files;
    diagnostics.skipped_files += session.skipped_files;
    diagnostics.prefiltered_files += session.prefiltered_files;
    diagnostics.tail_read_files += session.tail_read_files;
    diagnostics.tail_read_hits += session.tail_read_hits;
    diagnostics.mtime_read_files += session.mtime_read_files;
    diagnostics.mtime_tail_hits += session.mtime_tail_hits;
    diagnostics.mtime_read_hits += session.mtime_read_hits;
    diagnostics.fork_files += session.fork_files;
    diagnostics.fork_parent_missing += session.fork_parent_missing;
    diagnostics.fork_replay_lines += session.fork_replay_lines;
}

fn read_rate_limit_samples_from_file(
    session_file: &PreparedSessionFile,
    range: DateRange,
    account_history: Option<&UsageAccountHistory>,
    options: &RateLimitSamplesReadOptions,
    diagnostics: &mut RateLimitDiagnostics,
) -> Result<Vec<RateLimitSample>, AppError> {
    let path = &session_file.path;
    let file_handle = File::open(path).map_err(|error| AppError::new(error.to_string()))?;
    let mut reader = BufReader::with_capacity(SESSION_READ_BUFFER_SIZE, file_handle);
    let mut line = String::new();
    let mut line_number = 0_usize;
    let mut session_id = session_file
        .current_session_id
        .clone()
        .unwrap_or_else(|| session_id_from_path(path));
    let file_path = path_to_string(path);
    let mut file_has_rate_limits = false;
    let mut file_has_token_count = false;
    let mut samples = Vec::new();

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
        if line.contains("\"token_count\"") {
            file_has_token_count = true;
        }

        if !line.contains("\"rate_limits\"")
            && !line.contains("\"session_meta\"")
            && !line.contains("\"turn_context\"")
        {
            continue;
        }

        let is_fork_replay_line = session_file.replay_prefix_lines > 0
            && line_number > 1
            && line_number <= session_file.replay_prefix_lines;
        if is_fork_replay_line {
            diagnostics.fork_replay_lines_skipped += 1;
            continue;
        }

        if line.contains("\"session_meta\"") || line.contains("\"turn_context\"") {
            match metadata_event(&line) {
                Ok(Some(metadata)) => {
                    if metadata.event_type == "session_meta"
                        && session_file.current_session_id.is_none()
                    {
                        if let Some(next_session_id) = metadata.session_id {
                            session_id = next_session_id;
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => diagnostics.invalid_json_lines += 1,
            }
        }

        if !line.contains("\"rate_limits\"") {
            continue;
        }
        file_has_rate_limits = true;
        let account_id = resolve_rate_limit_account_id(timestamp_from_line(&line), account_history);
        let mut parse_diagnostics = RateLimitParseDiagnostics::default();
        let mut parsed = parse_rate_limit_line(
            &line,
            RateLimitLineContext {
                session_id: &session_id,
                account_id: account_id.as_deref(),
                source: Some(SourceSpan {
                    path: file_path.clone(),
                    line_number,
                }),
            },
            &mut parse_diagnostics,
        );
        diagnostics.merge_parse(&parse_diagnostics);

        for sample in parsed.drain(..) {
            if !sample_in_range(&sample, range) {
                diagnostics.out_of_range_samples += 1;
                continue;
            }
            if let Some(filter) = options.account_id.as_deref() {
                if sample.account_id.as_deref() != Some(filter) {
                    diagnostics.account_mismatches += 1;
                    continue;
                }
            }
            if let Some(filter) = options.plan_type.as_deref() {
                if sample.plan_type.as_deref() != Some(filter) {
                    diagnostics.plan_mismatches += 1;
                    continue;
                }
            }
            if let Some(filter) = options.window_minutes {
                if sample.window_minutes != filter {
                    diagnostics.window_mismatches += 1;
                    continue;
                }
            }
            if let Some(source) = sample.source.clone() {
                diagnostics.source_spans.push(source);
            }
            diagnostics.included_samples += 1;
            samples.push(sample);
        }
    }

    if file_has_rate_limits && !file_has_token_count {
        diagnostics.rate_limit_only_files += 1;
    }

    Ok(samples)
}

#[derive(Debug)]
struct MetadataEvent {
    event_type: String,
    session_id: Option<String>,
}

fn metadata_event(line: &str) -> Result<Option<MetadataEvent>, serde_json::Error> {
    let value = serde_json::from_str::<Value>(line)?;
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(event_type) = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(None);
    };
    if event_type != "session_meta" && event_type != "turn_context" {
        return Ok(None);
    }
    let session_id = object
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Ok(Some(MetadataEvent {
        event_type,
        session_id,
    }))
}

fn resolve_rate_limit_account_id(
    timestamp: Option<DateTime<Utc>>,
    history: Option<&UsageAccountHistory>,
) -> Option<String> {
    history.and_then(|history| timestamp.and_then(|timestamp| history.account_id_at(timestamp)))
}

fn sample_in_range(sample: &RateLimitSample, range: DateRange) -> bool {
    sample.timestamp >= range.start && sample.timestamp <= range.end
}

fn read_last_rate_limit_timestamp(path: &Path) -> Result<Option<DateTime<Utc>>, AppError> {
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

            if let Some(timestamp) = last_rate_limit_timestamp_in_lines(
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
        } else if let Some(timestamp) = last_rate_limit_timestamp_in_lines(&combined) {
            return Ok(Some(timestamp));
        }
    }

    Ok(None)
}

fn last_rate_limit_timestamp_in_lines(bytes: &[u8]) -> Option<DateTime<Utc>> {
    for line in bytes.split(|byte| *byte == b'\n').rev() {
        let line = trim_line_end_bytes(line);
        if line.is_empty() || !line_contains_bytes(line, b"\"rate_limits\"") {
            continue;
        };

        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        if let Some(timestamp) = timestamp_from_line(line) {
            return Some(timestamp);
        }
    }

    None
}

fn timestamp_from_line(line: &str) -> Option<DateTime<Utc>> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let object = value.as_object()?;
    object.get("timestamp").and_then(value_to_utc)
}

fn value_to_utc(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::String(value) => DateTime::parse_from_rfc3339(value.trim())
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc)),
        Value::Number(number) => {
            let millis = number
                .as_i64()
                .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))?;
            Utc.timestamp_millis_opt(millis).single()
        }
        _ => None,
    }
}

fn trim_line_end_bytes(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn line_contains_bytes(line: &[u8], needle: &[u8]) -> bool {
    line.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_history::format_account_history_iso;
    use crate::account_history::{
        AccountHistoryAccount, AccountHistoryStore, AccountHistorySwitchEvent,
        ACCOUNT_HISTORY_STORE_VERSION, AUTH_SELECT_SOURCE, DEFAULT_ACCOUNT_SOURCE,
    };
    use chrono::TimeZone;
    use serde_json::json;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn fixture_scan_returns_sorted_5h_and_7d_samples_with_diagnostics() {
        let fixture = fixture_codex_home();

        let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 10, 0, 0),
            end: utc_time(2026, 5, 12, 14, 0),
            sessions_dir: fixture.join("sessions"),
            scan_all_files: true,
            account_history_file: Some(fixture.join("codex-ops/auth-account-history.json")),
            account_id: None,
            plan_type: None,
            window_minutes: None,
        })
        .expect("read limit samples");

        assert_eq!(report.samples.len(), 11);
        assert!(report
            .samples
            .windows(2)
            .all(|pair| pair[0].timestamp <= pair[1].timestamp));
        assert_eq!(
            report
                .samples
                .iter()
                .filter(|sample| sample.window_minutes == 300)
                .count(),
            6
        );
        assert_eq!(
            report
                .samples
                .iter()
                .filter(|sample| sample.window_minutes == 10080)
                .count(),
            5
        );
        assert_eq!(report.diagnostics.null_rate_limits, 1);
        assert_eq!(report.diagnostics.rate_limit_only_files, 2);
        assert_eq!(report.diagnostics.rate_limit_events, 7);
        assert_eq!(report.diagnostics.included_samples, 11);
        assert_eq!(report.diagnostics.file_read_concurrency, 1);
        assert_eq!(report.diagnostics.source_spans.len(), 11);
        assert!(report
            .samples
            .iter()
            .any(|sample| sample.account_id.as_deref() == Some("account-other")));
    }

    #[test]
    fn account_plan_and_window_filters_are_applied_after_attribution() {
        let fixture = fixture_codex_home();
        let base = RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 10, 0, 0),
            end: utc_time(2026, 5, 12, 14, 0),
            sessions_dir: fixture.join("sessions"),
            scan_all_files: true,
            account_history_file: Some(fixture.join("codex-ops/auth-account-history.json")),
            account_id: None,
            plan_type: None,
            window_minutes: None,
        };

        let mut fixture_account = base.clone();
        fixture_account.account_id = Some("account-fixture".to_string());
        let report = read_rate_limit_samples_report(&fixture_account).expect("account filter");
        assert_eq!(report.samples.len(), 10);
        assert_eq!(report.diagnostics.account_mismatches, 1);

        let mut other_account = base.clone();
        other_account.account_id = Some("account-other".to_string());
        let report = read_rate_limit_samples_report(&other_account).expect("other account filter");
        assert_eq!(report.samples.len(), 1);
        assert_eq!(report.samples[0].plan_type.as_deref(), Some("plus"));
        assert_eq!(report.diagnostics.account_mismatches, 10);

        let mut plan = base.clone();
        plan.plan_type = Some("plus".to_string());
        let report = read_rate_limit_samples_report(&plan).expect("plan filter");
        assert_eq!(report.samples.len(), 1);
        assert_eq!(report.diagnostics.plan_mismatches, 10);

        let mut window = base;
        window.window_minutes = Some(10080);
        let report = read_rate_limit_samples_report(&window).expect("window filter");
        assert_eq!(report.samples.len(), 5);
        assert_eq!(report.diagnostics.window_mismatches, 6);
    }

    #[test]
    fn account_filter_without_history_treats_samples_as_unknown_mismatches() {
        let fixture = fixture_codex_home();

        let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 10, 0, 0),
            end: utc_time(2026, 5, 12, 14, 0),
            sessions_dir: fixture.join("sessions"),
            scan_all_files: true,
            account_history_file: None,
            account_id: Some("account-fixture".to_string()),
            plan_type: None,
            window_minutes: None,
        })
        .expect("read limit samples");

        assert!(report.samples.is_empty());
        assert_eq!(report.diagnostics.account_mismatches, 11);
    }

    #[test]
    fn tail_prefilter_reads_rate_limit_only_files_without_token_count() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        write_session_file(
            &sessions_dir,
            2026,
            5,
            10,
            "rollout-2026-05-10T00-00-00-rate-limit-only.jsonl",
            &[rate_limit_line(
                "2026-05-12T00:00:01.000Z",
                300,
                12.0,
                1778605200,
            )],
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            9,
            "rollout-2026-05-09T00-00-00-stale-rate-limit.jsonl",
            &[rate_limit_line(
                "2026-05-11T23:59:59.000Z",
                300,
                92.0,
                1778605200,
            )],
        );

        let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 12, 0, 0),
            end: utc_time(2026, 5, 12, 1, 0),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            account_id: None,
            plan_type: None,
            window_minutes: None,
        })
        .expect("read limit samples");

        assert_eq!(report.samples.len(), 1);
        assert_eq!(report.samples[0].used_percent, 12.0);
        assert_eq!(report.diagnostics.tail_read_files, 2);
        assert_eq!(report.diagnostics.tail_read_hits, 1);
        assert_eq!(report.diagnostics.prefiltered_files, 1);
        assert_eq!(report.diagnostics.rate_limit_only_files, 1);
    }

    #[test]
    fn fork_replayed_rate_limits_are_not_counted_for_child_sessions() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        let parent = "019e5110-1f92-7280-b02a-0876af32b81f";
        let child = "019e5111-2720-73d0-8519-4c80dffbe80e";
        let parent_lines = vec![
            session_meta_line("2026-05-21T00:00:00.000Z", parent),
            rate_limit_line("2026-05-21T00:00:01.000Z", 300, 10.0, 1779469200),
        ];
        let mut child_lines = vec![session_meta_line("2026-05-21T00:10:00.000Z", child)];
        child_lines.extend(retimestamp_lines(&parent_lines, "2026-05-21T00:10:00.001Z"));
        child_lines.push(rate_limit_line(
            "2026-05-21T00:10:01.000Z",
            300,
            11.0,
            1779469200,
        ));
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-00-00-{parent}.jsonl"),
            &parent_lines,
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-10-00-{child}.jsonl"),
            &child_lines,
        );

        let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 21, 0, 0),
            end: utc_time(2026, 5, 21, 1, 0),
            sessions_dir,
            scan_all_files: false,
            account_history_file: None,
            account_id: None,
            plan_type: None,
            window_minutes: None,
        })
        .expect("read limit samples");

        assert_eq!(report.samples.len(), 2);
        assert_eq!(
            report
                .samples
                .iter()
                .filter(|sample| sample.session_id == child)
                .count(),
            1
        );
        assert_eq!(report.diagnostics.rate_limit_events, 2);
        assert_eq!(report.diagnostics.fork_replay_lines_skipped, 2);
    }

    #[test]
    fn serialized_samples_report_hides_source_evidence_by_default() {
        let fixture = fixture_codex_home();
        let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 10, 0, 0),
            end: utc_time(2026, 5, 12, 14, 0),
            sessions_dir: fixture.join("sessions"),
            scan_all_files: true,
            account_history_file: Some(fixture.join("codex-ops/auth-account-history.json")),
            account_id: None,
            plan_type: None,
            window_minutes: None,
        })
        .expect("read limit samples");

        let value = serde_json::to_value(&report.samples).expect("sample json");
        assert_no_source_evidence(&value);
        let diagnostics = serde_json::to_value(&report.diagnostics).expect("diagnostics json");
        assert_no_source_evidence(&diagnostics);
    }

    #[test]
    fn scanner_accepts_synthetic_account_history_store_for_attribution() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        let history_file = temp.path().join("account-history.json");
        write_account_history(&history_file);
        write_session_file(
            &sessions_dir,
            2026,
            5,
            12,
            "rollout-2026-05-12T10-00-00-before-switch.jsonl",
            &[rate_limit_line(
                "2026-05-12T10:00:01.000Z",
                300,
                20.0,
                1778605200,
            )],
        );
        write_session_file(
            &sessions_dir,
            2026,
            5,
            12,
            "rollout-2026-05-12T13-00-00-after-switch.jsonl",
            &[rate_limit_line(
                "2026-05-12T13:00:01.000Z",
                300,
                22.0,
                1778605200,
            )],
        );

        let report = read_rate_limit_samples_report(&RateLimitSamplesReadOptions {
            start: utc_time(2026, 5, 12, 0, 0),
            end: utc_time(2026, 5, 12, 14, 0),
            sessions_dir,
            scan_all_files: false,
            account_history_file: Some(history_file),
            account_id: None,
            plan_type: None,
            window_minutes: None,
        })
        .expect("read limit samples");

        let accounts = report
            .samples
            .iter()
            .map(|sample| sample.account_id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            accounts,
            vec![Some("account-fixture"), Some("account-other")]
        );
    }

    fn fixture_codex_home() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/rust-run/codex-home")
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

    fn session_meta_line(timestamp: &str, session_id: &str) -> String {
        json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "model": "gpt-5.5",
                "cwd": "/workspace/limit-scan-test"
            }
        })
        .to_string()
    }

    fn rate_limit_line(
        timestamp: &str,
        window_minutes: i64,
        used_percent: f64,
        resets_at: i64,
    ) -> String {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "rate_limits": {
                    "primary": {
                        "window_minutes": window_minutes,
                        "used_percent": used_percent,
                        "resets_at": resets_at
                    },
                    "plan_type": "pro",
                    "limit_id": "fixture-limit"
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

    fn write_account_history(path: &Path) {
        let store = AccountHistoryStore {
            version: ACCOUNT_HISTORY_STORE_VERSION,
            default_account: Some(AccountHistoryAccount {
                account_id: "account-fixture".to_string(),
                observed_at: format_account_history_iso(utc_time(2026, 5, 12, 0, 0)),
                source: DEFAULT_ACCOUNT_SOURCE.to_string(),
                name: None,
                email: None,
                plan_type: Some("pro".to_string()),
            }),
            switches: vec![AccountHistorySwitchEvent {
                timestamp: format_account_history_iso(utc_time(2026, 5, 12, 12, 30)),
                from_account_id: "account-fixture".to_string(),
                to_account_id: "account-other".to_string(),
                source: AUTH_SELECT_SOURCE.to_string(),
            }],
        };
        let content = serde_json::to_string_pretty(&store).expect("history json");
        std::fs::write(path, format!("{content}\n")).expect("write history");
    }

    fn assert_no_source_evidence(value: &Value) {
        match value {
            Value::Object(object) => {
                for key in ["source", "sourcePath", "sourceLine", "line", "lineNumber"] {
                    assert!(object.get(key).is_none(), "unexpected source key {key}");
                }
                for value in object.values() {
                    assert_no_source_evidence(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    assert_no_source_evidence(value);
                }
            }
            _ => {}
        }
    }

    fn utc_time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid test time")
    }
}
