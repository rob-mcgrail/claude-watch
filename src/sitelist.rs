use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::now_ms;

/// One row of the sites registry, flattened to what the panel shows.
#[derive(Clone, Serialize, Deserialize)]
pub struct SiteRow {
    pub repo: String,
    /// None when the site has never been patched — which sorts first, since
    /// that is the most alarming thing this panel can tell you.
    pub last_patched_ms: Option<i64>,
    pub stack: String,
    pub runtime: String,
    /// Earliest runtime EOL across the site's runtimes.
    pub eol_ms: Option<i64>,
    pub sla: bool,
    pub has_tests: bool,
    pub internal: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct SitesState {
    pub rows: Vec<SiteRow>,
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub fetching: bool,
    #[serde(default)]
    pub fetched_at_ms: i64,
}

fn strs(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_ms(t: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn opt_ms(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_str()).and_then(parse_ms)
}

/// Blocking read of the registry — one local CLI call, but it still runs on a
/// background thread so a slow network never stalls a frame.
pub fn fetch() -> SitesState {
    let mut st = SitesState::default();
    // never let a background poll trigger the CLI's silent self-update
    let out = match Command::new("sites")
        .args(["list", "--json"])
        .env("SITES_NO_AUTO_UPDATE", "1")
        .output()
    {
        Ok(o) => o,
        Err(_) => {
            st.error = Some("sites: CLI not installed".to_string());
            return st;
        }
    };
    if !out.status.success() {
        st.error = Some(crate::cve::sites_err(&String::from_utf8_lossy(&out.stderr)));
        return st;
    }
    let v: Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(e) => {
            st.error = Some(format!("sites: bad json ({e})"));
            return st;
        }
    };
    let empty = vec![];
    for s in v.as_array().unwrap_or(&empty) {
        let Some(repo) = s.get("repo_name").and_then(|x| x.as_str()) else {
            continue;
        };
        // stack_versions is the specific one ("next-16"); fall back to the
        // coarse tags when a site has not recorded versions yet
        let versions = strs(s, "stack_versions");
        let stack = if versions.is_empty() {
            strs(s, "stack_tags").join(", ")
        } else {
            versions.join(", ")
        };
        let eol_ms = strs(s, "runtime_eol_dates")
            .iter()
            .filter_map(|d| parse_ms(d).or_else(|| parse_ms(&format!("{d}T00:00:00Z"))))
            .min();
        st.rows.push(SiteRow {
            repo: repo.to_string(),
            last_patched_ms: opt_ms(s, "last_patched"),
            stack,
            runtime: strs(s, "runtime_versions").join(", "),
            eol_ms,
            sla: s.get("under_proactive_sla").and_then(|x| x.as_bool()).unwrap_or(false),
            has_tests: s.get("has_tests").and_then(|x| x.as_bool()).unwrap_or(false),
            internal: s.get("internal").and_then(|x| x.as_bool()).unwrap_or(false),
        });
    }
    // stalest first: never-patched, then oldest — this reads as a work queue
    st.rows.sort_by_key(|r| r.last_patched_ms.unwrap_or(i64::MIN));
    st.fetched_at_ms = now_ms();
    st
}
