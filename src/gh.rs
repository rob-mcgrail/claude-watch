use std::process::Command;

use serde_json::Value;

use serde::{Deserialize, Serialize};

use crate::app::now_ms;

#[derive(Serialize, Deserialize)]
pub struct GhRun {
    pub repo: String,
    pub workflow: String,
    pub branch: String,
    pub status: String,     // queued | in_progress | completed
    pub conclusion: String, // success | failure | cancelled | ...
    pub created_ms: i64,
}

#[derive(Serialize, Deserialize)]
pub struct GhPr {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: String, // OPEN | MERGED | CLOSED
    pub draft: bool,
    pub created_ms: i64,
}

#[derive(Default)]
pub struct GhState {
    pub runs: Vec<GhRun>,
    pub prs: Vec<GhPr>,
    pub error: Option<String>,
    pub fetching: bool,
    pub fetched_at_ms: i64,
}

pub struct FetchResult {
    pub runs: Vec<GhRun>,
    pub prs: Vec<GhPr>,
    pub error: Option<String>,
}

fn gh_json(args: &[&str]) -> Result<Value, String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("gh not runnable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("bad gh json: {e}"))
}

fn iso_ms(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(|x| x.as_str())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// Blocking fetch — run this on a background thread only. Per-repo calls
/// fan out onto their own threads, so wall time ≈ the slowest single call.
pub fn fetch() -> FetchResult {
    let now = now_ms();
    let hour = 3_600_000i64;
    let mut error: Option<String> = None;
    let empty = vec![];

    // candidate repos: anything I can access (incl. org repos) pushed in
    // the last 24h — `gh repo list` only sees personally-owned repos
    let repos = match gh_json(&[
        "api",
        "user/repos?sort=pushed&per_page=50&affiliation=owner,collaborator,organization_member",
    ]) {
        Ok(v) => v,
        Err(e) => {
            return FetchResult { runs: vec![], prs: vec![], error: Some(e) };
        }
    };
    let candidates: Vec<String> = repos
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|r| now - iso_ms(r, "pushed_at") < 24 * hour)
        .filter_map(|r| r.get("full_name").and_then(|x| x.as_str()))
        .take(12)
        .map(str::to_string)
        .collect();

    // one thread per repo: workflow runs + PRs together
    type RepoResult = (String, Result<Value, String>, Result<Value, String>);
    let handles: Vec<std::thread::JoinHandle<RepoResult>> = candidates
        .iter()
        .map(|repo| {
            let repo = repo.clone();
            std::thread::spawn(move || {
                let runs = gh_json(&[
                    "run", "list", "-R", &repo, "--limit", "20", "--json",
                    "workflowName,headBranch,status,conclusion,createdAt",
                ]);
                let prs = gh_json(&[
                    "pr", "list", "-R", &repo, "--state", "all", "--limit", "15", "--json",
                    "number,title,author,state,isDraft,createdAt",
                ]);
                (repo, runs, prs)
            })
        })
        .collect();

    // the cross-repo PR search runs on this thread while the fan-out flies
    let since = chrono::DateTime::from_timestamp_millis(now - 6 * hour)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default();
    let search = gh_json(&[
        "search", "prs", "--involves", "@me", "--created", &format!(">={since}"),
        "--limit", "30", "--json", "number,title,author,repository,state,isDraft,createdAt",
    ]);

    let mut runs: Vec<GhRun> = Vec::new();
    let mut prs: Vec<GhPr> = Vec::new();
    for h in handles {
        let Ok((repo, runs_v, prs_v)) = h.join() else { continue };
        match runs_v {
            Ok(v) => {
                for r in v.as_array().unwrap_or(&empty) {
                    let status = s(r, "status");
                    let created = iso_ms(r, "createdAt");
                    let running = status == "in_progress" || status == "queued";
                    if !running && now - created > 4 * hour {
                        continue;
                    }
                    runs.push(GhRun {
                        repo: repo.clone(),
                        workflow: s(r, "workflowName"),
                        branch: s(r, "headBranch"),
                        status,
                        conclusion: s(r, "conclusion"),
                        created_ms: created,
                    });
                }
            }
            Err(e) => error = Some(e),
        }
        if let Ok(v) = prs_v {
            for p in v.as_array().unwrap_or(&empty) {
                let created = iso_ms(p, "createdAt");
                if now - created > 6 * hour {
                    continue;
                }
                prs.push(GhPr {
                    repo: repo.clone(),
                    number: p.get("number").and_then(|x| x.as_u64()).unwrap_or(0),
                    title: s(p, "title"),
                    author: p.get("author").map(|a| s(a, "login")).unwrap_or_default(),
                    state: s(p, "state"),
                    draft: p.get("isDraft").and_then(|x| x.as_bool()).unwrap_or(false),
                    created_ms: created,
                });
            }
        }
    }
    runs.sort_by_key(|r| (r.status == "completed", -r.created_ms));
    runs.truncate(20);

    if let Ok(v) = search {
        for p in v.as_array().unwrap_or(&empty) {
            let repo = p
                .get("repository")
                .map(|r| s(r, "nameWithOwner"))
                .unwrap_or_default();
            let number = p.get("number").and_then(|x| x.as_u64()).unwrap_or(0);
            if prs.iter().any(|x| x.repo == repo && x.number == number) {
                continue;
            }
            prs.push(GhPr {
                repo,
                number,
                title: s(p, "title"),
                author: p.get("author").map(|a| s(a, "login")).unwrap_or_default(),
                state: s(p, "state"),
                draft: p.get("isDraft").and_then(|x| x.as_bool()).unwrap_or(false),
                created_ms: iso_ms(p, "createdAt"),
            });
        }
    }
    prs.sort_by_key(|p| -p.created_ms);

    FetchResult { runs, prs, error }
}
