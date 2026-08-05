use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::now_ms;
use crate::gh::{FetchResult, GhPr, GhRun};
use crate::haunt::{HauntRun, HauntState};

/// Shared on-disk cache of the g view's fetched data, so multiple
/// claude-watch instances don't all hit gh/roadmap/sites at once.

#[derive(Deserialize)]
pub struct GCache {
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

fn file() -> PathBuf {
    dir().join("gview.json")
}

fn lock() -> PathBuf {
    dir().join("gview.lock")
}

pub fn load() -> Option<GCache> {
    let txt = fs::read_to_string(file()).ok()?;
    serde_json::from_str(&txt).ok()
}

/// Atomic write (temp + rename) so readers never see a torn file.
pub fn store(g: &FetchResult, h: &HauntState) {
    let _ = fs::create_dir_all(dir());
    let c = GCacheRef {
        fetched_at_ms: now_ms(),
        runs: &g.runs,
        prs: &g.prs,
        gh_error: &g.error,
        haunt_runs: &h.runs,
        roadmap_err: &h.roadmap_err,
        sites_err: &h.sites_err,
    };
    if let Ok(json) = serde_json::to_string(&c) {
        let tmp = dir().join("gview.json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, file());
        }
    }
}

/// One fetcher at a time across all instances; stale locks (crashed
/// writers) are stolen after 3 minutes.
pub fn try_lock() -> bool {
    let _ = fs::create_dir_all(dir());
    let l = lock();
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

pub fn unlock() {
    let _ = fs::remove_file(lock());
}
