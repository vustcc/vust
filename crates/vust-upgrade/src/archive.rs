//! 升级组件归档处理。

use flate2::read::GzDecoder;
use std::io::Read;
use thiserror::Error;

/// 归档解析错误。
#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("failed to read archive: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive does not contain {0}")]
    Missing(String),
}

/// 从 tar.gz 中提取指定文件名的内容。
pub fn extract_named_file_from_tar_gz(
    archive_bytes: &[u8],
    expected_file_name: &str,
) -> Result<Vec<u8>, ArchiveError> {
    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?;
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name == expected_file_name {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    Err(ArchiveError::Missing(expected_file_name.to_string()))
}
