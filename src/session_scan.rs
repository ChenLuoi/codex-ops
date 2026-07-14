use crate::error::AppError;
use crate::time::{local_to_utc, local_to_utc_checked, DateRange};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::thread;

pub(crate) const DEFAULT_FILE_READ_CONCURRENCY: i64 = 8;
pub(crate) const SESSION_READ_BUFFER_SIZE: usize = 256 * 1024;

#[cfg(target_env = "musl")]
const DEFAULT_MAX_FILE_SCAN_THREADS: usize = 1;

#[cfg(not(target_env = "musl"))]
const DEFAULT_MAX_FILE_SCAN_THREADS: usize = 8;

const FILE_SCAN_WORKER_MIN_FILES: usize = 64;
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionScanOptions<'a> {
    pub(crate) sessions_dir: &'a Path,
    pub(crate) range: DateRange,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedSessionScan {
    pub(crate) files: Vec<PreparedSessionFile>,
    pub(crate) diagnostics: SessionScanDiagnostics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedSessionFile {
    pub(crate) path: PathBuf,
    pub(crate) current_session_id: Option<String>,
    pub(crate) replay_prefix_lines: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SessionScanDiagnostics {
    pub(crate) scanned_directories: i64,
    pub(crate) skipped_directories: i64,
    pub(crate) read_files: i64,
    pub(crate) skipped_files: i64,
    pub(crate) prefiltered_files: i64,
    pub(crate) tail_read_files: i64,
    pub(crate) tail_read_hits: i64,
    pub(crate) mtime_read_files: i64,
    pub(crate) mtime_tail_hits: i64,
    pub(crate) mtime_read_hits: i64,
    pub(crate) fork_files: i64,
    pub(crate) fork_parent_missing: i64,
    pub(crate) fork_replay_lines: i64,
}

#[derive(Default)]
struct JsonlFileListing {
    files: Vec<PathBuf>,
    tail_candidates: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
struct JsonlScanPolicy {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl JsonlScanPolicy {
    fn new(range: DateRange) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

enum JsonlFileAction {
    Read,
    TailPrefilter,
    Skip,
}

pub(crate) fn prepare_session_scan<F>(
    options: SessionScanOptions<'_>,
    mut read_tail_timestamp: F,
) -> Result<PreparedSessionScan, AppError>
where
    F: FnMut(&Path) -> Result<Option<DateTime<Utc>>, AppError>,
{
    let mut diagnostics = SessionScanDiagnostics::default();
    let listing = list_jsonl_files(
        options.sessions_dir,
        options.range,
        Some(Vec::new()),
        &mut diagnostics,
    )?;
    let archived_listing =
        list_archived_jsonl_files(options.sessions_dir, options.range, &mut diagnostics)?;
    let mut files = listing.files;
    files.extend(archived_listing.files);
    let mut tail_candidates = listing.tail_candidates;
    tail_candidates.extend(archived_listing.tail_candidates);
    let prefiltered_files = prefilter_files_by_last_event(
        &tail_candidates,
        options.range.start,
        &mut diagnostics,
        &mut read_tail_timestamp,
    )?;
    files.extend(prefiltered_files);
    files.sort();
    let files = prepare_session_files(files, options.sessions_dir, &mut diagnostics)?;
    diagnostics.read_files = files.len() as i64;

    Ok(PreparedSessionScan { files, diagnostics })
}

pub(crate) fn partition_items_for_workers<T: Clone>(
    items: &[T],
    worker_count: usize,
) -> Vec<Vec<T>> {
    if items.is_empty() {
        return Vec::new();
    }

    let partition_count = worker_count.max(1).min(items.len());
    let chunk_size = items.len().div_ceil(partition_count);
    items
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>()
}

pub(crate) fn resolve_session_file_scan_worker_count(file_count: usize) -> Result<usize, AppError> {
    if file_count <= 1 {
        return Ok(1);
    }

    if let Some(configured) = configured_file_scan_worker_count()? {
        return Ok(if configured == 0 {
            1
        } else {
            configured.min(file_count)
        });
    }

    if file_count < FILE_SCAN_WORKER_MIN_FILES {
        return Ok(1);
    }

    let available = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1);
    Ok(available
        .min(DEFAULT_MAX_FILE_SCAN_THREADS)
        .min(file_count)
        .max(1))
}

fn configured_file_scan_worker_count() -> Result<Option<usize>, AppError> {
    let Ok(raw) = env::var("CODEX_OPS_STAT_WORKERS") else {
        return Ok(None);
    };
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Ok(None);
    }

    trimmed.parse::<usize>().map(Some).map_err(|_| {
        AppError::new("Invalid CODEX_OPS_STAT_WORKERS. Expected a non-negative integer.")
    })
}

fn list_archived_jsonl_files(
    sessions_dir: &Path,
    range: DateRange,
    diagnostics: &mut SessionScanDiagnostics,
) -> Result<JsonlFileListing, AppError> {
    let Some(archived_dir) = archived_sessions_dir_for(sessions_dir) else {
        return Ok(JsonlFileListing::default());
    };
    if !archived_dir
        .try_exists()
        .map_err(|error| AppError::new(error.to_string()))?
    {
        return Ok(JsonlFileListing::default());
    }

    list_jsonl_files(&archived_dir, range, Some(Vec::new()), diagnostics)
}

fn archived_sessions_dir_for(sessions_dir: &Path) -> Option<PathBuf> {
    let parent = sessions_dir.parent()?;
    let archived_dir = parent.join("archived_sessions");
    if archived_dir == sessions_dir {
        None
    } else {
        Some(archived_dir)
    }
}

fn prepare_session_files(
    files: Vec<PathBuf>,
    sessions_dir: &Path,
    diagnostics: &mut SessionScanDiagnostics,
) -> Result<Vec<PreparedSessionFile>, AppError> {
    let mut metadata = Vec::with_capacity(files.len());
    let mut session_files = HashMap::new();

    for path in files {
        let lineage = read_leading_session_meta_ids(&path)?;
        let current_session_id = lineage.first().cloned();
        if let Some(session_id) = current_session_id.as_ref() {
            session_files
                .entry(session_id.clone())
                .or_insert_with(|| path.clone());
        }
        metadata.push(ForkFileMetadata {
            path,
            lineage,
            current_session_id,
            replay_prefix_lines: 0,
        });
    }

    let lookup_roots = file_lookup_roots(sessions_dir);
    let mut parent_lookup_cache: HashMap<String, Option<PathBuf>> = HashMap::new();

    for item in &mut metadata {
        let Some(parent_session_id) = item.lineage.get(1).cloned() else {
            continue;
        };

        diagnostics.fork_files += 1;
        let parent_path = match session_files.get(&parent_session_id).cloned() {
            Some(path) => Some(path),
            None => match parent_lookup_cache.get(&parent_session_id) {
                Some(path) => path.clone(),
                None => {
                    let path = find_rollout_file_by_session_id(&lookup_roots, &parent_session_id)?;
                    parent_lookup_cache.insert(parent_session_id.clone(), path.clone());
                    path
                }
            },
        };

        let Some(parent_path) = parent_path else {
            diagnostics.fork_parent_missing += 1;
            continue;
        };

        let child_path = item.path.clone();
        let replay_lines = count_fork_replay_lines_streaming(&child_path, &parent_path)?;

        if replay_lines > 0 {
            diagnostics.fork_replay_lines += replay_lines.saturating_sub(1) as i64;
            item.replay_prefix_lines = replay_lines;
        }
    }

    Ok(metadata
        .into_iter()
        .map(|metadata| PreparedSessionFile {
            path: metadata.path,
            current_session_id: if metadata.lineage.len() > 1 {
                metadata.current_session_id
            } else {
                None
            },
            replay_prefix_lines: metadata.replay_prefix_lines,
        })
        .collect())
}

struct ForkFileMetadata {
    path: PathBuf,
    lineage: Vec<String>,
    current_session_id: Option<String>,
    replay_prefix_lines: usize,
}

fn read_leading_session_meta_ids(path: &Path) -> Result<Vec<String>, AppError> {
    let file = File::open(path).map_err(|error| AppError::new(error.to_string()))?;
    let mut reader = BufReader::with_capacity(SESSION_READ_BUFFER_SIZE, file);
    let mut line = String::new();
    let mut lineage = Vec::new();

    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|error| AppError::new(error.to_string()))?;
        if bytes_read == 0 {
            break;
        }

        let Some(id) = leading_session_meta_id(&line) else {
            break;
        };
        lineage.push(id);
    }

    Ok(lineage)
}

fn leading_session_meta_id(line: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let fields = value.as_object()?;

    if fields.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }

    fields
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
}

fn file_lookup_roots(sessions_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![sessions_dir.to_path_buf()];
    if let Some(archived_dir) = archived_sessions_dir_for(sessions_dir) {
        roots.push(archived_dir);
    }
    roots
}

fn find_rollout_file_by_session_id(
    roots: &[PathBuf],
    session_id: &str,
) -> Result<Option<PathBuf>, AppError> {
    let mut matches = Vec::new();

    for root in roots {
        collect_rollout_files_by_session_id(root, session_id, &mut matches)?;
    }

    matches.sort();
    Ok(matches.into_iter().next())
}

fn collect_rollout_files_by_session_id(
    root: &Path,
    session_id: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<(), AppError> {
    let mut entries = match fs::read_dir(root) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::new(error.to_string()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::new(error.to_string())),
    };
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| AppError::new(error.to_string()))?;
        if file_type.is_dir() {
            collect_rollout_files_by_session_id(&path, session_id, matches)?;
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.contains(session_id))
        {
            matches.push(path);
        }
    }

    Ok(())
}

fn normalized_event_fingerprint_into(line: &str, buffer: &mut String) {
    buffer.clear();
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        buffer.push_str("invalid-json:");
        buffer.push_str(&line.len().to_string());
        return;
    };

    if let Value::Object(fields) = &mut value {
        fields.remove("timestamp");
    }

    canonical_json(&value, buffer);
}

fn canonical_json(value: &Value, buffer: &mut String) {
    match value {
        Value::Null => buffer.push_str("null"),
        Value::Bool(value) => {
            if *value {
                buffer.push_str("true");
            } else {
                buffer.push_str("false");
            }
        }
        Value::Number(value) => {
            buffer.push_str(&value.to_string());
        }
        Value::String(value) => {
            let serialized = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
            buffer.push_str(&serialized);
        }
        Value::Array(values) => {
            buffer.push('[');
            for (i, element) in values.iter().enumerate() {
                if i > 0 {
                    buffer.push(',');
                }
                canonical_json(element, buffer);
            }
            buffer.push(']');
        }
        Value::Object(fields) => {
            let mut entries: Vec<(&String, &Value)> = fields.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            buffer.push('{');
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    buffer.push(',');
                }
                let serialized_key =
                    serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                buffer.push_str(&serialized_key);
                buffer.push(':');
                canonical_json(value, buffer);
            }
            buffer.push('}');
        }
    }
}

fn count_fork_replay_lines_streaming(
    child_path: &Path,
    parent_path: &Path,
) -> Result<usize, AppError> {
    let child_file = File::open(child_path).map_err(|error| AppError::new(error.to_string()))?;
    let parent_file = File::open(parent_path).map_err(|error| AppError::new(error.to_string()))?;
    let mut child_reader = BufReader::with_capacity(SESSION_READ_BUFFER_SIZE, child_file);
    let mut parent_reader = BufReader::with_capacity(SESSION_READ_BUFFER_SIZE, parent_file);
    let mut child_line = String::new();
    let mut parent_line = String::new();
    let mut child_fingerprint = String::new();
    let mut parent_fingerprint = String::new();
    let mut matched = 0;

    if child_reader
        .read_line(&mut child_line)
        .map_err(|error| AppError::new(error.to_string()))?
        == 0
    {
        return Ok(0);
    }

    loop {
        child_line.clear();
        let child_bytes_read = child_reader
            .read_line(&mut child_line)
            .map_err(|error| AppError::new(error.to_string()))?;
        if child_bytes_read == 0 {
            break;
        }

        parent_line.clear();
        let parent_bytes_read = parent_reader
            .read_line(&mut parent_line)
            .map_err(|error| AppError::new(error.to_string()))?;
        if parent_bytes_read == 0 {
            break;
        }

        normalized_event_fingerprint_into(&child_line, &mut child_fingerprint);
        normalized_event_fingerprint_into(&parent_line, &mut parent_fingerprint);
        if child_fingerprint != parent_fingerprint {
            break;
        }
        matched += 1;
    }

    if matched == 0 {
        Ok(0)
    } else {
        Ok(matched + 1)
    }
}

fn list_jsonl_files(
    root: &Path,
    range: DateRange,
    date_parts: Option<Vec<String>>,
    diagnostics: &mut SessionScanDiagnostics,
) -> Result<JsonlFileListing, AppError> {
    diagnostics.scanned_directories += 1;

    let mut entries = match fs::read_dir(root) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::new(error.to_string()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(JsonlFileListing::default());
        }
        Err(error) => return Err(AppError::new(error.to_string())),
    };
    entries.sort_by_key(|entry| entry.file_name());

    let mut listing = JsonlFileListing::default();
    let policy = JsonlScanPolicy::new(range);

    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| AppError::new(error.to_string()))?;

        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            let next_date_parts = append_date_path_part(date_parts.as_ref(), &name);

            if let Some(parts) = next_date_parts.as_ref() {
                if should_skip_date_directory(parts, policy) {
                    diagnostics.skipped_directories += 1;
                    continue;
                }
            }

            let child_listing = list_jsonl_files(&path, range, next_date_parts, diagnostics)?;
            listing.files.extend(child_listing.files);
            listing
                .tail_candidates
                .extend(child_listing.tail_candidates);
        } else if file_type.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        {
            match classify_jsonl_file(&path, policy) {
                JsonlFileAction::Read => listing.files.push(path),
                JsonlFileAction::TailPrefilter => listing.tail_candidates.push(path),
                JsonlFileAction::Skip => diagnostics.skipped_files += 1,
            }
        }
    }

    listing.files.sort();
    listing.tail_candidates.sort();
    Ok(listing)
}

fn append_date_path_part(parts: Option<&Vec<String>>, name: &str) -> Option<Vec<String>> {
    let parts = parts?;

    if parts.len() >= 3 {
        return Some(parts.clone());
    }

    if parts.is_empty() && name.len() == 4 && name.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(vec![name.to_string()]);
    }

    if (parts.len() == 1 || parts.len() == 2)
        && name.len() == 2
        && name.chars().all(|ch| ch.is_ascii_digit())
    {
        let mut next = parts.clone();
        next.push(name.to_string());
        return Some(next);
    }

    None
}

fn should_skip_date_directory(parts: &[String], policy: JsonlScanPolicy) -> bool {
    let Some((start, _end)) = date_path_range(parts) else {
        return false;
    };

    start > policy.end
}

fn date_path_range(parts: &[String]) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let year = parts.first()?.parse::<i32>().ok()?;

    if parts.len() == 1 {
        return Some((
            local_to_utc(year, 1, 1, 0, 0, 0, 0),
            local_to_utc(year + 1, 1, 1, 0, 0, 0, 0) - Duration::milliseconds(1),
        ));
    }

    let month = parts.get(1)?.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }

    if parts.len() == 2 {
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        return Some((
            local_to_utc(year, month, 1, 0, 0, 0, 0),
            local_to_utc(next_year, next_month, 1, 0, 0, 0, 0) - Duration::milliseconds(1),
        ));
    }

    let day = parts.get(2)?.parse::<u32>().ok()?;
    let start = local_to_utc_checked(year, month, day, 0, 0, 0, 0)?;
    Some((start, start + Duration::days(1) - Duration::milliseconds(1)))
}

fn classify_jsonl_file(path: &Path, policy: JsonlScanPolicy) -> JsonlFileAction {
    let Some(timestamp) = rollout_timestamp_from_file_name(path) else {
        return JsonlFileAction::Read;
    };

    if timestamp > policy.end {
        return JsonlFileAction::Skip;
    }

    if timestamp >= policy.start {
        return JsonlFileAction::Read;
    }

    JsonlFileAction::TailPrefilter
}

fn prefilter_files_by_last_event<F>(
    files: &[PathBuf],
    start: DateTime<Utc>,
    diagnostics: &mut SessionScanDiagnostics,
    read_tail_timestamp: &mut F,
) -> Result<Vec<PathBuf>, AppError>
where
    F: FnMut(&Path) -> Result<Option<DateTime<Utc>>, AppError>,
{
    let mut kept = Vec::new();

    for candidate in files {
        diagnostics.tail_read_files += 1;
        let last_event_at = read_tail_timestamp(candidate)?;

        if last_event_at.is_some_and(|timestamp| timestamp < start) {
            diagnostics.prefiltered_files += 1;
        } else {
            diagnostics.tail_read_hits += 1;
            kept.push(candidate.clone());
        }
    }

    Ok(kept)
}

pub(crate) fn session_id_from_path(path: &Path) -> String {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return path.to_string_lossy().to_string();
    };

    if let Some(rest) = name.strip_prefix("rollout-") {
        if let Some(id) = rest.strip_suffix(".jsonl").and_then(|rest| rest.get(20..)) {
            return id.to_string();
        }
    }

    path.to_string_lossy().to_string()
}

fn rollout_timestamp_from_file_name(path: &Path) -> Option<DateTime<Utc>> {
    let name = path.file_name()?.to_str()?;

    if !name.starts_with("rollout-") || !name.ends_with(".jsonl") || name.len() < 28 {
        return None;
    }

    let year = name.get(8..12)?.parse::<i32>().ok()?;
    let month = name.get(13..15)?.parse::<u32>().ok()?;
    let day = name.get(16..18)?.parse::<u32>().ok()?;
    let hour = name.get(19..21)?.parse::<u32>().ok()?;
    let minute = name.get(22..24)?.parse::<u32>().ok()?;
    let second = name.get(25..27)?.parse::<u32>().ok()?;

    local_to_utc_checked(year, month, day, hour, minute, second, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::Write;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn scan_policy_keeps_the_requested_range() {
        let start = utc_time(2026, 5, 21, 0);
        let end = start + Duration::days(120);
        let policy = JsonlScanPolicy::new(DateRange { start, end });

        assert_eq!(policy.start, start);
        assert_eq!(policy.end, end);
    }

    #[test]
    fn date_directory_pruning_skips_future_days() {
        let temp = TempDir::new().expect("tempdir");
        write_session_file(
            temp.path(),
            2026,
            5,
            23,
            "rollout-2026-05-23T00-00-00-future.jsonl",
            &["{}".to_string()],
        );
        let mut diagnostics = SessionScanDiagnostics::default();

        let listing = list_jsonl_files(
            temp.path(),
            DateRange {
                start: utc_time(2026, 5, 21, 0),
                end: utc_time(2026, 5, 21, 23),
            },
            Some(Vec::new()),
            &mut diagnostics,
        )
        .expect("list files");

        assert!(listing.files.is_empty());
        assert!(listing.tail_candidates.is_empty());
        assert_eq!(diagnostics.skipped_directories, 1);
    }

    #[test]
    fn old_files_before_range_are_tail_prefiltered_without_mtime() {
        let temp = TempDir::new().expect("tempdir");
        let start = utc_time(2000, 1, 1, 0);
        let end = utc_time(2000, 1, 2, 0);
        let file = write_session_file(
            temp.path(),
            1999,
            1,
            1,
            "rollout-1999-01-01T00-00-00-active.jsonl",
            &["{}".to_string()],
        );
        let mut diagnostics = SessionScanDiagnostics::default();

        let listing = list_jsonl_files(
            temp.path(),
            DateRange { start, end },
            Some(Vec::new()),
            &mut diagnostics,
        )
        .expect("list files");

        assert!(listing.files.is_empty());
        assert_eq!(listing.tail_candidates.len(), 1);
        assert_eq!(listing.tail_candidates[0], file);
        assert_eq!(diagnostics.skipped_directories, 0);
    }

    #[test]
    fn old_files_with_old_mtime_are_still_tail_prefiltered() {
        let temp = TempDir::new().expect("tempdir");
        let start = utc_time(2999, 1, 1, 0);
        let end = utc_time(2999, 1, 2, 0);
        let file = write_session_file(
            temp.path(),
            2020,
            1,
            1,
            "rollout-2020-01-01T00-00-00-inactive.jsonl",
            &["{}".to_string()],
        );
        let mut diagnostics = SessionScanDiagnostics::default();

        let listing = list_jsonl_files(
            temp.path(),
            DateRange { start, end },
            Some(Vec::new()),
            &mut diagnostics,
        )
        .expect("list files");

        assert!(listing.files.is_empty());
        assert_eq!(listing.tail_candidates, vec![file]);
        assert_eq!(diagnostics.skipped_files, 0);
        assert_eq!(diagnostics.skipped_directories, 0);
    }

    #[test]
    fn tail_prefilter_tracks_hits() {
        let temp = TempDir::new().expect("tempdir");
        let start = utc_time(2026, 5, 21, 0);
        let stale = temp.path().join("stale.jsonl");
        let active = temp.path().join("active.jsonl");
        let unknown = temp.path().join("unknown.jsonl");
        let candidates = vec![stale.clone(), active.clone(), unknown.clone()];
        let mut diagnostics = SessionScanDiagnostics::default();

        let kept =
            prefilter_files_by_last_event(&candidates, start, &mut diagnostics, &mut |path| {
                if path == stale {
                    Ok(Some(start - Duration::seconds(1)))
                } else if path == active {
                    Ok(Some(start + Duration::seconds(1)))
                } else {
                    Ok(None)
                }
            })
            .expect("prefilter files");

        assert_eq!(kept, vec![active, unknown]);
        assert_eq!(diagnostics.tail_read_files, 3);
        assert_eq!(diagnostics.tail_read_hits, 2);
        assert_eq!(diagnostics.prefiltered_files, 1);
    }

    #[test]
    fn prepare_session_scan_includes_archived_sessions_sibling() {
        let temp = TempDir::new().expect("tempdir");
        let codex_home = temp.path().join("codex-home");
        let sessions_dir = codex_home.join("sessions");
        let archived_dir = codex_home.join("archived_sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let archived_file = write_flat_session_file(
            &archived_dir,
            "rollout-2026-05-21T01-00-00-archived-session.jsonl",
            &["{}".to_string()],
        );

        let prepared = prepare_session_scan(
            SessionScanOptions {
                sessions_dir: &sessions_dir,
                range: DateRange {
                    start: utc_time(2026, 5, 21, 0),
                    end: utc_time(2026, 5, 21, 2),
                },
            },
            |_| Ok(None),
        )
        .expect("prepare scan");

        assert_eq!(prepared.files.len(), 1);
        assert_eq!(prepared.files[0].path, archived_file);
        assert_eq!(prepared.diagnostics.read_files, 1);
    }

    #[test]
    fn prepare_session_scan_marks_fork_replay_prefix_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let sessions_dir = temp.path().join("sessions");
        let parent_session = "019e5110-1f92-7280-b02a-0876af32b81f";
        let child_session = "019e5111-2720-73d0-8519-4c80dffbe80e";
        let parent_lines = vec![
            session_meta_line("2026-05-21T00:00:00.000Z", parent_session),
            event_line("2026-05-21T00:00:01.000Z", "parent-event"),
        ];
        let mut child_lines = vec![session_meta_line("2026-05-21T00:10:00.000Z", child_session)];
        child_lines.extend(retimestamp_lines(&parent_lines, "2026-05-21T00:10:00.001Z"));
        child_lines.push(event_line("2026-05-21T00:10:01.000Z", "child-event"));
        let parent_path = write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-00-00-{parent_session}.jsonl"),
            &parent_lines,
        );
        let child_path = write_session_file(
            &sessions_dir,
            2026,
            5,
            21,
            &format!("rollout-2026-05-21T00-10-00-{child_session}.jsonl"),
            &child_lines,
        );

        let prepared = prepare_session_scan(
            SessionScanOptions {
                sessions_dir: &sessions_dir,
                range: DateRange {
                    start: utc_time(2026, 5, 21, 0),
                    end: utc_time(2026, 5, 21, 1),
                },
            },
            |_| Ok(None),
        )
        .expect("prepare scan");

        let parent = prepared
            .files
            .iter()
            .find(|file| file.path == parent_path)
            .expect("parent file");
        let child = prepared
            .files
            .iter()
            .find(|file| file.path == child_path)
            .expect("child file");

        assert_eq!(parent.current_session_id, None);
        assert_eq!(parent.replay_prefix_lines, 0);
        assert_eq!(child.current_session_id.as_deref(), Some(child_session));
        assert_eq!(child.replay_prefix_lines, 3);
        assert_eq!(prepared.diagnostics.fork_files, 1);
        assert_eq!(prepared.diagnostics.fork_parent_missing, 0);
        assert_eq!(prepared.diagnostics.fork_replay_lines, 2);
    }

    #[test]
    fn partitions_items_for_workers_in_stable_order() {
        let files = (0..10)
            .map(|index| PathBuf::from(format!("file-{index}.jsonl")))
            .collect::<Vec<_>>();
        let partitions = partition_items_for_workers(&files, 3);

        assert_eq!(
            partitions.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![4, 4, 2]
        );
        assert_eq!(partitions.into_iter().flatten().collect::<Vec<_>>(), files);
        assert!(partition_items_for_workers::<PathBuf>(&[], 8).is_empty());
        assert_eq!(partition_items_for_workers(&files[..2], 8).len(), 2);
    }

    #[test]
    fn default_file_scan_worker_limit_matches_target_env() {
        if cfg!(target_env = "musl") {
            assert_eq!(DEFAULT_MAX_FILE_SCAN_THREADS, 1);
        } else {
            assert_eq!(DEFAULT_MAX_FILE_SCAN_THREADS, 8);
        }
    }

    #[test]
    fn file_scan_worker_count_respects_small_file_sets() {
        let _guard = env_lock();
        env::remove_var("CODEX_OPS_STAT_WORKERS");

        assert_eq!(resolve_session_file_scan_worker_count(0).unwrap(), 1);
        assert_eq!(resolve_session_file_scan_worker_count(1).unwrap(), 1);
        assert_eq!(
            resolve_session_file_scan_worker_count(FILE_SCAN_WORKER_MIN_FILES - 1).unwrap(),
            1
        );
    }

    #[test]
    fn file_scan_worker_count_respects_env_override() {
        let _guard = env_lock();
        env::set_var("CODEX_OPS_STAT_WORKERS", "4");

        assert_eq!(resolve_session_file_scan_worker_count(100).unwrap(), 4);
        assert_eq!(resolve_session_file_scan_worker_count(2).unwrap(), 2);

        env::set_var("CODEX_OPS_STAT_WORKERS", "0");
        assert_eq!(resolve_session_file_scan_worker_count(100).unwrap(), 1);

        env::remove_var("CODEX_OPS_STAT_WORKERS");
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

    fn session_meta_line(timestamp: &str, session_id: &str) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "model": "gpt-5.5",
                "cwd": "/workspace/fork-test"
            }
        })
        .to_string()
    }

    fn event_line(timestamp: &str, label: &str) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": label
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
            .expect("valid test time")
    }
}
