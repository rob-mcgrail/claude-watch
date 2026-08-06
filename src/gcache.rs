use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::now_ms;
use crate::gh::{FetchResult, GhPr, GhRun};
use crate::haunt::{HauntRun, HauntState};

/// Shared on-disk cache of the g view's fetched data, so multiple
/// claude-watch instances don't all hit gh/roadmap/sites at once. One file per
/// mode: the live feed and the 10-day digest hold different data and must never
/// overwrite each other.

#[derive(Deserialize)]
pub struct GCache {
    /// Which mode wrote this. Absent in files written before modes existed, and
    /// those hold windows we can no longer identify — so `load` rejects them.
    #[serde(default)]
    pub mode: String,
    pub fetched_at_ms: i64,
    pub runs: Vec<GhRun>,
    pub prs: Vec<GhPr>,
    pub gh_error: Option<String>,
    pub haunt_runs: Vec<HauntRun>,
    pub roadmap_err: Option<String>,
    pub sites_err: Option<String>,
}

#[derive(Serialize)]
struct GCacheRef<'a> {
    mode: &'a str,
    fetched_at_ms: i64,
    runs: &'a [GhRun],
    prs: &'a [GhPr],
    gh_error: &'a Option<String>,
    haunt_runs: &'a [HauntRun],
    roadmap_err: &'a Option<String>,
    sites_err: &'a Option<String>,
}

fn dir() -> PathBuf {
    crate::discover::home().join(".cache").join("claude-watch")
}

fn file(mode: &str) -> PathBuf {
    dir().join(format!("gview-{mode}.json"))
}

fn lock(mode: &str) -> PathBuf {
    dir().join(format!("gview-{mode}.lock"))
}

/// A cache written by a different mode is worse than no cache — it would render
/// one mode's windows under the other's headers — so mismatches are dropped.
pub fn load(mode: &str) -> Option<GCache> {
    let txt = fs::read_to_string(file(mode)).ok()?;
    let c: GCache = serde_json::from_str(&txt).ok()?;
    (c.mode == mode).then_some(c)
}

/// Atomic write (temp + rename) so readers never see a torn file.
pub fn store(mode: &str, g: &FetchResult, h: &HauntState) {
    let _ = fs::create_dir_all(dir());
    let c = GCacheRef {
        mode,
        fetched_at_ms: now_ms(),
        runs: &g.runs,
        prs: &g.prs,
        gh_error: &g.error,
        haunt_runs: &h.runs,
        roadmap_err: &h.roadmap_err,
        sites_err: &h.sites_err,
    };
    if let Ok(json) = serde_json::to_string(&c) {
        let tmp = dir().join(format!("gview-{mode}.json.tmp"));
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, file(mode));
        }
    }
}

/// The security scan caches whole, since it is already an aggregate.
pub fn load_cve() -> Option<crate::cve::CveState> {
    let txt = fs::read_to_string(dir().join("cve.json")).ok()?;
    serde_json::from_str(&txt).ok()
}

pub fn store_cve(st: &crate::cve::CveState) {
    let _ = fs::create_dir_all(dir());
    if let Ok(json) = serde_json::to_string(st) {
        let tmp = dir().join("cve.json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, dir().join("cve.json"));
        }
    }
}

/// The sites registry caches whole — it is one cheap CLI call, cached only so
/// the panel opens instantly.
pub fn load_sites() -> Option<crate::sitelist::SitesState> {
    let txt = fs::read_to_string(dir().join("sites.json")).ok()?;
    serde_json::from_str(&txt).ok()
}

pub fn store_sites(st: &crate::sitelist::SitesState) {
    let _ = fs::create_dir_all(dir());
    if let Ok(json) = serde_json::to_string(st) {
        let tmp = dir().join("sites.json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, dir().join("sites.json"));
        }
    }
}

/// One fetcher at a time across all instances; stale locks (crashed
/// writers) are stolen after 3 minutes.
pub fn try_lock(mode: &str) -> bool {
    let _ = fs::create_dir_all(dir());
    let l = lock(mode);
    if let Ok(md) = fs::metadata(&l) {
        let age = md
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        if age > 180 {
            let _ = fs::remove_file(&l);
        }
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&l)
        .is_ok()
}

pub fn unlock(mode: &str) {
    let _ = fs::remove_file(lock(mode));
}
