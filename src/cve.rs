use std::collections::HashMap;
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::{now_ms, truncate_chars};

/// One CVE as it lands across the estate. The org-wide Dependabot endpoint
/// returns tens of thousands of alerts — most of them against throwaway repos —
/// so this is scoped to the sites registry and rolled up per advisory: the same
/// CVE in fourteen sites is one row with a blast radius, not fourteen rows.
#[derive(Serialize, Deserialize)]
pub struct CveRollup {
    pub id: String, // CVE-… , or the GHSA id when no CVE is assigned
    pub severity: String,
    pub cvss: f64,
    pub package: String,
    pub ecosystem: String,
    pub sites: usize,
    pub oldest_ms: i64,
}

#[derive(Serialize, Deserialize)]
pub struct SiteTally {
    pub repo: String,
    pub critical: usize,
    pub high: usize,
}

#[derive(Default, Serialize, Deserialize)]
pub struct CveState {
    pub worst: Vec<CveRollup>,
    pub by_site: Vec<SiteTally>,
    pub total: usize,
    pub critical: usize,
    pub distinct: usize,
    pub sites_scanned: usize,
    pub sites_affected: usize,
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub fetching: bool,
    /// Round-trips through the cache — it is what tells another instance
    /// whether the scan on disk is still worth adopting.
    #[serde(default)]
    pub fetched_at_ms: i64,
}

/// How many rows each section keeps.
const KEEP_CVES: usize = 25;
const KEEP_SITES: usize = 15;
/// Concurrent `gh` subprocesses. Eight scans the whole registry in ~8s; more
/// only trades memory for GitHub's secondary rate limiter.
const FANOUT: usize = 8;

fn parse_flex_ms(t: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(t)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

/// `git@github.com:haunt-digital/foo.git` / `https://github.com/haunt-digital/foo`
/// → `haunt-digital`. The registry is one org today, but the owner is data.
fn owner_of(repo_url: &str) -> Option<String> {
    let after_host = repo_url
        .rsplit_once("github.com")
        .map(|(_, rest)| rest.trim_start_matches([':', '/']))?;
    after_host.split('/').next().map(str::to_string)
}

/// The managed sites, as (owner, repo). Anything not in this registry is a
/// throwaway as far as this panel is concerned.
fn registry() -> Result<Vec<(String, String)>, String> {
    let out = Command::new("sites")
        .args(["list", "--json"])
        .output()
        .map_err(|_| "sites: CLI not installed".to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(truncate_chars(&err, 120));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("sites: bad json ({e})"))?;
    let empty = vec![];
    Ok(v
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|s| {
            let repo = s.get("repo_name").and_then(|x| x.as_str())?.to_string();
            let owner = s
                .get("repo_url")
                .and_then(|x| x.as_str())
                .and_then(owner_of)
                .unwrap_or_else(|| "haunt-digital".to_string());
            Some((owner, repo))
        })
        .collect())
}

/// severity, cvss, id, ecosystem, package, created — TSV keeps the payload
/// small, since the full advisory bodies run to several KB each.
const JQ: &str = ".[] | [\
    .security_advisory.severity, \
    ((.security_advisory.cvss.score // 0) as $v3 | if $v3 > 0 then $v3 else (.security_advisory.cvss_severities.cvss_v4.score // 0) end), \
    (.security_advisory.cve_id // .security_advisory.ghsa_id), \
    .dependency.package.ecosystem, \
    .dependency.package.name, \
    .created_at\
    ] | @tsv";

struct Alert {
    severity: String,
    cvss: f64,
    id: String,
    ecosystem: String,
    package: String,
    created_ms: i64,
}

/// Open critical+high alerts for one repo. A repo with Dependabot disabled (or
/// no alerts) simply yields none — that is not an error worth surfacing.
fn repo_alerts(owner: &str, repo: &str) -> Result<Vec<Alert>, String> {
    let path = format!(
        "/repos/{owner}/{repo}/dependabot/alerts?state=open&severity=critical,high&per_page=100"
    );
    let out = Command::new("gh")
        .args(["api", "--paginate", &path, "--jq", JQ])
        .output()
        .map_err(|e| format!("gh not runnable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 6 {
                return None;
            }
            Some(Alert {
                severity: f[0].to_string(),
                cvss: f[1].parse().unwrap_or(0.0),
                id: f[2].to_string(),
                ecosystem: f[3].to_string(),
                package: f[4].to_string(),
                created_ms: parse_flex_ms(f[5]),
            })
        })
        .collect())
}

/// Blocking scan of every registered site — background thread only, ~8s.
pub fn fetch() -> CveState {
    let mut st = CveState::default();
    let sites = match registry() {
        Ok(s) => s,
        Err(e) => {
            st.error = Some(e);
            return st;
        }
    };
    st.sites_scanned = sites.len();

    // chunked fan-out: the whole registry at once would be 40 concurrent
    // subprocesses, and GitHub starts pushing back well before that helps
    let mut per_site: Vec<(String, Vec<Alert>)> = Vec::new();
    for chunk in sites.chunks(FANOUT) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|(owner, repo)| {
                let (owner, repo) = (owner.clone(), repo.clone());
                std::thread::spawn(move || {
                    let got = repo_alerts(&owner, &repo);
                    (repo, got)
                })
            })
            .collect();
        for h in handles {
            let Ok((repo, got)) = h.join() else { continue };
            match got {
                Ok(alerts) => per_site.push((repo, alerts)),
                // one unreadable repo shouldn't blank the panel; keep the last
                // error as a footnote and carry on
                Err(e) => st.error = Some(truncate_chars(&e, 110)),
            }
        }
    }

    let mut rollups: HashMap<String, CveRollup> = HashMap::new();
    let mut seen_site: HashMap<String, Vec<String>> = HashMap::new();
    for (repo, alerts) in &per_site {
        let (mut crit, mut high) = (0, 0);
        for a in alerts {
            if a.severity == "critical" {
                crit += 1;
            } else {
                high += 1;
            }
            let r = rollups.entry(a.id.clone()).or_insert_with(|| CveRollup {
                id: a.id.clone(),
                severity: a.severity.clone(),
                cvss: a.cvss,
                package: a.package.clone(),
                ecosystem: a.ecosystem.clone(),
                sites: 0,
                oldest_ms: a.created_ms,
            });
            r.cvss = r.cvss.max(a.cvss);
            r.oldest_ms = r.oldest_ms.min(a.created_ms);
            seen_site.entry(a.id.clone()).or_default().push(repo.clone());
        }
        st.total += crit + high;
        st.critical += crit;
        if crit + high > 0 {
            st.sites_affected += 1;
            st.by_site.push(SiteTally { repo: repo.clone(), critical: crit, high });
        }
    }
    // a CVE hitting the same site through two manifests is still one site
    for (id, mut repos) in seen_site {
        repos.sort();
        repos.dedup();
        if let Some(r) = rollups.get_mut(&id) {
            r.sites = repos.len();
        }
    }

    st.distinct = rollups.len();
    let mut worst: Vec<CveRollup> = rollups.into_values().collect();
    // critical first, then blast radius, then score: what to fix once to fix
    // it everywhere
    worst.sort_by(|a, b| {
        let rank = |s: &str| if s == "critical" { 0 } else { 1 };
        rank(&a.severity)
            .cmp(&rank(&b.severity))
            .then(b.sites.cmp(&a.sites))
            .then(b.cvss.total_cmp(&a.cvss))
            .then(a.oldest_ms.cmp(&b.oldest_ms))
    });
    worst.truncate(KEEP_CVES);
    st.worst = worst;

    st.by_site.sort_by(|a, b| {
        b.critical
            .cmp(&a.critical)
            .then(b.high.cmp(&a.high))
            .then(a.repo.cmp(&b.repo))
    });
    st.by_site.truncate(KEEP_SITES);
    st.fetched_at_ms = now_ms();
    st
}
