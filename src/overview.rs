use std::fs::{self, File};
use std::io::{Read as _, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::app::{first_line, now_ms, truncate_chars};
use crate::discover;

#[derive(Clone, Copy, PartialEq)]
pub enum SessState {
    Working,
    Waiting,
    Stalled,
    Idle,
}

/// One machine-wide session card for the `0` view.
pub struct OverviewSession {
    pub state: SessState,
    pub id: String,
    pub cwd: String,
    pub branch: String,
    pub title: String,
    pub model: String,
    pub mtime_ms: i64,
    pub actions: Vec<(i64, String)>,
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn parse_ts(v: &Value) -> Option<i64> {
    v.get("timestamp")?
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.timestamp_millis())
}

/// Feed-style one-liner for a transcript entry, or None if it isn't an action.
fn summarize(v: &Value) -> Option<String> {
    match s(v, "type").unwrap_or("") {
        "user" => {
            if v.get("isMeta").and_then(|x| x.as_bool()) == Some(true)
                || v.get("isCompactSummary").and_then(|x| x.as_bool()) == Some(true)
            {
                return None;
            }
            let c = v.get("message")?.get("content")?;
            let text = match c {
                Value::String(t) => t.clone(),
                Value::Array(a) => a
                    .iter()
                    .filter(|b| s(b, "type") == Some("text"))
                    .filter_map(|b| s(b, "text"))
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => return None,
            };
            let t = text.trim();
            if t.is_empty() || t.starts_with('<') {
                return None;
            }
            Some(format!("❯ {}", first_line(t, 90)))
        }
        "assistant" => {
            let blocks = v.get("message")?.get("content")?.as_array()?;
            for b in blocks {
                match s(b, "type").unwrap_or("") {
                    "text" => {
                        if let Some(t) = s(b, "text") {
                            if !t.trim().is_empty() {
                                return Some(format!("▷ {}", first_line(t, 90)));
                            }
                        }
                    }
                    "tool_use" => {
                        let name = s(b, "name").unwrap_or("?");
                        let input = b.get("input");
                        let inp = |k: &str| {
                            input.and_then(|i| s(i, k)).unwrap_or("")
                        };
                        let sum = match name {
                            "Bash" => format!("$ {}", first_line(inp("command"), 88)),
                            "Read" => format!("R {}", first_line(inp("file_path"), 80)),
                            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                                format!("E {}", first_line(inp("file_path"), 80))
                            }
                            "Task" | "Agent" => {
                                format!("⚑ {}", first_line(inp("description"), 80))
                            }
                            "Skill" => format!("◆ {}", inp("skill")),
                            n if n.starts_with("mcp__") => {
                                let mut parts = n.strip_prefix("mcp__").unwrap().splitn(2, "__");
                                format!(
                                    "◇ {}·{}",
                                    parts.next().unwrap_or("?"),
                                    parts.next().unwrap_or("?")
                                )
                            }
                            n => format!("▸ {n}"),
                        };
                        return Some(sum);
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

fn read_tail(p: &Path, mtime_ms: i64) -> Option<OverviewSession> {
    let mut f = File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(65_536);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // drop the partial first line
    }

    let mut cwd = String::new();
    let mut branch = String::new();
    let mut title = String::new();
    let mut model = String::new();
    let mut last_prompt = String::new();
    let mut actions: Vec<(i64, String)> = Vec::new();

    // main-chain state tracking, mirroring App::status()
    #[derive(PartialEq)]
    enum K {
        Other,
        Prompt,
        Reply,
        Tool,
        TurnEnd,
    }
    let mut last_kind = K::Other;
    let mut pending: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let sidechain = v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false)
            || v.get("agentId").is_some();
        if !sidechain {
            match s(&v, "type").unwrap_or("") {
                "system" => {
                    if s(&v, "subtype") == Some("turn_duration") {
                        last_kind = K::TurnEnd;
                    }
                }
                "assistant" => {
                    if let Some(bs) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array()) {
                        for b in bs {
                            match s(b, "type").unwrap_or("") {
                                "text" => {
                                    if s(b, "text").map(|t| !t.trim().is_empty()).unwrap_or(false) {
                                        last_kind = K::Reply;
                                    }
                                }
                                "tool_use" => {
                                    if let Some(id) = s(b, "id") {
                                        pending.insert(id.to_string());
                                    }
                                    last_kind = K::Tool;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                "user" => {
                    let c = v.get("message").and_then(|m| m.get("content"));
                    let mut had_result = false;
                    if let Some(arr) = c.and_then(|x| x.as_array()) {
                        for b in arr {
                            if s(b, "type") == Some("tool_result") {
                                had_result = true;
                                if let Some(id) = s(b, "tool_use_id") {
                                    pending.remove(id);
                                }
                            }
                        }
                    }
                    if had_result {
                        last_kind = K::Tool;
                    } else if v.get("isMeta").and_then(|x| x.as_bool()) != Some(true)
                        && v.get("isCompactSummary").and_then(|x| x.as_bool()) != Some(true)
                    {
                        let txt = match c {
                            Some(Value::String(t)) => t.clone(),
                            _ => String::new(),
                        };
                        if !txt.trim().is_empty() && !txt.trim_start().starts_with('<') {
                            last_kind = K::Prompt;
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(c) = s(&v, "cwd") {
            cwd = c.to_string();
        }
        if let Some(b) = s(&v, "gitBranch") {
            if !b.is_empty() {
                branch = b.to_string();
            }
        }
        if let Some(t) = s(&v, "aiTitle") {
            title = t.to_string();
        }
        if let Some(m) = v.get("message").and_then(|m| s(m, "model")) {
            model = m.to_string();
        }
        let ts = parse_ts(&v).unwrap_or(mtime_ms);
        if let Some(sum) = summarize(&v) {
            if sum.starts_with('❯') {
                last_prompt = sum.clone();
            }
            actions.push((ts, sum));
        }
    }

    // trivial session: nothing action-shaped in the tail
    if actions.is_empty() {
        return None;
    }
    let n = actions.len();
    let actions = actions.split_off(n.saturating_sub(5));
    if title.is_empty() {
        title = last_prompt.trim_start_matches("❯ ").to_string();
    }
    // colour by what the session is DOING, not by how recently its file moved
    let age = now_ms() - mtime_ms;
    let state = if last_kind == K::Tool && !pending.is_empty() && age > 10_000 {
        SessState::Stalled
    } else if age > 600_000 {
        SessState::Idle
    } else if last_kind == K::TurnEnd
        || (last_kind == K::Reply && age > 20_000)
        || (last_kind == K::Prompt && age > 45_000)
        || age > 60_000
    {
        SessState::Waiting
    } else {
        SessState::Working
    };
    Some(OverviewSession {
        state,
        id: p.file_stem()?.to_string_lossy().to_string(),
        cwd,
        branch,
        title: truncate_chars(&title, 60),
        model,
        mtime_ms,
        actions,
    })
}

/// All non-trivial sessions on this machine with activity inside the window.
pub fn scan(window_ms: i64) -> Vec<OverviewSession> {
    let now = now_ms();
    let projects = discover::home().join(".claude").join("projects");
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(&projects) else { return out };
    for proj in rd.flatten() {
        let pd = proj.path();
        if !pd.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&pd) else { continue };
        for e in files.flatten() {
            let p: PathBuf = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if !md.is_file() || md.len() < 8_000 {
                continue; // trivial: barely any transcript
            }
            let mtime_ms = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if now - mtime_ms > window_ms {
                continue;
            }
            if let Some(sess) = read_tail(&p, mtime_ms) {
                out.push(sess);
            }
        }
    }
    out.sort_by_key(|o| -o.mtime_ms);
    out
}
