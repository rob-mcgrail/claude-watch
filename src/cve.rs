use std::collections::{HashMap, HashSet};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::{now_ms, truncate_chars};

/// Where a vulnerable dependency actually runs, as the registry classifies it.
///
/// This is the distinction the old gh-direct scan inferred for itself, by
/// looking at whether an npm manifest sat inside a composer/rubygems app
/// directory. The registry now decides it — per-site `depscope` glob rules, the
/// site's declared languages, the dev-dependency flag — so the heuristic and
/// its edge cases are gone, and a wrong call is fixed with `sites depscope`
/// rather than a new release of this binary.
///
/// `server` faces the internet. `build` never leaves CI. `deploy` is the
/// tooling that ships the site (what the old scan called `cap`).
const TIER_ORDER: [&str; 3] = ["server", "build", "deploy"];

/// One advisory across the estate. The same CVE in six sites is one row
/// listing six, which is also how the API groups it — the rollup that used to
/// happen here now arrives done.
#[derive(Clone, Serialize, Deserialize)]
pub struct Advisory {
    /// CVE id where one is assigned, else the GHSA id.
    pub id: String,
    pub summary: String,
    pub severity: String,
    pub cvss: f64,
    /// Probability of exploitation in the wild, 0–1. Absent for ~7% of
    /// advisories — a gap, not a zero, so it stays optional all the way to the
    /// render.
    pub epss: Option<f64>,
    pub published_ms: i64,
    /// True when a patched version exists. An unfixable critical is a
    /// different kind of problem from one nobody has got round to.
    pub fixable: bool,
    pub alerts: Vec<AlertRef>,
}

/// One advisory landing in one manifest of one site.
#[derive(Clone, Serialize, Deserialize)]
pub struct AlertRef {
    pub repo: String,
    pub tier: String,
    pub ecosystem: String,
    pub package: String,
    /// Why it landed in that tier — "rule romeo/**", "language php",
    /// "dev-dependency". Worth showing on a critical: it is the difference
    /// between "this serves users" and "a glob says so".
    pub scope: String,
    pub sla: bool,
    /// A patched version, where the advisory names one.
    pub first_patched: String,
}

/// Head of a repo's deploy-production branch — what actually went out, and
/// when. Still GitHub's to answer; the registry does not track it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Deploy {
    pub repo: String,
    pub when_ms: i64,
    pub author: String,
    pub subject: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct CveState {
    /// Every advisory at every tier. The filters narrow this at render time —
    /// re-fetching to change a filter would be absurd when the whole fleet is
    /// one 300ms call.
    pub advisories: Vec<Advisory>,
    /// Tiers present, in severity-of-exposure order.
    pub tiers: Vec<String>,
    /// Ecosystems present, most alerts first.
    pub ecosystems: Vec<String>,
    /// Sites in the registry, for the "N of M affected" denominator.
    pub sites_total: usize,
    /// When the registry last pulled from GitHub — the age that matters, since
    /// our own fetch is just a read of that.
    pub refreshed_ms: i64,
    /// The registry's own last-pull complaint, if it had one.
    #[serde(default)]
    pub refresh_error: Option<String>,
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub fetching: bool,
    pub fetched_at_ms: i64,
}

/// Deploy heads live apart from the advisories now: one is a 300ms read of the
/// registry, the other is ~25 GitHub calls. Sharing a fetch would have made the
/// fast half wait for the slow one.
#[derive(Default, Serialize, Deserialize)]
pub struct DeployState {
    pub deploys: Vec<Deploy>,
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub fetching: bool,
    pub fetched_at_ms: i64,
}

/// The four axes the panel filters on. Tier and ecosystem are orthogonal now —
/// the old single axis conflated them because the npm split *was* the tier.
#[derive(Clone, Copy)]
pub struct Filters {
    /// Index into `tiers`; `tiers.len()` means every tier.
    pub tier: usize,
    /// 0 is every ecosystem, then one per entry in `ecosystems`.
    pub eco: usize,
    /// 0 = critical+high, 1 = down to medium, 2 = everything.
    pub sev: usize,
    /// Only advisories with a patched version to move to.
    pub fixable: bool,
}

impl Default for Filters {
    fn default() -> Self {
        // server-tier, critical+high: what faces the internet and would wake
        // someone up. Everything else is one keystroke away.
        Self { tier: 0, eco: 0, sev: 0, fixable: false }
    }
}

/// An advisory resolved against the current filter — its sites and tiers are
/// only those that survived it.
#[derive(Clone)]
pub struct Row {
    pub id: String,
    pub summary: String,
    pub severity: String,
    pub cvss: f64,
    pub epss: Option<f64>,
    pub published_ms: i64,
    pub fixable: bool,
    pub package: String,
    pub ecosystem: String,
    pub sites: Vec<String>,
    pub tiers: Vec<String>,
    pub scope: String,
    pub first_patched: String,
}

#[derive(Clone)]
pub struct SiteTally {
    pub repo: String,
    pub critical: usize,
    pub high: usize,
    pub rest: usize,
    pub sla: bool,
}

pub struct Filtered {
    /// Criticals get the expanded treatment — there are only ever a handful,
    /// and each one is a decision.
    pub critical: Vec<Row>,
    pub worst: Vec<Row>,
    /// Ranked by EPSS rather than severity. This is the section the old panel
    /// could not have had: the fleet's most-likely-to-be-exploited advisory is
    /// a *medium*, and severity ranking buries it forever.
    pub exploitable: Vec<Row>,
    pub by_site: Vec<SiteTally>,
    pub advisories: usize,
    pub alerts: usize,
    pub sev_counts: [usize; 4],
    pub sites_affected: usize,
    pub unfixable: usize,
}

fn sev_rank(s: &str) -> usize {
    match s {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        _ => 3,
    }
}

/// EPSS below this is noise; above it, worth saying out loud.
pub const EPSS_FLOOR: f64 = 0.01;

impl CveState {
    pub fn tier_label(&self, idx: usize) -> String {
        self.tiers.get(idx).cloned().unwrap_or_else(|| "all".to_string())
    }

    /// One past the last tier is "all tiers".
    pub fn tier_count(&self) -> usize {
        self.tiers.len() + 1
    }

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

    pub fn sev_label(idx: usize) -> &'static str {
        match idx {
            0 => "critical+high",
            1 => "+medium",
            _ => "everything",
        }
    }

    /// Re-derive everything the panel shows for one filter selection. A couple
    /// of hundred advisories and ~1200 alerts — cheap enough per frame.
    pub fn filtered(&self, f: Filters, keep: usize, keep_sites: usize) -> Filtered {
        let tier = (f.tier < self.tiers.len()).then(|| self.tiers[f.tier].clone());
        let eco = (f.eco > 0).then(|| self.eco_label(f.eco));
        let keep_alert = |a: &AlertRef| {
            tier.as_deref().map(|t| t == a.tier).unwrap_or(true)
                && eco.as_deref().map(|e| e == a.ecosystem).unwrap_or(true)
        };
        let sev_floor = match f.sev {
            0 => 1, // critical + high
            1 => 2, // …and medium
            _ => 3,
        };

        let mut rows: Vec<Row> = Vec::new();
        let mut alerts = 0usize;
        let mut sev_counts = [0usize; 4];
        let mut unfixable = 0usize;
        let mut tally: HashMap<&str, SiteTally> = HashMap::new();

        for adv in &self.advisories {
            if sev_rank(&adv.severity) > sev_floor {
                continue;
            }
            if f.fixable && !adv.fixable {
                continue;
            }
            let hits: Vec<&AlertRef> = adv.alerts.iter().filter(|a| keep_alert(a)).collect();
            if hits.is_empty() {
                continue;
            }
            alerts += hits.len();
            sev_counts[sev_rank(&adv.severity)] += 1;
            if !adv.fixable {
                unfixable += 1;
            }

            // a site reached through two manifests is still one site
            let mut sites: Vec<String> = hits.iter().map(|a| a.repo.clone()).collect();
            sites.sort();
            sites.dedup();
            let mut tiers: Vec<String> = hits.iter().map(|a| a.tier.clone()).collect();
            tiers.sort_by_key(|t| TIER_ORDER.iter().position(|x| x == t).unwrap_or(9));
            tiers.dedup();

            for a in &hits {
                let e = tally.entry(a.repo.as_str()).or_insert_with(|| SiteTally {
                    repo: a.repo.clone(),
                    critical: 0,
                    high: 0,
                    rest: 0,
                    sla: false,
                });
                match sev_rank(&adv.severity) {
                    0 => e.critical += 1,
                    1 => e.high += 1,
                    _ => e.rest += 1,
                }
                e.sla |= a.sla;
            }

            let first = hits[0];
            rows.push(Row {
                id: adv.id.clone(),
                summary: adv.summary.clone(),
                severity: adv.severity.clone(),
                cvss: adv.cvss,
                epss: adv.epss,
                published_ms: adv.published_ms,
                fixable: adv.fixable,
                package: first.package.clone(),
                ecosystem: first.ecosystem.clone(),
                sites,
                tiers,
                scope: first.scope.clone(),
                first_patched: hits
                    .iter()
                    .map(|a| a.first_patched.as_str())
                    .find(|v| !v.is_empty())
                    .unwrap_or_default()
                    .to_string(),
            });
        }

        let advisories = rows.len();
        let sites_affected = tally.len();

        // severity is how anyone triages, so it leads; EPSS breaks ties ahead
        // of blast radius because a likely exploit on one site beats an
        // improbable one on three
        let mut worst = rows.clone();
        worst.sort_by(|a, b| {
            sev_rank(&a.severity)
                .cmp(&sev_rank(&b.severity))
                .then(b.epss.unwrap_or(0.0).total_cmp(&a.epss.unwrap_or(0.0)))
                .then(b.sites.len().cmp(&a.sites.len()))
                .then(b.cvss.total_cmp(&a.cvss))
                .then(a.published_ms.cmp(&b.published_ms))
        });

        let critical: Vec<Row> =
            worst.iter().filter(|r| r.severity == "critical").cloned().collect();
        // the criticals have their own expanded block above; no point printing
        // them twice
        worst.retain(|r| r.severity != "critical");
        worst.truncate(keep);

        let mut exploitable: Vec<Row> = rows
            .iter()
            .filter(|r| r.epss.unwrap_or(0.0) >= EPSS_FLOOR && r.severity != "critical")
            .cloned()
            .collect();
        exploitable.sort_by(|a, b| {
            b.epss.unwrap_or(0.0)
                .total_cmp(&a.epss.unwrap_or(0.0))
                .then(sev_rank(&a.severity).cmp(&sev_rank(&b.severity)))
        });
        exploitable.truncate(KEEP_EPSS);

        let mut by_site: Vec<SiteTally> = tally.into_values().collect();
        by_site.sort_by(|a, b| {
            b.critical
                .cmp(&a.critical)
                .then(b.high.cmp(&a.high))
                .then(b.rest.cmp(&a.rest))
                .then(a.repo.cmp(&b.repo))
        });
        by_site.truncate(keep_sites);

        Filtered {
            critical,
            worst,
            exploitable,
            by_site,
            advisories,
            alerts,
            sev_counts,
            sites_affected,
            unfixable,
        }
    }
}

/// How many rows each section keeps.
pub const KEEP_CVES: usize = 30;
pub const KEEP_SITES: usize = 15;
pub const KEEP_EPSS: usize = 6;
const KEEP_DEPLOYS: usize = 12;
/// Concurrent `gh` subprocesses for the deploy heads. Only the deploy scan
/// still talks to GitHub at all.
const FANOUT: usize = 8;

fn parse_flex_ms(t: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(t)
        .map(|d| d.timestamp_millis())
        // the registry stamps "YYYY-MM-DD HH:MM:SS" in UTC, without a zone
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc3339(&format!("{}Z", t.replace(' ', "T")))
                .map(|d| d.timestamp_millis())
        })
        .unwrap_or(0)
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

/// The CLI's own wording, made short and actionable.
///
/// An expired token is the ordinary case rather than an exotic one: the `sites`
/// binary self-updates silently, and a server deploy can outdate a session that
/// was working ten minutes earlier. That reads as a thing to go and do, not as
/// a failure of the scan, so it gets its own shape.
pub fn sites_err(raw: &str) -> String {
    let e = raw.trim();
    if e.contains("Authentication failed") || e.contains("sites login") || e.contains("401") {
        "sites: not authenticated — run `sites login`".to_string()
    } else {
        format!("sites: {}", truncate_chars(e.trim_start_matches("Error: "), 110))
    }
}

/// Whether a message is the one the user can fix in one command.
pub fn is_auth_err(e: &str) -> bool {
    e.contains("not authenticated")
}

fn sites_json(args: &[&str]) -> Result<Value, String> {
    let out = Command::new("sites")
        .args(args)
        .output()
        .map_err(|_| "sites: CLI not installed".to_string())?;
    if !out.status.success() {
        return Err(sites_err(&String::from_utf8_lossy(&out.stderr)));
    }
    // an unauthenticated CLI has been seen to answer on stdout as well, and a
    // parse error would bury the one message worth reading
    let body = String::from_utf8_lossy(&out.stdout);
    if body.trim_start().starts_with("Error:") {
        return Err(sites_err(&body));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("sites: bad json ({e})"))
}

/// One read of the registry's advisory view. `--tooling` is a strict superset
/// of the default server-only answer — every server alert is present, plus the
/// build and deploy ones — so the tier filter can run locally off one call
/// instead of a subprocess per keystroke.
///
/// Risk-accepted alerts stay hidden: the registry has already had that
/// argument, and re-litigating it in a dashboard would be rude.
pub fn fetch() -> CveState {
    let mut st = CveState::default();
    let v = match sites_json(&["cves", "--tooling", "--json"]) {
        Ok(v) => v,
        Err(e) => {
            st.error = Some(e);
            st.fetched_at_ms = now_ms();
            return st;
        }
    };

    st.refreshed_ms = parse_flex_ms(&s(&v, "refreshed_at"));
    if v.get("refresh_ok").and_then(|x| x.as_bool()) == Some(false) {
        let e = s(&v, "refresh_error");
        st.refresh_error = Some(if e.is_empty() {
            "registry could not reach github".to_string()
        } else {
            truncate_chars(&e, 110)
        });
    }
    // the registry has never pulled at all — a real state, and not one to
    // report as a healthy zero
    if v.get("configured").and_then(|x| x.as_bool()) == Some(false) {
        st.error = Some("sites: dependabot not configured for this org".to_string());
    }

    let empty = vec![];
    let groups = ["critical", "other"];
    for g in groups {
        for a in v.get(g).and_then(|x| x.as_array()).unwrap_or(&empty) {
            let ghsa = s(a, "ghsa_id");
            let cve = s(a, "cve_id");
            let id = if cve.is_empty() { ghsa } else { cve };
            if id.is_empty() {
                continue;
            }
            let alerts: Vec<AlertRef> = a
                .get("alerts")
                .and_then(|x| x.as_array())
                .unwrap_or(&empty)
                .iter()
                .filter_map(|al| {
                    let repo = s(al, "repo_name");
                    if repo.is_empty() {
                        return None;
                    }
                    let reason = s(al, "scope_reason");
                    let detail = s(al, "scope_detail");
                    Some(AlertRef {
                        repo,
                        tier: s(al, "tier"),
                        ecosystem: s(al, "ecosystem"),
                        package: s(al, "package_name"),
                        scope: match (reason.as_str(), detail.as_str()) {
                            ("", _) => String::new(),
                            (r, "") => r.to_string(),
                            (r, d) => format!("{r} {d}"),
                        },
                        sla: al
                            .get("under_proactive_sla")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                        first_patched: s(al, "first_patched"),
                    })
                })
                .collect();
            if alerts.is_empty() {
                continue;
            }
            st.advisories.push(Advisory {
                id,
                summary: s(a, "summary"),
                severity: s(a, "severity"),
                cvss: a.get("cvss_score").and_then(|x| x.as_f64()).unwrap_or(0.0),
                epss: a.get("epss").and_then(|x| x.as_f64()),
                published_ms: parse_flex_ms(&s(a, "published_at")),
                fixable: a.get("fixable").and_then(|x| x.as_bool()).unwrap_or(false),
                alerts,
            });
        }
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for a in &st.advisories {
        for al in &a.alerts {
            seen.insert(al.tier.as_str());
        }
    }
    st.tiers = TIER_ORDER
        .iter()
        .filter(|t| seen.contains(**t))
        .map(|t| t.to_string())
        .collect();

    let mut eco_alerts: HashMap<&str, usize> = HashMap::new();
    for a in &st.advisories {
        for al in &a.alerts {
            *eco_alerts.entry(al.ecosystem.as_str()).or_insert(0) += 1;
        }
    }
    let mut ecos: Vec<(&str, usize)> = eco_alerts.into_iter().collect();
    ecos.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    st.ecosystems = ecos.into_iter().map(|(e, _)| e.to_string()).collect();

    // the denominator for "N of M sites affected"
    st.sites_total = sites_json(&["list", "--json"])
        .ok()
        .and_then(|v| v.as_array().map(|a| a.len()))
        .unwrap_or(0);

    st.fetched_at_ms = now_ms();
    st
}

const JQ_BRANCH: &str = "[.commit.commit.committer.date, \
    (.commit.commit.author.name // \"\"), \
    (.commit.commit.message | split(\"\\n\")[0])] | @tsv";

fn gh_lines(args: &[&str]) -> Result<Vec<String>, String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("gh not runnable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect())
}

/// `git@github.com:haunt-digital/foo.git` → `haunt-digital`. One org today,
/// but the owner is data.
fn owner_of(repo_url: &str) -> Option<String> {
    let after_host = repo_url
        .rsplit_once("github.com")
        .map(|(_, rest)| rest.trim_start_matches([':', '/']))?;
    after_host.split('/').next().map(str::to_string)
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

/// What has actually shipped. ~25 GitHub calls, so it runs on its own thread
/// and its own cadence — the advisories no longer wait on it.
pub fn fetch_deploys() -> DeployState {
    let mut st = DeployState::default();
    let v = match sites_json(&["list", "--json"]) {
        Ok(v) => v,
        Err(e) => {
            st.error = Some(e);
            st.fetched_at_ms = now_ms();
            return st;
        }
    };
    let empty = vec![];
    let repos: Vec<(String, String)> = v
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
        .collect();

    for chunk in repos.chunks(FANOUT) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|(owner, repo)| {
                let (owner, repo) = (owner.clone(), repo.clone());
                std::thread::spawn(move || repo_deploy(&owner, &repo))
            })
            .collect();
        for h in handles {
            if let Ok(Some(d)) = h.join() {
                st.deploys.push(d);
            }
        }
    }
    st.deploys.sort_by_key(|d| -d.when_ms);
    st.deploys.truncate(KEEP_DEPLOYS);
    st.fetched_at_ms = now_ms();
    st
}
