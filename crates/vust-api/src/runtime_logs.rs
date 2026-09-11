// Master 运行日志服务：受控文件枚举、轮转安全游标与有界反向扫描。

use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use vust_contracts::{
    api::ErrorCode,
    runtime_logs::{
        RuntimeLogAvailability, RuntimeLogFile, RuntimeLogFileList, RuntimeLogLine,
        RuntimeLogQuery, RuntimeLogQueryResult,
    },
};

use crate::error::ApiError;

type ApiResult<T> = Result<T, ApiError>;

const MAX_LIMIT: u32 = 300;
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;
const BUFFER_SIZE: usize = 64 * 1024;
const DEFAULT_PRODUCTION_HOME: &str = "/opt/vust";

pub fn runtime_log_root() -> PathBuf {
    std::env::var_os("VUST_LOG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("VUST_DATA_DIR").map(|value| PathBuf::from(value).join("logs"))
        })
        .unwrap_or_else(default_log_root)
}

pub fn service_log_dir(service: &str) -> ApiResult<PathBuf> {
    match service {
        "master" => Ok(runtime_log_root().join("vust")),
        "agent" => Ok(runtime_log_root().join("agent")),
        _ => Err(ApiError::BadRequest(
            "unsupported runtime log service".to_string(),
        )),
    }
}

pub async fn list_runtime_log_files(service_filter: Option<&str>) -> ApiResult<RuntimeLogFileList> {
    let service = service_filter.unwrap_or("master");
    let dir = service_log_dir(service)?;
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeLogFileList {
                availability: RuntimeLogAvailability::ServiceStopped,
                reason_code: Some("RUNTIME_LOG_DIRECTORY_MISSING".to_string()),
                files: vec![],
            });
        }
        Err(err) => return Err(ApiError::Io(err)),
    };
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(ApiError::Io)? {
        let metadata = entry.metadata().await.map_err(ApiError::Io)?;
        if !metadata.is_file() {
            continue;
        }
        let Some(file_name) = entry.file_name().to_str().map(ToString::to_string) else {
            continue;
        };
        let version = file_identity(service, &file_name, &metadata);
        files.push(RuntimeLogFile {
            service: service.to_string(),
            node_id: local_node_id(service),
            node_name: local_node_name(service),
            file_id: version.clone(),
            file_version: version,
            file_name,
            size_bytes: metadata.len(),
            modified_at: metadata.modified().ok().and_then(format_system_time),
        });
    }
    files.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.file_name.cmp(&b.file_name))
    });
    Ok(RuntimeLogFileList {
        availability: if files.is_empty() {
            RuntimeLogAvailability::NoLogFiles
        } else {
            RuntimeLogAvailability::Available
        },
        reason_code: files
            .is_empty()
            .then(|| "RUNTIME_LOG_FILES_EMPTY".to_string()),
        files,
    })
}

pub async fn query_runtime_logs(query: RuntimeLogQuery) -> ApiResult<RuntimeLogQueryResult> {
    if query.limit == 0 || query.limit > MAX_LIMIT {
        return Err(ApiError::BadRequest(
            "runtime log limit must be between 1 and 300".to_string(),
        ));
    }
    if query
        .keyword
        .as_ref()
        .is_some_and(|v| v.chars().count() > 100)
        || query
            .target
            .as_ref()
            .is_some_and(|v| v.chars().count() > 100)
    {
        return Err(ApiError::BadRequest(
            "runtime log filter is too long".to_string(),
        ));
    }
    let path = resolve_log_file(&query.service, &query.file_name)?;
    let mut file = tokio::fs::File::open(&path).await.map_err(ApiError::Io)?;
    let metadata = file.metadata().await.map_err(ApiError::Io)?;
    let version = file_identity(&query.service, &query.file_name, &metadata);
    if query.file_id != version {
        return Err(stale_cursor());
    }
    let file_len = metadata.len();
    let mut cursor = match query.cursor.as_deref() {
        Some(value) => decode_cursor(value, &version)?,
        None => file_len,
    };
    if cursor > file_len {
        return Err(stale_cursor());
    }

    let mut lines = Vec::new();
    let mut scanned = 0u64;
    let mut leftover = Vec::new();
    while cursor > 0 && lines.len() < query.limit as usize && scanned < MAX_SCAN_BYTES {
        let read_len = cursor.min(BUFFER_SIZE as u64).min(MAX_SCAN_BYTES - scanned) as usize;
        let start = cursor - read_len as u64;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(ApiError::Io)?;
        let mut buffer = vec![0; read_len];
        file.read_exact(&mut buffer).await.map_err(ApiError::Io)?;
        scanned += read_len as u64;
        cursor = start;
        buffer.extend_from_slice(&leftover);
        leftover.clear();
        let mut offset = start;
        let mut parts: Vec<(u64, &[u8])> = buffer
            .split(|byte| *byte == b'\n')
            .map(|part| {
                let current = offset;
                offset += part.len() as u64 + 1;
                (current, part)
            })
            .collect();
        if cursor > 0 && !parts.is_empty() {
            leftover = parts.remove(0).1.to_vec();
        }
        for (line_offset, raw) in parts.into_iter().rev() {
            let raw = String::from_utf8_lossy(raw)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            if raw.is_empty() {
                continue;
            }
            let line = parse_runtime_log_line(line_offset, &raw);
            if matches_query(&line, &raw, &query) {
                lines.push(line);
                if lines.len() == query.limit as usize {
                    cursor = line_offset;
                    break;
                }
            }
        }
    }
    if cursor == 0 && !leftover.is_empty() && lines.len() < query.limit as usize {
        let raw = String::from_utf8_lossy(&leftover).trim().to_string();
        if !raw.is_empty() {
            let line = parse_runtime_log_line(0, &raw);
            if matches_query(&line, &raw, &query) {
                lines.push(line);
            }
        }
    }
    let scan_truncated =
        scanned >= MAX_SCAN_BYTES && cursor > 0 && lines.len() < query.limit as usize;
    Ok(RuntimeLogQueryResult {
        lines,
        next_cursor: (cursor > 0).then(|| encode_cursor(&version, cursor)),
        has_more: cursor > 0,
        scan_truncated,
        file_version: version,
    })
}

fn resolve_log_file(service: &str, file_name: &str) -> ApiResult<PathBuf> {
    let name = Path::new(file_name);
    if name.components().count() != 1 || file_name.starts_with('.') {
        return Err(ApiError::BadRequest(
            "invalid runtime log file name".to_string(),
        ));
    }
    Ok(service_log_dir(service)?.join(name))
}
fn file_identity(service: &str, file_name: &str, metadata: &std::fs::Metadata) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    service.hash(&mut hasher);
    file_name.hash(&mut hasher);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.dev().hash(&mut hasher);
        metadata.ino().hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}
fn encode_cursor(version: &str, offset: u64) -> String {
    format!("{version}:{offset}")
}
fn decode_cursor(cursor: &str, version: &str) -> ApiResult<u64> {
    let Some((actual, offset)) = cursor.split_once(':') else {
        return Err(stale_cursor());
    };
    if actual != version {
        return Err(stale_cursor());
    }
    offset.parse().map_err(|_| stale_cursor())
}
fn stale_cursor() -> ApiError {
    ApiError::conflict(
        ErrorCode::RuntimeLogCursorStale,
        "runtime log file was rotated or truncated",
    )
}

fn parse_runtime_log_line(offset: u64, raw: &str) -> RuntimeLogLine {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return RuntimeLogLine {
            offset,
            timestamp: None,
            level: None,
            target: None,
            source: None,
            message: redact(raw),
            parse_error: true,
        };
    };
    RuntimeLogLine {
        offset,
        timestamp: json_string(&value, "timestamp"),
        level: json_string(&value, "level"),
        target: json_string(&value, "target"),
        source: nested_string(&value, "client_ip").and_then(|v| normalize_client_ip(&v)),
        message: redact(&nested_string(&value, "message").unwrap_or_default()),
        parse_error: false,
    }
}
fn matches_query(line: &RuntimeLogLine, raw: &str, query: &RuntimeLogQuery) -> bool {
    if let Some(level) = query
        .level
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        && !line
            .level
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case(level))
    {
        return false;
    }
    if let Some(target) = query
        .target
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        && !line.target.as_deref().unwrap_or_default().contains(target)
    {
        return false;
    }
    if let Some(keyword) = query
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        && !raw.contains(keyword)
        && !line.message.contains(keyword)
    {
        return false;
    }
    true
}
fn redact(value: &str) -> String {
    if ["authorization", "password", "token", "secret", "cookie"]
        .iter()
        .any(|key| value.to_ascii_lowercase().contains(key))
    {
        "[sensitive content redacted]".to_string()
    } else {
        value.chars().take(4096).collect()
    }
}
fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn nested_string(value: &Value, key: &str) -> Option<String> {
    json_string(value, key)
        .or_else(|| value.get("fields").and_then(|v| json_string(v, key)))
        .or_else(|| value.get("span").and_then(|v| json_string(v, key)))
        .or_else(|| {
            value
                .get("spans")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().rev().find_map(|v| json_string(v, key)))
        })
}
fn normalize_client_ip(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(
            value
                .split(',')
                .next()
                .unwrap_or(value)
                .chars()
                .take(64)
                .collect(),
        )
    }
}
fn format_system_time(value: SystemTime) -> Option<String> {
    let value: chrono::DateTime<chrono::Utc> = value.into();
    Some(value.to_rfc3339())
}
fn local_node_id(service: &str) -> Option<String> {
    (service == "agent").then(|| "local".to_string())
}
fn local_node_name(service: &str) -> Option<String> {
    (service == "agent").then(|| "Local Node".to_string())
}
fn default_log_root() -> PathBuf {
    PathBuf::from(DEFAULT_PRODUCTION_HOME).join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal() {
        assert!(resolve_log_file("master", "../secret").is_err());
    }
    #[test]
    fn cursor_checks_file_version() {
        assert!(decode_cursor("old:10", "new").is_err());
        assert_eq!(decode_cursor("new:10", "new").unwrap(), 10);
    }
    #[test]
    fn redacts_sensitive_lines() {
        assert_eq!(
            redact("Authorization: bearer x"),
            "[sensitive content redacted]"
        );
    }
}
