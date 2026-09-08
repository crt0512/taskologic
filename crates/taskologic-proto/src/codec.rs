//! One JSON document per line.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Longest line either side will accept. Descriptions are not novels.
pub const MAX_LINE_BYTES: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("malformed message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("message of {0} bytes exceeds the {MAX_LINE_BYTES} byte limit")]
    TooLong(usize),
}

/// Serialise with a trailing newline. serde_json escapes newlines inside
/// strings, so the output is always exactly one line.
pub fn encode<T: Serialize>(msg: &T) -> Result<String, CodecError> {
    let mut s = serde_json::to_string(msg)?;
    if s.len() > MAX_LINE_BYTES {
        return Err(CodecError::TooLong(s.len()));
    }
    s.push('\n');
    Ok(s)
}

pub fn decode<T: DeserializeOwned>(line: &str) -> Result<T, CodecError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(CodecError::TooLong(line.len()));
    }
    Ok(serde_json::from_str(line.trim_end_matches(['\r', '\n']))?)
}
