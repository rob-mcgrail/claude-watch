use std::collections::HashMap;
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::{now_ms, truncate_chars};

/// One advisory as it lands across the estate. The org-wide Dependabot endpoint
/// returns tens of thousands of alerts — most of them against throwaway repos —
/// so this is scoped to the sites registry and rolled up per advisory: the same
/// CVE in fourteen sites is one row with a blast radius, not fourteen rows.
#[derive(Clone, Serialize, Deserialize)]
pub struct CveRollup {
    pub id: String, // CVE-… , or the GHSA id when no CVE is assigned
    pub severity: String,
    pub cvss: f64,
    pub package: String,
    pub ecosystem: String,
    /// Affected site count per filter key. One CVE can span both npm buckets —
    /// minimist turns up in a Next app and in a SilverStripe theme build — so
    /// the blast radius is per bucket, not a single number.
    pub sites_by_key: Vec<(String, usize)>,
    pub oldest_ms: i64,
}

/// A rollup resolved against the current filter.
#[derive(Clone)]
pub struct CveRow {
    pub id: String,
    pub severity: String,
    pub cvss: f64,
    pub package: String,
    pub ecosystem: String,
    pub sites: usize,
    pub oldest_ms: i64,
}

/// npm means two very different things across this estate: the runtime of a
/// Next/Bun app, or just the webpack/gulp toolchain that precompiles assets for
/// a Rails or SilverStripe site. A CVE in the first is served to users; in the
/// second it never leaves the build. `runtime_versions` in the registry is what
/// separates them, and the manifest paths agree — the build-only sites carry
/// their lockfiles under themes/ and app/cms/javascript/.
pub const NPM_SERVED: &str = "npm · served";
pub const NPM_BUILD: &str = "npm · toolchain";
/// A Gemfile on a site that does not run Ruby is there to deploy it —
/// Capistrano and its dependency tree. Worth fixing; not worth alarming about.
pub const CAP: &str = "cap";

/// The rule of thumb: **an npm tree living inside a Ruby or PHP app directory
/// is that app's asset pipeline.** A CVE there is webpack/gulp build machinery;
/// a CVE in a standalone package.json is a Next or Bun app serving users.
///
/// It resolves the monorepos without a special case. `consumer` has npm at
/// `mms/` and `replatform/`, and a Gemfile at `mms/` — so mms is the Rails
/// theme build and replatform is the react-router app. `powerswitch` has
/// `gilbert/app/client` under `gilbert/Gemfile.lock`, and `romeo/` standing
/// alone with next in it. Same rule, both right, and no extra API calls: those
/// composer and rubygems manifest paths arrive with the alerts.
///
/// `roots` are the app directories; "" means the whole repo is one app, which
/// is the ordinary case for a SilverStripe or Rails site.
fn npm_is_toolchain(manifest_dir: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| {
        r.is_empty() || manifest_dir == r || manifest_dir.starts_with(&format!("{r}/"))
    })
}

/// Directory part of `themes/x/package-lock.json`, or "" at the repo root.
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// Counts per (site, ecosystem) rather than per site, so the ecosystem filter
/// can re-tally without another scan.
#[derive(Clone, Serialize, Deserialize)]
pub struct SiteEco {
    pub repo: String,
    pub ecosystem: String,
    pub critical: usize,
    pub high: usize,
}

/// Head of a repo's deploy-production branch — what actually went out, and when.
#[derive(Clone, Serialize, Deserialize)]
pub struct Deploy {
    pub repo: String,
    pub when_ms: i64,
    pub author: String,
    pub subject: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct CveState {
    /// Every rollup, not just the visible ones — the ecosystem filter narrows
    /// this at render time, and re-scanning to change filter would be absurd.
    pub rollups: Vec<CveRollup>,
    pub site_eco: Vec<SiteEco>,
    /// Ecosystems present, most alerts first; the `<`/`>` filter cycles these.
    pub ecosystems: Vec<String>,
    pub deploys: Vec<Deploy>,
    pub sites_scanned: usize,
    /// Registry entries GitHub would not show us — renamed, deleted, or private
    /// to this token. Worth a footnote, not a red warning.
    #[serde(default)]
    pub unreadable: usize,
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub fetching: bool,
    /// Round-trips through the cache — it is what tells another instance
    /// whether the scan on disk is still worth adopting.
    #[serde(default)]
    pub fetched_at_ms: i64,
}

/// What the panel shows for one ecosystem selection.
pub struct Filtered {
    pub worst: Vec<CveRow>,
    pub by_site: Vec<(String, usize, usize)>,
    pub total: usize,
    pub critical: usize,
    pub distinct: usize,
    pub sites_affected: usize,
}

impl CveState {
    /// Label for a filter index: 0 is everything, then one per ecosystem.
    pub fn eco_label(&self, idx: usize) -> String {
        if idx == 0 {
            "all".to_string()
        } else {
            self.ecosystems.get(idx - 1).cloned().unwrap_or_else(|| "all".to_string())
        }
    }

    pub fn eco_count(&self) -> usize {
        self.ecosystems.len() + 1
    }

    /// Re-derive the visible rows for one ecosystem selection. Cheap enough to
    /// run every frame: a few hundred rollups and a few dozen tallies.
    pub fn filtered(&self, idx: usize, keep_cves: usize, keep_sites: usize) -> Filtered {
        let eco = (idx > 0).then(|| self.eco_label(idx));
        let keep = |e: &str| eco.as_deref().map(|f| f == e).unwrap_or(true);

        // the blast radius depends on the filter, so the ranking has to be
        // recomputed with it rather than reused from the scan
        let mut worst: Vec<CveRow> = self
            .rollups
            .iter()
            .filter_map(|r| {
                let sites: usize =
                    r.sites_by_key.iter().filter(|(k, _)| keep(k)).map(|(_, n)| n).sum();
                (sites > 0).then(|| CveRow {
                    id: r.id.clone(),
                    severity: r.severity.clone(),
                    cvss: r.cvss,
                    package: r.package.clone(),
                    ecosystem: r.ecosystem.clone(),
                    sites,
                    oldest_ms: r.oldest_ms,
                })
            })
            .collect();
        worst.sort_by(|a, b| {
            let rank = |s: &str| if s == "critical" { 0 } else { 1 };
            rank(&a.severity)
                .cmp(&rank(&b.severity))
                .then(b.sites.cmp(&a.sites))
                .then(b.cvss.total_cmp(&a.cvss))
                .then(a.oldest_ms.cmp(&b.oldest_ms))
        });
        let distinct = worst.len();
        worst.truncate(keep_cves);

        let mut tally: HashMap<&str, (usize, usize)> = HashMap::new();
        for s in self.site_eco.iter().filter(|s| keep(&s.ecosystem)) {
            let e = tally.entry(s.repo.as_str()).or_insert((0, 0));
            e.0 += s.critical;
            e.1 += s.high;
        }
        let critical: usize = tally.values().map(|(c, _)| c).sum();
        let high: usize = tally.values().map(|(_, h)| h).sum();
        let sites_affected = tally.values().filter(|(c, h)| c + h > 0).count();
        let mut by_site: Vec<(String, usize, usize)> =
            tally.into_iter().map(|(r, (c, h))| (r.to_string(), c, h)).collect();
        by_site.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        by_site.truncate(keep_sites);

        Filtered {
            worst,
            by_site,
            total: critical + high,
            critical,
            distinct,
            sites_affected,
        }
    }
}

/// How many rows each section keeps.
pub const KEEP_CVES: usize = 25;
pub const KEEP_SITES: usize = 15;
const KEEP_DEPLOYS: usize = 12;
/// Concurrent `gh` subprocesses. Eight scans the whole registry in ~15s; more
/// only trades memory for GitHub's secondary rate limiter.
const FANOUT: usize = 8;

fn strs(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect())
        .unwrap_or_default()
}

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

pub struct Site {
    pub owner: String,
    pub repo: String,
    /// Runs PHP or Ruby in production, so an npm tree here may be a build step.
    pub server_side: bool,
}

/// The managed sites. Anything not in this registry is a throwaway as far as
/// this panel is concerned.
fn registry() -> Result<Vec<Site>, String> {
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
            let starts = |xs: &[String], ps: &[&str]| {
                xs.iter().any(|v| ps.iter().any(|p| v.starts_with(p)))
            };
            // stack_tags is the fallback for sites with no recorded versions
            let server_side = starts(&strs(s, "runtime_versions"), &["php", "ruby", "python"])
                || starts(&strs(s, "stack_tags"), &["php", "silverstripe", "rails", "ruby"]);
            Some(Site { owner, repo, server_side })
        })
        .collect())
}

/// severity, cvss, id, ecosystem, package, created — TSV keeps the payload
/// small, since the full advisory bodies run to several KB each.
const JQ_ALERTS: &str = ".[] | [\
    .security_advisory.severity, \
    ((.security_advisory.cvss.score // 0) as $v3 | if $v3 > 0 then $v3 else (.security_advisory.cvss_severities.cvss_v4.score // 0) end), \
    (.security_advisory.cve_id // .security_advisory.ghsa_id), \
    .dependency.package.ecosystem, \
    .dependency.package.name, \
    .created_at, \
    (.dependency.manifest_path // \"\")\
    ] | @tsv";

const JQ_BRANCH: &str = "[.commit.commit.committer.date, \
    (.commit.commit.author.name // \"\"), \
    (.commit.commit.message | split(\"\\n\")[0])] | @tsv";

struct Alert {
    severity: String,
    cvss: f64,
    id: String,
    ecosystem: String,
    package: String,
    created_ms: i64,
    manifest_path: String,
}

fn gh_lines(args: &[&str]) -> Result<Vec<String>, String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("gh not runnable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

/// Open critical+high alerts for one repo. A repo with Dependabot disabled (or
/// no alerts) simply yields none — that is not an error worth surfacing.
fn repo_alerts(owner: &str, repo: &str) -> Result<Vec<Alert>, String> {
    let path = format!(
        "/repos/{owner}/{repo}/dependabot/alerts?state=open&severity=critical,high&per_page=100"
    );
    Ok(gh_lines(&["api", "--paginate", &path, "--jq", JQ_ALERTS])?
        .iter()
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
                manifest_path: f.get(6).unwrap_or(&"").to_string(),
            })
        })
        .collect())
}

/// Head of deploy-production, or None when the repo has no such branch. The
/// registry's `deploy_branches` drifts from what is actually on GitHub — it
/// listed 31 while 34 repos had the branch — so ask GitHub, not the registry.
fn repo_deploy(owner: &str, repo: &str) -> Option<Deploy> {
    let path = format!("/repos/{owner}/{repo}/branches/deploy-production");
    let lines = gh_lines(&["api", &path, "--jq", JQ_BRANCH]).ok()?;
    let f: Vec<&str> = lines.first()?.split('\t').collect();
    if f.len() < 3 {
        return None;
    }
    Some(Deploy {
        repo: repo.to_string(),
        when_ms: parse_flex_ms(f[0]),
        author: f[1].to_string(),
        subject: f[2].to_string(),
    })
}

/// Blocking scan of every registered site — background thread only, ~15s.
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
    // subprocesses, and GitHub starts pushing back well before that helps.
    // Alerts and the deploy head share a thread, so this is one pass, not two.
    type Scan = (String, bool, Result<Vec<Alert>, String>, Option<Deploy>);
    let mut scans: Vec<Scan> = Vec::new();
    for chunk in sites.chunks(FANOUT) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|site| {
                let (owner, repo, server_side) =
                    (site.owner.clone(), site.repo.clone(), site.server_side);
                std::thread::spawn(move || {
                    let alerts = repo_alerts(&owner, &repo);
                    let deploy = repo_deploy(&owner, &repo);
                    (repo, server_side, alerts, deploy)
                })
            })
            .collect();
        for h in handles {
            if let Ok(s) = h.join() {
                scans.push(s);
            }
        }
    }

    let mut rollups: HashMap<String, CveRollup> = HashMap::new();
    // (cve, filter key) -> sites, deduped: one CVE can reach a site through two
    // manifests and that is still one site
    let mut cve_sites: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut eco_alerts: HashMap<String, usize> = HashMap::new();
    for (repo, server_side, alerts, deploy) in &scans {
        if let Some(d) = deploy {
            st.deploys.push(d.clone());
        }
        let alerts = match alerts {
            Ok(a) => a,
            // a repo the registry knows and GitHub does not is drift, not a
            // failure; anything else (auth, rate limit) is worth showing
            Err(e) => {
                if e.contains("404") || e.contains("Not Found") {
                    st.unreadable += 1;
                } else {
                    st.error = Some(truncate_chars(e, 110));
                }
                continue;
            }
        };
        // where the Ruby/PHP apps live in this repo, from their own manifests.
        // Only worth looking on a site the registry says actually runs one:
        // a Next or Bun repo can carry a root Gemfile for deploy tooling, and
        // that must not turn its whole npm tree into "build only".
        let mut roots: Vec<String> = if *server_side {
            alerts
                .iter()
                .filter(|a| matches!(a.ecosystem.as_str(), "composer" | "rubygems"))
                .map(|a| dir_of(&a.manifest_path))
                .collect()
        } else {
            Vec::new()
        };
        roots.sort();
        roots.dedup();
        // an app in a subdirectory means the repo root is scaffolding, not a
        // third app — otherwise a root deploy Gemfile would swallow the lot
        if roots.len() > 1 {
            roots.retain(|r| !r.is_empty());
        }
        // a server-side site with no located app is one app filling the repo —
        // the ordinary single-app SilverStripe or Rails layout
        if roots.is_empty() && *server_side {
            roots.push(String::new());
        }

        let mut per_eco: HashMap<String, (usize, usize)> = HashMap::new();
        for a in alerts {
            let key = match a.ecosystem.as_str() {
                "npm" => if npm_is_toolchain(&dir_of(&a.manifest_path), &roots) {
                    NPM_BUILD
                } else {
                    NPM_SERVED
                }
                .to_string(),
                // same rule the other way round: a Ruby or PHP manifest with no
                // app around it is deploy tooling, not the application
                "rubygems" | "composer" if !*server_side => CAP.to_string(),
                e => e.to_string(),
            };
            let e = per_eco.entry(key.clone()).or_insert((0, 0));
            if a.severity == "critical" {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
            *eco_alerts.entry(key.clone()).or_insert(0) += 1;

            let r = rollups.entry(a.id.clone()).or_insert_with(|| CveRollup {
                id: a.id.clone(),
                severity: a.severity.clone(),
                cvss: a.cvss,
                package: a.package.clone(),
                ecosystem: a.ecosystem.clone(),
                sites_by_key: Vec::new(),
                oldest_ms: a.created_ms,
            });
            r.cvss = r.cvss.max(a.cvss);
            r.oldest_ms = r.oldest_ms.min(a.created_ms);
            cve_sites
                .entry((a.id.clone(), key))
                .or_default()
                .push(repo.clone());
        }
        for (eco, (critical, high)) in per_eco {
            st.site_eco.push(SiteEco {
                repo: repo.clone(),
                ecosystem: eco,
                critical,
                high,
            });
        }
    }

    for ((id, key), mut repos) in cve_sites {
        repos.sort();
        repos.dedup();
        if let Some(r) = rollups.get_mut(&id) {
            r.sites_by_key.push((key, repos.len()));
        }
    }
    // ranking depends on the active filter, so `filtered` does the sorting
    st.rollups = rollups.into_values().collect();

    let mut ecos: Vec<(String, usize)> = eco_alerts.into_iter().collect();
    ecos.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    st.ecosystems = ecos.into_iter().map(|(e, _)| e).collect();

    st.deploys.sort_by_key(|d| -d.when_ms);
    st.deploys.truncate(KEEP_DEPLOYS);
    st.fetched_at_ms = now_ms();
    st
}
