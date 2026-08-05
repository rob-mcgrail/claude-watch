use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::{now_ms, truncate_chars};

#[derive(Serialize, Deserialize)]
pub struct HauntRun {
    pub source: String, // "roadmap" | "sites"
    pub label: String,  // project slug / site repo name
    pub what: String,   // run mode + stories / maintenance task
    pub status: String,
    pub running: bool,
    pub ok: bool,
    pub created_ms: i64,
}

#[derive(Default)]
pub struct HauntState {
    pub runs: Vec<HauntRun>,
    pub roadmap_err: Option<String>,
    pub sites_err: Option<String>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// Run a CLI expecting JSON on stdout; degrade to a readable error string
/// when the binary is missing, unauthenticated, or otherwise unhappy.
fn cli_json(cmd: &str, args: &[&str]) -> Result<Value, String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|_| format!("{cmd}: CLI not installed"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err
        };
        return Err(truncate_chars(&msg, 120));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("{cmd}: bad json ({e})"))
}

/// Parse RFC3339 or "YYYY-MM-DD HH:MM:SS" (UTC) timestamps.
fn parse_flex_ms(t: &str) -> i64 {
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
        return d.timestamp_millis();
    }
    if let Ok(n) = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S") {
        return n.and_utc().timestamp_millis();
    }
    0
}

/// One roadmap RunSummary → a card row, or None if outside the window.
fn roadmap_run(r: &Value, label: String, now: i64, window: i64) -> Option<HauntRun> {
    let created = parse_flex_ms(s(r, "created_at"));
    let status = s(r, "status").to_string();
    let unfinished = r.get("completed_at").map(|x| x.is_null()).unwrap_or(false);
    let running = unfinished && matches!(status.as_str(), "in_progress" | "pending" | "blocked");
    if !running && now - created > window {
        return None;
    }
    let nstories = r
        .get("story_slugs")
        .and_then(|x| x.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let what = if nstories > 0 {
        format!("{} · {} stories", s(r, "mode"), nstories)
    } else {
        s(r, "mode").to_string()
    };
    Some(HauntRun {
        source: "roadmap".to_string(),
        label,
        what,
        status: status.clone(),
        running,
        ok: status == "done",
        created_ms: created,
    })
}

/// Blocking fetch of roadmap + sites delivery runs — background thread only.
pub fn fetch() -> HauntState {
    let now = now_ms();
    let window = 16 * 3_600_000i64;
    let mut st = HauntState::default();
    let empty = vec![];

    // sites.haunt.digital maintenance runs
    match cli_json("sites", &["runs", "--limit", "30", "--json"]) {
        Ok(v) => {
            for r in v.as_array().unwrap_or(&empty) {
                let started = parse_flex_ms(s(r, "started_at"));
                let running = r.get("finished_at").map(|x| x.is_null()).unwrap_or(true);
                if !running && now - started > window {
                    continue;
                }
                let exit = r.get("exit_code").and_then(|x| x.as_i64());
                st.runs.push(HauntRun {
                    source: "sites".to_string(),
                    label: s(r, "repo_name").to_string(),
                    what: s(r, "task").to_string(),
                    status: if running {
                        "running".to_string()
                    } else if exit == Some(0) {
                        "ok".to_string()
                    } else {
                        format!("exit {}", exit.unwrap_or(-1))
                    },
                    running,
                    ok: exit == Some(0),
                    created_ms: started,
                });
            }
        }
        Err(e) => st.sites_err = Some(e),
    }

    // roadmaps.haunt.digital delivery runs: one global call (/api/runs),
    // falling back to a per-project scan on CLIs without `runs --all`
    match cli_json("roadmap", &["runs", "--all", "--limit", "60", "--json"]) {
        Ok(v) => {
            for r in v.as_array().unwrap_or(&empty) {
                let label = s(r, "project_slug").to_string();
                if let Some(run) = roadmap_run(r, label, now, window) {
                    st.runs.push(run);
                }
            }
        }
        Err(global_err) => match cli_json("roadmap", &["projects", "--json"]) {
            Ok(v) => {
                let mut slugs: Vec<(i64, String)> = v
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter_map(|p| {
                        let slug = ["slug", "name", "id"]
                            .iter()
                            .map(|k| s(p, k))
                            .find(|x| !x.is_empty())?
                            .to_string();
                        let upd = ["updated_at", "updatedAt", "updated", "last_activity_at"]
                            .iter()
                            .map(|k| parse_flex_ms(s(p, k)))
                            .max()
                            .unwrap_or(0);
                        Some((upd, slug))
                    })
                    .collect();
                slugs.sort_by_key(|(u, _)| -u);
                for (_, slug) in slugs.into_iter().take(6) {
                    let Ok(rv) = cli_json("roadmap", &["--slug", &slug, "runs", "--json"]) else {
                        continue;
                    };
                    for r in rv.as_array().unwrap_or(&empty) {
                        if let Some(run) = roadmap_run(r, slug.clone(), now, window) {
                            st.runs.push(run);
                        }
                    }
                }
            }
            Err(_) => st.roadmap_err = Some(global_err),
        },
    }

    st.runs.sort_by_key(|r| (!r.running, -r.created_ms));
    st
}
