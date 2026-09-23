//! Reverse JSONL paging with fixed I/O, parse and response budgets, including oversized records.
use super::{
    catalog::{decode, encode},
    pi_files::{io_status, timestamp},
};
use aionui_api_types::{NativeSessionBackend, NativeSessionMessage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::File,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(super) const READ_BYTES: usize = 512 * 1024;
const RECORD_BYTES: usize = 128 * 1024;
pub(super) const TEXT_CHARS: usize = 8000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    backend: NativeSessionBackend,
    id: String,
    length: u64,
    stamp: u64,
    end: u64,
    discard: bool,
}

pub(super) fn page(
    root: &Path,
    path: &Path,
    backend: NativeSessionBackend,
    id: &str,
    raw: Option<&str>,
    limit: usize,
) -> Result<(Vec<NativeSessionMessage>, Option<String>, bool), String> {
    let mut file = open_file(root, path)?;
    let metadata = file.metadata().map_err(io_status)?;
    let mut w: Window = if let Some(raw) = raw {
        decode(raw)?
    } else {
        let length = metadata.len();
        Window {
            backend,
            id: id.into(),
            length,
            stamp: fingerprint(&mut file, length)?,
            end: length,
            discard: false,
        }
    };
    if w.backend != backend || w.id != id || w.end > w.length {
        return Err("bad_request".into());
    }
    if metadata.len() < w.length || fingerprint(&mut file, w.length)? != w.stamp {
        return Err("stale_cursor".into());
    }
    let start = w.end.saturating_sub(READ_BYTES as u64);
    file.seek(SeekFrom::Start(start)).map_err(io_status)?;
    let mut bytes = Vec::with_capacity((w.end - start) as usize);
    file.take(w.end - start).read_to_end(&mut bytes).map_err(io_status)?;
    let mut end = bytes.len();
    let mut partial = false;
    let mut items = Vec::new();
    let unfinished_tail = bytes.last().is_some_and(|b| *b != b'\n') && {
        let line_start = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
        (line_start == 0 && start > 0)
            || bytes.len() - line_start > RECORD_BYTES
            || serde_json::from_slice::<Value>(&bytes[line_start..]).is_err()
    };
    if w.discard || unfinished_tail {
        // Valid JSON at EOF needs no newline; unfinished appends and oversized fragments do.
        partial = true;
        match bytes.iter().rposition(|b| *b == b'\n') {
            Some(pos) => {
                end = pos + 1;
                w.discard = false;
            }
            None => {
                w.end = start;
                w.discard = start > 0;
                return Ok((items, (start > 0).then(|| encode(&w)), true));
            }
        }
    }
    let mut scanned = 0;
    while end > 0 && items.len() < limit && scanned < 2048 {
        scanned += 1;
        let content_end = if bytes[end - 1] == b'\n' { end - 1 } else { end };
        let line_start = bytes[..content_end]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        if line_start == 0 && start > 0 {
            if end == bytes.len() {
                // This record alone exceeds the entire window. Mark once, then skip its fragments on following pages.
                items.push(marker(id, start + end as u64, "oversized"));
                partial = true;
                w.discard = true;
                end = 0;
            }
            break;
        }
        let raw_line = &bytes[line_start..content_end];
        if raw_line.len() > RECORD_BYTES {
            items.push(marker(id, start + line_start as u64, "oversized"));
            partial = true;
        } else if !raw_line.is_empty() {
            match serde_json::from_slice::<Value>(raw_line) {
                Ok(value) => {
                    if let Some(mut item) = parse(&value, backend) {
                        item.id = format!("{id}:{}", start + line_start as u64);
                        items.push(item);
                    }
                }
                Err(_) => {
                    items.push(marker(id, start + line_start as u64, "malformed"));
                    partial = true;
                }
            }
        }
        end = line_start;
    }
    w.end = start + end as u64;
    items.reverse();
    Ok((items, (w.end > 0).then(|| encode(&w)), partial))
}

/// Reject non-regular paths before opening them (notably FIFOs), including cached Pi paths.
pub(super) fn open_file(root: &Path, path: &Path) -> Result<File, String> {
    let canonical_root = root.canonicalize().map_err(io_status)?;
    let canonical = path.canonicalize().map_err(io_status)?;
    if !canonical.starts_with(&canonical_root)
        || canonical.extension().is_none_or(|e| e != "jsonl")
        || !std::fs::symlink_metadata(path)
            .map_err(io_status)?
            .file_type()
            .is_file()
    {
        return Err("read_error".into());
    }
    let file = File::open(canonical).map_err(io_status)?;
    if !file.metadata().map_err(io_status)?.is_file() {
        return Err("read_error".into());
    }
    Ok(file)
}

fn fingerprint(file: &mut File, length: u64) -> Result<u64, String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    length.hash(&mut hash);
    for offset in [0, length.saturating_sub(256)] {
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        let mut bytes = [0u8; 256];
        let len = ((length - offset).min(256)) as usize;
        file.read_exact(&mut bytes[..len]).map_err(io_status)?;
        bytes[..len].hash(&mut hash);
    }
    Ok(hash.finish())
}
fn marker(id: &str, position: u64, kind: &str) -> NativeSessionMessage {
    NativeSessionMessage {
        id: format!("{id}:{position}"),
        role: "system".into(),
        kind: kind.into(),
        text: String::new(),
        timestamp: None,
        truncated: true,
    }
}

pub(super) fn parse(v: &Value, backend: NativeSessionBackend) -> Option<NativeSessionMessage> {
    let (role, kind, body, time) = if backend == NativeSessionBackend::Pi {
        if v["type"] != "message" {
            return None;
        }
        let m = &v["message"];
        let role = m["role"].as_str().unwrap_or("unknown");
        (
            role,
            if role == "toolResult" { "tool_result" } else { "message" },
            m["content"].clone(),
            m["timestamp"]
                .as_i64()
                .or_else(|| v["timestamp"].as_str().and_then(timestamp)),
        )
    } else {
        if v["type"] != "response_item" {
            return None;
        }
        let p = &v["payload"];
        let time = v["timestamp"].as_str().and_then(timestamp);
        match p["type"].as_str()? {
            "message" => (
                p["role"].as_str().unwrap_or("unknown"),
                "message",
                p["content"].clone(),
                time,
            ),
            "function_call" | "custom_tool_call" => (
                "assistant",
                "tool_call",
                Value::String(format!(
                    "{}\n{}",
                    p["name"].as_str().unwrap_or(""),
                    p["arguments"].as_str().or_else(|| p["input"].as_str()).unwrap_or("")
                )),
                time,
            ),
            "function_call_output" | "custom_tool_call_output" => ("tool", "tool_result", p["output"].clone(), time),
            "reasoning" => ("assistant", "thinking", p["summary"].clone(), time),
            _ => return None,
        }
    };
    let (text, truncated) = text(&body, TEXT_CHARS);
    if text.is_empty() {
        return None;
    }
    Some(NativeSessionMessage {
        id: String::new(),
        role: role.into(),
        kind: kind.into(),
        text,
        timestamp: time,
        truncated,
    })
}

/// Extract display text, never signatures/base64/images. Bound allocations before concatenation.
pub(super) fn text(v: &Value, max: usize) -> (String, bool) {
    fn append(v: &Value, out: &mut String, remaining: &mut usize, truncated: &mut bool) {
        match v {
            Value::String(s) => {
                let mut chars = s.chars();
                let piece: String = chars.by_ref().take(*remaining).collect();
                *remaining -= piece.chars().count();
                out.push_str(&piece);
                *truncated |= chars.next().is_some();
            }
            Value::Array(values) => {
                for v in values {
                    if *remaining == 0 {
                        *truncated = true;
                        break;
                    }
                    if !out.is_empty() {
                        out.push('\n');
                        *remaining = remaining.saturating_sub(1);
                    }
                    append(v, out, remaining, truncated);
                }
            }
            Value::Object(o) => {
                if let Some(value) = ["text", "thinking", "output", "message"]
                    .iter()
                    .find_map(|key| o.get(*key))
                {
                    append(value, out, remaining, truncated);
                } else if o.get("type").and_then(Value::as_str) == Some("toolCall") {
                    if let Some(name) = o.get("name") {
                        append(name, out, remaining, truncated);
                    }
                    if let Some(args) = o.get("arguments") {
                        append(&Value::String(args.to_string()), out, remaining, truncated);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    let mut remaining = max;
    let mut truncated = false;
    append(v, &mut out, &mut remaining, &mut truncated);
    (out, truncated)
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
