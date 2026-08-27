use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{Read as _, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use chrono::TimeZone;
use ratatui::layout::Rect;
use serde_json::Value;

use crate::discover::{self, SessionRef};
use crate::price;

// ---------- small helpers ----------

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn fmt_clock(ms: i64) -> String {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".into())
}

pub fn fmt_dur(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else if s < 86400 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{}h", s / 86400, (s % 86400) / 3600)
    }
}

pub fn fmt_tok(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, max)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

/// Keep the tail of a path — the interesting end — when it doesn't fit.
pub fn tail_truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let skip = n - max.saturating_sub(1);
    let t: String = s.chars().skip(skip).collect();
    format!("…{t}")
}

fn result_text(block: &Value) -> Option<String> {
    match block.get("content") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(a)) => a.iter().find_map(|b| s(b, "text").map(str::to_string)),
        _ => None,
    }
}

fn parse_ts(v: &Value) -> Option<i64> {
    v.get("timestamp")?
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

// ---------- file tailing ----------

struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self { path, offset: 0, partial: Vec::new() }
    }

    fn poll(&mut self) -> Vec<String> {
        let Ok(md) = fs::metadata(&self.path) else { return vec![] };
        let len = md.len();
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset {
            return vec![];
        }
        let Ok(mut f) = File::open(&self.path) else { return vec![] };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return vec![];
        }
        let mut data = Vec::new();
        if f.take(len - self.offset).read_to_end(&mut data).is_err() {
            return vec![];
        }
        self.offset += data.len() as u64;
        self.partial.extend_from_slice(&data);
        let mut lines = Vec::new();
        while let Some(pos) = self.partial.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.partial.drain(..=pos).collect();
            if let Ok(line) = String::from_utf8(raw) {
                let t = line.trim();
                if !t.is_empty() {
                    lines.push(t.to_string());
                }
            }
        }
        lines
    }
}

// ---------- domain types ----------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PaneId {
    Feed,
    Reads,
    Writes,
    Hooks,
    Skills,
    Thinking,
    Memory,
    Context,
    ToolIO,
    Overview,
    GitHub,
    Cve,
    Sites,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Status {
    Working,
    Waiting,
    Blocked,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LastKind {
    Prompt,
    Reply,
    Tool,
    Other,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FeedKind {
    Tool,
    Mcp,
    Prompt,
    Reply,
    Info,
    Warn,
    Agent,
    Skill,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToolStatus {
    None,
    Pending,
    Ok,
    Err,
}

pub struct FeedItem {
    pub ts: i64,
    pub agent: Option<String>,
    pub text: String,
    pub kind: FeedKind,
    pub status: ToolStatus,
}

pub struct ReadEntry {
    pub ts: i64,
    pub agent: Option<String>,
    pub path: String,
    pub count: u32,
    /// how the content entered context: 'R' Read tool, '$' shell command,
    /// '@' user-attached file, '±' re-read after external edit
    pub src: char,
    pub err: bool,
}

pub struct WriteEntry {
    pub ts: i64,
    pub agent: Option<String>,
    pub path: String,
    pub kind: char, // 'W' write, 'E' edit
    pub adds: Option<u64>,
    pub dels: Option<u64>,
    pub err: bool,
}

pub struct Think {
    pub ts: i64,
    pub agent: Option<String>,
    pub text: String,
}

#[derive(Default)]
pub struct HookStat {
    pub count: u64,
    pub total_ms: u64,
    pub acted: u64,
    pub last_ts: i64,
}

pub struct HookAction {
    pub ts: i64,
    pub label: String,
    pub detail: String,
    pub sev: u8, // 2 = red, 1 = magenta
}

pub struct SkillUse {
    pub name: String,
    pub count: u32,
    pub last_ts: i64,
}

pub struct AgentInfo {
    pub idx: usize,
    pub model: String,
    pub desc: String,
}

pub struct ConfigHook {
    pub event: String,
    pub name: String,
    pub status_message: Option<String>,
    pub command: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ThinkFilter {
    All,
    Main,
    Agent(usize),
}

/// One display line of the thinking pane (post-wrap).
pub struct TLine {
    pub text: String,
    pub agent_idx: Option<usize>,
    pub header: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CtxKind {
    User,
    Assistant,
    Tool,
    Summary,
    Boundary,
}

/// One message of the main chain's context window.
pub struct CtxMsg {
    pub ts: i64,
    pub kind: CtxKind,
    pub text: String,
}

/// Full, untruncated input/output of one tool call (non file-I/O tools).
pub struct ToolIO {
    pub ts: i64,
    pub agent: Option<String>,
    pub name: String,
    pub input: String,
    pub output: Option<String>,
    pub err: bool,
    pub dur_ms: i64,
}

/// Rendered-lines cache for the full-screen panes, keyed on (width, revision).
#[derive(Default)]
pub struct PaneCache {
    pub key: (usize, u64, usize),
    pub lines: Vec<ratatui::text::Line<'static>>,
    /// lowercased plain text of each line, for search
    pub texts: Vec<String>,
}

pub struct SearchState {
    pub input: Option<String>,
    pub query: Option<String>,
    /// which pane the query applies to
    pub target: PaneId,
    pub matches: Vec<usize>,
    pub cur: usize,
    pub jump_pending: bool,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            input: None,
            query: None,
            target: PaneId::Thinking,
            matches: Vec::new(),
            cur: 0,
            jump_pending: false,
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub c5m: u64,
    pub c1h: u64,
}

#[derive(Default)]
struct Pending {
    feed_idx: Option<usize>,
    write_idx: Option<usize>,
    read_idx: Option<usize>,
    tio_idx: Option<usize>,
    start_ts: i64,
}

// ---------- app ----------

pub struct App {
    pub cwd: PathBuf,
    pub nzd_rate: f64,
    pub ctx_window: u64,

    pub sessions: Vec<SessionRef>,
    pub sel: usize,
    pub auto_follow: bool,
    current_id: Option<String>,
    subagents_dir: Option<PathBuf>,
    tails: Vec<Tail>,
    tailed_agents: Vec<PathBuf>,
    last_discover: Instant,
    last_sub_scan: Instant,
    pub last_activity: Instant,

    pub agents: HashMap<String, AgentInfo>,
    pub agent_order: Vec<String>,

    pub feed: Vec<FeedItem>,
    pub reads: Vec<ReadEntry>,
    pub writes: Vec<WriteEntry>,
    pub thinking: Vec<Think>,
    pub hook_stats: BTreeMap<String, HookStat>,
    pub hook_actions: Vec<HookAction>,
    pub skills: Vec<SkillUse>,
    pub hooks_config: Vec<ConfigHook>,

    pending: HashMap<String, Pending>,
    usage_by_msg: HashMap<String, (String, Usage, i64)>,
    pub ctx_tokens: u64,
    pub model: String,
    pub title: String,
    pub branch: String,

    turn_open: bool,
    last_main_kind: LastKind,
    pub turn_start_ts: i64,
    pub last_turn_end_ts: i64,
    last_ts: i64,

    // ui state
    pub layout: u8,
    pub focus: PaneId,
    pub scroll: HashMap<PaneId, usize>,
    pub think_filter_pos: usize,
    pub search: SearchState,
    pub pane_rects: Vec<(PaneId, Rect)>,
    pub pending_jump: Option<usize>,
    pub think_lines: Vec<TLine>,
    pub think_cache_key: (usize, usize, usize, usize),

    // full-screen views
    pub ctx: Vec<CtxMsg>,
    pub ctx_rev: u64,
    pub tool_ios: Vec<ToolIO>,
    pub tio_rev: u64,
    pub memory_files: Vec<(String, String)>,
    pub mem_rev: u64,
    mem_stamp: (u64, u64),
    last_mem_check: Instant,
    pub ctx_cache: PaneCache,
    pub tio_cache: PaneCache,
    pub mem_cache: PaneCache,

    // machine-wide session switcher (view 0)
    pub overview: Vec<crate::overview::OverviewSession>,
    pub overview_sel: usize,
    pub overview_top: usize,
    pub overview_ranges: Vec<(usize, usize)>,
    last_overview_scan: Instant,

    // github + haunt activity (view g / space)
    pub gh_mode: GhMode,
    // dependabot across the managed sites (view s)
    pub cve: crate::cve::CveState,
    cve_tx: Sender<crate::cve::CveState>,
    cve_rx: Receiver<crate::cve::CveState>,
    cve_attempt: Option<Instant>,
    pub cve_filters: crate::cve::Filters,
    /// What has shipped — GitHub's answer, on its own slower clock so the
    /// advisories never wait for it.
    pub deploys: crate::cve::DeployState,
    dep_tx: Sender<crate::cve::DeployState>,
    dep_rx: Receiver<crate::cve::DeployState>,
    dep_attempt: Option<Instant>,
    // sites registry (view m)
    pub sites_reg: crate::sitelist::SitesState,
    sites_tx: Sender<crate::sitelist::SitesState>,
    sites_rx: Receiver<crate::sitelist::SitesState>,
    sites_attempt: Option<Instant>,
    gh_views: [GhView; 2],
    gh_tx: Sender<GhMsg>,
    gh_rx: Receiver<GhMsg>,
    last_gcache_check: Instant,
}

/// The g view has two modes over the same sources: `g` for what is happening
/// right now, space for a digest of the last 10 days. Each keeps its own state
/// and its own cache file, so switching between them never refetches.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum GhMode {
    Live,
    Digest,
}

impl GhMode {
    pub const ALL: [GhMode; 2] = [GhMode::Live, GhMode::Digest];

    fn idx(self) -> usize {
        match self {
            GhMode::Live => 0,
            GhMode::Digest => 1,
        }
    }

    /// Cache-file tag, and the label in the pane title.
    pub fn tag(self) -> &'static str {
        match self {
            GhMode::Live => "live",
            GhMode::Digest => "digest",
        }
    }

    pub fn gh_params(self) -> crate::gh::Params {
        match self {
            GhMode::Live => crate::gh::Params::LIVE,
            GhMode::Digest => crate::gh::Params::DIGEST,
        }
    }

    pub fn haunt_params(self) -> crate::haunt::Params {
        match self {
            GhMode::Live => crate::haunt::Params::LIVE,
            GhMode::Digest => crate::haunt::Params::DIGEST,
        }
    }

    /// The live feed goes stale in a couple of minutes; a 10-day digest does not.
    fn ttl_secs(self) -> i64 {
        match self {
            GhMode::Live => 120,
            GhMode::Digest => 600,
        }
    }
}

#[derive(Default)]
pub struct GhView {
    pub gh: crate::gh::GhState,
    pub haunt: crate::haunt::HauntState,
    /// When a fetch was last *started*, successful or not. A fetch that loses
    /// the cross-instance lock leaves the data stale, so without this the
    /// staleness check in `tick` would respawn it every frame.
    last_attempt: Option<Instant>,
}

struct GhMsg {
    mode: GhMode,
    /// None when another instance held the fetch lock — nothing fetched, and
    /// the pane's spinner needs clearing either way.
    data: Option<(crate::gh::FetchResult, crate::haunt::HauntState)>,
}

/// Fetch one mode on the calling (background) thread and hand the result back.
/// One fetcher per mode across all instances; instances that lose the lock pick
/// the winner's result up from the cache via the 3s poll in `tick`.
fn fetch_mode(mode: GhMode, tx: &Sender<GhMsg>) {
    if !crate::gcache::try_lock(mode.tag()) {
        let _ = tx.send(GhMsg { mode, data: None });
        return;
    }
    let gh_params = mode.gh_params();
    let gh_handle = std::thread::spawn(move || crate::gh::fetch(gh_params));
    let h = crate::haunt::fetch(mode.haunt_params());
    let g = gh_handle.join().unwrap_or_else(|_| crate::gh::FetchResult {
        runs: vec![],
        prs: vec![],
        error: Some("gh fetch panicked".into()),
    });
    crate::gcache::store(mode.tag(), &g, &h);
    crate::gcache::unlock(mode.tag());
    let _ = tx.send(GhMsg { mode, data: Some((g, h)) });
}

impl App {
    pub fn new(cwd: PathBuf, nzd_rate: f64, ctx_window: u64) -> Self {
        let hooks_config = load_hook_config(&cwd);
        let (gh_tx, gh_rx) = std::sync::mpsc::channel();
        let (cve_tx, cve_rx) = std::sync::mpsc::channel();
        let (dep_tx, dep_rx) = std::sync::mpsc::channel();
        let (sites_tx, sites_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            cwd,
            nzd_rate,
            ctx_window,
            sessions: Vec::new(),
            sel: 0,
            auto_follow: true,
            current_id: None,
            subagents_dir: None,
            tails: Vec::new(),
            tailed_agents: Vec::new(),
            last_discover: Instant::now(),
            last_sub_scan: Instant::now(),
            last_activity: Instant::now(),
            agents: HashMap::new(),
            agent_order: Vec::new(),
            feed: Vec::new(),
            reads: Vec::new(),
            writes: Vec::new(),
            thinking: Vec::new(),
            hook_stats: BTreeMap::new(),
            hook_actions: Vec::new(),
            skills: Vec::new(),
            hooks_config,
            pending: HashMap::new(),
            usage_by_msg: HashMap::new(),
            ctx_tokens: 0,
            model: String::new(),
            title: String::new(),
            branch: String::new(),
            turn_open: false,
            last_main_kind: LastKind::Other,
            turn_start_ts: 0,
            last_turn_end_ts: 0,
            last_ts: 0,
            layout: 1,
            focus: PaneId::Feed,
            scroll: HashMap::new(),
            think_filter_pos: 0,
            search: SearchState::default(),
            pane_rects: Vec::new(),
            pending_jump: None,
            think_lines: Vec::new(),
            think_cache_key: (0, 0, 0, 0),
            ctx: Vec::new(),
            ctx_rev: 0,
            tool_ios: Vec::new(),
            tio_rev: 0,
            memory_files: Vec::new(),
            mem_rev: 0,
            mem_stamp: (0, 0),
            last_mem_check: Instant::now(),
            ctx_cache: PaneCache::default(),
            tio_cache: PaneCache::default(),
            mem_cache: PaneCache::default(),
            overview: Vec::new(),
            overview_sel: 0,
            overview_top: 0,
            overview_ranges: Vec::new(),
            last_overview_scan: Instant::now(),
            gh_mode: GhMode::Live,
            cve: Default::default(),
            cve_tx,
            cve_rx,
            cve_attempt: None,
            cve_filters: Default::default(),
            deploys: Default::default(),
            dep_tx,
            dep_rx,
            dep_attempt: None,
            sites_reg: Default::default(),
            sites_tx,
            sites_rx,
            sites_attempt: None,
            gh_views: Default::default(),
            gh_tx,
            gh_rx,
            last_gcache_check: Instant::now(),
        };
        app.sessions = discover::discover_sessions(&app.cwd);
        if !app.sessions.is_empty() {
            app.open_session(0);
        }
        app
    }

    pub fn select_session_by_prefix(&mut self, prefix: &str) {
        if let Some(i) = self.sessions.iter().position(|s| s.id.starts_with(prefix)) {
            self.open_session(i);
        }
    }

    pub fn open_session(&mut self, idx: usize) {
        if idx >= self.sessions.len() {
            return;
        }
        self.sel = idx;
        self.auto_follow = idx == 0;
        let sref = self.sessions[idx].clone();
        self.current_id = Some(sref.id.clone());

        self.agents.clear();
        self.agent_order.clear();
        self.feed.clear();
        self.reads.clear();
        self.writes.clear();
        self.thinking.clear();
        self.hook_stats.clear();
        self.hook_actions.clear();
        self.skills.clear();
        self.pending.clear();
        self.usage_by_msg.clear();
        self.ctx_tokens = 0;
        self.model.clear();
        self.title.clear();
        self.branch.clear();
        self.turn_open = false;
        self.last_main_kind = LastKind::Other;
        self.turn_start_ts = 0;
        self.last_turn_end_ts = 0;
        self.last_ts = 0;
        self.scroll.clear();
        self.think_filter_pos = 0;
        self.search = SearchState::default();
        self.pending_jump = None;
        self.think_lines.clear();
        self.think_cache_key = (0, 0, 0, 0);
        self.ctx.clear();
        self.ctx_rev += 1;
        self.tool_ios.clear();
        self.tio_rev += 1;
        self.memory_files.clear();
        self.mem_rev += 1;
        self.mem_stamp = (0, 0);

        self.tails = vec![Tail::new(sref.file.clone())];
        self.tailed_agents.clear();
        self.subagents_dir = Some(sref.project_dir.join(&sref.id).join("subagents"));
        self.scan_subagents();
        self.drain_tails();
        self.last_activity = Instant::now();
    }

    fn scan_subagents(&mut self) {
        let Some(dir) = self.subagents_dir.clone() else { return };
        let Ok(rd) = fs::read_dir(&dir) else { return };
        let mut new_files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                    && !self.tailed_agents.contains(p)
            })
            .collect();
        new_files.sort();
        for p in new_files {
            self.tailed_agents.push(p.clone());
            self.tails.push(Tail::new(p));
        }
    }

    fn drain_tails(&mut self) {
        let mut batch: Vec<(i64, Value)> = Vec::new();
        for t in &mut self.tails {
            for line in t.poll() {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let ts = parse_ts(&v).unwrap_or(self.last_ts);
                    batch.push((ts, v));
                }
            }
        }
        if batch.is_empty() {
            return;
        }
        self.last_activity = Instant::now();
        batch.sort_by_key(|(ts, _)| *ts);
        for (ts, v) in batch {
            self.last_ts = self.last_ts.max(ts);
            self.apply(ts, &v);
        }
    }

    pub fn tick(&mut self) {
        if self.last_discover.elapsed() > Duration::from_secs(2) {
            self.last_discover = Instant::now();
            let found = discover::discover_sessions(&self.cwd);
            if !found.is_empty() {
                self.sessions = found;
                match &self.current_id {
                    None => self.open_session(0),
                    Some(id) => {
                        if self.auto_follow && self.sessions[0].id != *id {
                            self.open_session(0);
                        } else if let Some(i) =
                            self.sessions.iter().position(|x| x.id == *id)
                        {
                            self.sel = i;
                        } else {
                            self.open_session(0);
                        }
                    }
                }
            }
        }
        if self.current_id.is_some() && self.last_sub_scan.elapsed() > Duration::from_secs(1) {
            self.last_sub_scan = Instant::now();
            self.scan_subagents();
        }
        if self.layout == 6 && self.last_mem_check.elapsed() > Duration::from_secs(2) {
            self.last_mem_check = Instant::now();
            self.load_memory();
        }
        if self.layout == 0 && self.last_overview_scan.elapsed() > Duration::from_secs(5) {
            self.last_overview_scan = Instant::now();
            self.refresh_overview();
        }
        if self.layout == 7 && self.last_gcache_check.elapsed() > Duration::from_secs(3) {
            self.last_gcache_check = Instant::now();
            self.load_gcache(self.gh_mode);
        }
        // the mode on screen comes first; when it is fresh, spend the idle time
        // keeping the other one warm so switching to it never shows a reload
        if self.layout == 7 {
            let active = self.gh_mode;
            if self.gh_stale(active) {
                self.gh_refresh(active);
            } else if let Some(m) = GhMode::ALL.into_iter().find(|&m| m != active && self.gh_stale(m)) {
                self.gh_refresh(m);
            }
        }
        while let Ok(msg) = self.gh_rx.try_recv() {
            self.apply_gh(msg);
        }
        while let Ok(st) = self.cve_rx.try_recv() {
            // an empty result means another instance held the read lock —
            // clear the spinner but keep whatever is already on screen
            if st.fetched_at_ms > 0 {
                self.cve = st;
                // content just grew from nothing; re-pin to the top so the
                // summary is not left scrolled off
                self.scroll.insert(PaneId::Cve, usize::MAX);
                self.clamp_cve_filters();
            } else {
                self.cve.fetching = false;
            }
        }
        while let Ok(st) = self.dep_rx.try_recv() {
            if st.fetched_at_ms > 0 {
                self.deploys = st;
            } else {
                self.deploys.fetching = false;
            }
        }
        if self.layout == 8 {
            if self.cve_stale() {
                self.cve_refresh();
            }
            if self.dep_stale() {
                self.dep_refresh();
            }
        }
        while let Ok(st) = self.sites_rx.try_recv() {
            let keep_err = st.error.clone();
            if !st.rows.is_empty() || keep_err.is_some() {
                self.sites_reg = st;
            } else {
                self.sites_reg.fetching = false;
            }
            self.scroll.insert(PaneId::Sites, usize::MAX);
        }
        if self.layout == 9 && self.sites_stale() {
            self.sites_refresh();
        }
        self.drain_tails();
    }

    pub fn next_session(&mut self, dir: i64) {
        let n = self.sessions.len();
        if n == 0 {
            return;
        }
        let cur = self.sel as i64;
        let next = (cur + dir).rem_euclid(n as i64) as usize;
        self.open_session(next);
    }

    // ---------- derived ----------

    pub fn status(&self) -> Status {
        if !self.turn_open {
            return Status::Waiting;
        }
        let idle = self.last_activity.elapsed();
        match self.last_main_kind {
            // mid-tool: quiet + unresolved tool = probably a permission prompt
            LastKind::Tool => {
                if !self.pending.is_empty() && idle > Duration::from_secs(10) {
                    Status::Blocked
                } else if idle > Duration::from_secs(60) {
                    Status::Blocked
                } else {
                    Status::Working
                }
            }
            // a reply then silence: the turn is over even if turn_duration
            // never arrived (it goes missing while background agents run)
            LastKind::Reply => {
                if idle > Duration::from_secs(20) {
                    Status::Waiting
                } else {
                    Status::Working
                }
            }
            // a prompt then silence: it was undone/edited away before running
            LastKind::Prompt => {
                if idle > Duration::from_secs(45) {
                    Status::Waiting
                } else {
                    Status::Working
                }
            }
            LastKind::Other => {
                if idle > Duration::from_secs(60) {
                    Status::Waiting
                } else {
                    Status::Working
                }
            }
        }
    }

    pub fn has_pending_tools(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn totals(&self) -> Usage {
        let mut t = Usage::default();
        for (_, u, _) in self.usage_by_msg.values() {
            t.input += u.input;
            t.output += u.output;
            t.cache_read += u.cache_read;
            t.c5m += u.c5m;
            t.c1h += u.c1h;
        }
        t
    }

    pub fn cost_usd(&self) -> f64 {
        self.usage_by_msg
            .values()
            .map(|(m, u, ts)| {
                let (pi, po) = price::prices_usd_per_mtok(m, *ts);
                (u.input as f64 * pi
                    + u.output as f64 * po
                    + u.cache_read as f64 * pi * 0.1
                    + u.c5m as f64 * pi * 1.25
                    + u.c1h as f64 * pi * 2.0)
                    / 1e6
            })
            .sum()
    }

    pub fn agent_tag(&self, key: &Option<String>) -> Option<(String, usize)> {
        let k = key.as_ref()?;
        let a = self.agents.get(k)?;
        let m = if a.model.is_empty() { "?" } else { price::model_short(&a.model) };
        Some((format!("[{}:{}]", m, a.idx), a.idx))
    }

    pub fn agent_by_idx(&self, idx: usize) -> Option<&AgentInfo> {
        self.agent_order
            .iter()
            .filter_map(|k| self.agents.get(k))
            .find(|a| a.idx == idx)
    }

    pub fn think_filters(&self) -> Vec<ThinkFilter> {
        let mut v = vec![ThinkFilter::All, ThinkFilter::Main];
        for i in 1..=self.agent_order.len() {
            v.push(ThinkFilter::Agent(i));
        }
        v
    }

    pub fn think_filter(&self) -> ThinkFilter {
        let f = self.think_filters();
        f[self.think_filter_pos % f.len()]
    }

    pub fn agent_passes_filter(&self, agent: &Option<String>) -> bool {
        match self.think_filter() {
            ThinkFilter::All => true,
            ThinkFilter::Main => agent.is_none(),
            ThinkFilter::Agent(i) => {
                self.agent_tag(agent).map(|(_, x)| x == i).unwrap_or(false)
            }
        }
    }

    pub fn cycle_think_filter(&mut self, dir: i64) {
        let n = self.think_filters().len() as i64;
        let cur = (self.think_filter_pos as i64) % n;
        self.think_filter_pos = (cur + dir).rem_euclid(n) as usize;
        self.scroll.insert(PaneId::Thinking, 0);
        self.scroll.insert(PaneId::Feed, 0);
        self.scroll.insert(PaneId::ToolIO, 0);
    }

    pub fn short_path(&self, p: &str) -> String {
        let cwd = self.cwd.to_string_lossy().to_string();
        if let Some(rest) = p.strip_prefix(&format!("{cwd}/")) {
            return rest.to_string();
        }
        let home = discover::home().to_string_lossy().to_string();
        if let Some(rest) = p.strip_prefix(&home) {
            return format!("~{rest}");
        }
        p.to_string()
    }

    // ---------- event application ----------

    fn ensure_agent(&mut self, id: &str) {
        if !self.agents.contains_key(id) {
            let idx = self.agent_order.len() + 1;
            self.agents.insert(
                id.to_string(),
                AgentInfo { idx, model: String::new(), desc: String::new() },
            );
            self.agent_order.push(id.to_string());
        }
    }

    fn push_feed(
        &mut self,
        ts: i64,
        agent: Option<String>,
        text: String,
        kind: FeedKind,
        status: ToolStatus,
    ) -> usize {
        if let Some(last) = self.feed.last() {
            let day = |ms: i64| {
                chrono::Local
                    .timestamp_millis_opt(ms)
                    .single()
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default()
            };
            if ts > last.ts && day(last.ts) != day(ts) {
                let label = chrono::Local
                    .timestamp_millis_opt(ts)
                    .single()
                    .map(|d| d.format("%a %e %b").to_string())
                    .unwrap_or_default();
                self.feed.push(FeedItem {
                    ts,
                    agent: None,
                    text: format!("── {label} ──"),
                    kind: FeedKind::Info,
                    status: ToolStatus::None,
                });
            }
        }
        self.feed.push(FeedItem { ts, agent, text, kind, status });
        self.feed.len() - 1
    }

    /// Context window estimate. Window sizes are not recorded in transcripts,
    /// so: per-model default (Fable sessions observably auto-compact at ~1M),
    /// bumped to the next real tier if observed context exceeds it.
    /// --context-window overrides the default.
    pub fn effective_window(&self) -> u64 {
        let model_default = if self.model.to_ascii_lowercase().contains("fable") {
            1_000_000
        } else {
            200_000
        };
        let base = self.ctx_window.max(if self.ctx_window == 200_000 {
            model_default
        } else {
            0
        });
        [base, 1_000_000, 2_000_000]
            .into_iter()
            .find(|t| self.ctx_tokens <= *t)
            .unwrap_or(2_000_000)
    }

    fn tally_skill(&mut self, name: &str, ts: i64) {
        if let Some(sk) = self.skills.iter_mut().find(|s| s.name == name) {
            sk.count += 1;
            sk.last_ts = ts;
        } else {
            self.skills.push(SkillUse { name: name.to_string(), count: 1, last_ts: ts });
        }
    }

    fn apply(&mut self, ts: i64, v: &Value) {
        let agent: Option<String> = s(v, "agentId").map(|a| a.to_string());
        if let Some(a) = &agent {
            self.ensure_agent(a);
        }
        let sidechain = v.get("isSidechain").and_then(|x| x.as_bool()).unwrap_or(false)
            || agent.is_some();
        if let Some(b) = s(v, "gitBranch") {
            if !b.is_empty() {
                self.branch = b.to_string();
            }
        }

        match s(v, "type").unwrap_or("") {
            "assistant" => self.apply_assistant(ts, v, &agent, sidechain),
            "user" => self.apply_user(ts, v, &agent, sidechain),
            "system" => self.apply_system(ts, v, sidechain),
            "attachment" => self.apply_attachment(ts, v),
            "queue-operation" => {
                if s(v, "operation") == Some("enqueue") {
                    let content = s(v, "content").unwrap_or("");
                    let text = format!("⧗ queued: {}", first_line(content, 100));
                    // queued input is user input — style it like a prompt
                    self.push_feed(ts, None, text, FeedKind::Prompt, ToolStatus::None);
                }
            }
            // "mode" / "permission-mode" entries are rewritten constantly by
            // Claude Code — pure noise in the feed, so they are not shown.
            "ai-title" => {
                if let Some(t) = s(v, "aiTitle") {
                    self.title = t.to_string();
                }
            }
            "pr-link" => {
                let url = s(v, "url").or_else(|| s(v, "prLink")).or_else(|| s(v, "link")).unwrap_or("");
                let text = format!("· PR linked {}", truncate_chars(url, 80));
                self.push_feed(ts, None, text, FeedKind::Info, ToolStatus::None);
            }
            _ => {}
        }
    }

    fn apply_assistant(&mut self, ts: i64, v: &Value, agent: &Option<String>, sidechain: bool) {
        let Some(msg) = v.get("message") else { return };
        if let Some(model) = s(msg, "model") {
            match agent {
                Some(a) => {
                    if let Some(info) = self.agents.get_mut(a) {
                        info.model = model.to_string();
                    }
                }
                None => self.model = model.to_string(),
            }
        }
        if let (Some(id), Some(u)) = (s(msg, "id"), msg.get("usage")) {
            let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            let mut uu = Usage {
                input: g("input_tokens"),
                output: g("output_tokens"),
                cache_read: g("cache_read_input_tokens"),
                c5m: 0,
                c1h: 0,
            };
            if let Some(cc) = u.get("cache_creation").filter(|c| c.is_object()) {
                let gc = |k: &str| cc.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                uu.c5m = gc("ephemeral_5m_input_tokens");
                uu.c1h = gc("ephemeral_1h_input_tokens");
            } else {
                uu.c5m = g("cache_creation_input_tokens");
            }
            let model = s(msg, "model").unwrap_or("").to_string();
            self.usage_by_msg.insert(id.to_string(), (model, uu, ts));
            if !sidechain {
                self.ctx_tokens = uu.input + uu.cache_read + uu.c5m + uu.c1h;
            }
        }
        if !sidechain {
            self.turn_open = true;
        }
        let Some(content) = msg.get("content").and_then(|c| c.as_array()) else { return };
        for block in content {
            match s(block, "type").unwrap_or("") {
                "thinking" => {
                    if let Some(t) = s(block, "thinking") {
                        if !t.trim().is_empty() {
                            self.thinking.push(Think {
                                ts,
                                agent: agent.clone(),
                                text: t.to_string(),
                            });
                        }
                    }
                }
                "text" => {
                    if let Some(t) = s(block, "text") {
                        if !t.trim().is_empty() {
                            let text = format!("▷ {}", first_line(t, 110));
                            self.push_feed(ts, agent.clone(), text, FeedKind::Reply, ToolStatus::None);
                            // Thinking text is never persisted in transcripts (empty +
                            // signature only), so assistant prose doubles as the
                            // narrative stream.
                            self.thinking.push(Think {
                                ts,
                                agent: agent.clone(),
                                text: t.to_string(),
                            });
                            if agent.is_none() {
                                self.push_ctx(ts, CtxKind::Assistant, t.to_string());
                                self.last_main_kind = LastKind::Reply;
                            }
                        }
                    }
                }
                "tool_use" => {
                    let id = s(block, "id").unwrap_or("").to_string();
                    let name = s(block, "name").unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    self.apply_tool_use(ts, agent, &id, name, &input);
                    if agent.is_none() {
                        self.last_main_kind = LastKind::Tool;
                    }
                }
                _ => {}
            }
        }
    }

    fn apply_tool_use(
        &mut self,
        ts: i64,
        agent: &Option<String>,
        id: &str,
        name: &str,
        input: &Value,
    ) {
        let inp = |k: &str| s(input, k).unwrap_or("");
        match name {
            "Read" => {
                let path = self.short_path(inp("file_path"));
                let idx = match self.reads.last_mut() {
                    Some(last) if last.path == path && last.agent == *agent && last.src == 'R' => {
                        last.count += 1;
                        last.ts = ts;
                        self.reads.len() - 1
                    }
                    _ => {
                        self.reads.push(ReadEntry {
                            ts,
                            agent: agent.clone(),
                            path,
                            count: 1,
                            src: 'R',
                            err: false,
                        });
                        self.reads.len() - 1
                    }
                };
                self.pending.insert(
                    id.to_string(),
                    Pending { read_idx: Some(idx), start_ts: ts, ..Default::default() },
                );
                if agent.is_none() {
                    let p = self.reads[idx].path.clone();
                    self.push_ctx(ts, CtxKind::Tool, format!("⏺ Read {p}"));
                }
            }
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                let raw = if name == "NotebookEdit" { inp("notebook_path") } else { inp("file_path") };
                let path = self.short_path(raw);
                if agent.is_none() {
                    self.push_ctx(ts, CtxKind::Tool, format!("⏺ {name} {path}"));
                }
                let kind = if name == "Write" { 'W' } else { 'E' };
                self.writes.push(WriteEntry {
                    ts,
                    agent: agent.clone(),
                    path,
                    kind,
                    adds: None,
                    dels: None,
                    err: false,
                });
                self.pending.insert(
                    id.to_string(),
                    Pending {
                        write_idx: Some(self.writes.len() - 1),
                        start_ts: ts,
                        ..Default::default()
                    },
                );
            }
            "Skill" => {
                let skill = inp("skill");
                let args = inp("args");
                self.tally_skill(&format!("/{}", skill.trim_start_matches('/')), ts);
                let mut text = format!("◆ skill {skill}");
                if !args.is_empty() {
                    text.push(' ');
                    text.push_str(&truncate_chars(args, 50));
                }
                if agent.is_none() {
                    self.push_ctx(ts, CtxKind::Tool, text.clone());
                }
                let fi = self.push_feed(ts, agent.clone(), text, FeedKind::Skill, ToolStatus::Pending);
                let ti = self.push_tio(
                    ts,
                    agent,
                    name,
                    serde_json::to_string_pretty(input).unwrap_or_default(),
                );
                self.pending.insert(
                    id.to_string(),
                    Pending {
                        feed_idx: Some(fi),
                        tio_idx: Some(ti),
                        start_ts: ts,
                        ..Default::default()
                    },
                );
            }
            "Task" | "Agent" => {
                let desc = if !inp("description").is_empty() {
                    inp("description").to_string()
                } else {
                    first_line(inp("prompt"), 60)
                };
                let st = inp("subagent_type");
                let text = if st.is_empty() {
                    format!("⚑ agent: {}", truncate_chars(&desc, 80))
                } else {
                    format!("⚑ agent [{st}]: {}", truncate_chars(&desc, 70))
                };
                if agent.is_none() {
                    self.push_ctx(ts, CtxKind::Tool, text.clone());
                }
                let fi = self.push_feed(ts, agent.clone(), text, FeedKind::Agent, ToolStatus::Pending);
                let ti = self.push_tio(
                    ts,
                    agent,
                    name,
                    serde_json::to_string_pretty(input).unwrap_or_default(),
                );
                self.pending.insert(
                    id.to_string(),
                    Pending {
                        feed_idx: Some(fi),
                        tio_idx: Some(ti),
                        start_ts: ts,
                        ..Default::default()
                    },
                );
            }
            _ => {
                if name == "Bash" {
                    for p in extract_read_paths(inp("command")) {
                        let sp = self.short_path(&p);
                        if let Some(last) = self.reads.last_mut() {
                            if last.path == sp && last.agent == *agent {
                                last.count += 1;
                                last.ts = ts;
                                continue;
                            }
                        }
                        self.reads.push(ReadEntry {
                            ts,
                            agent: agent.clone(),
                            path: sp,
                            count: 1,
                            src: '$',
                            err: false,
                        });
                    }
                }
                let (text, kind) = if name == "Bash" {
                    (format!("$ {}", first_line(inp("command"), 130)), FeedKind::Tool)
                } else if let Some(rest) = name.strip_prefix("mcp__") {
                    let mut parts = rest.splitn(2, "__");
                    let server = parts.next().unwrap_or("?");
                    let tool = parts.next().unwrap_or("?");
                    let args = serde_json::to_string(input).unwrap_or_default();
                    (
                        format!("◇ {server}·{tool} {}", truncate_chars(&args, 60)),
                        FeedKind::Mcp,
                    )
                } else if name == "Grep" {
                    let mut t = format!("▸ grep {}", inp("pattern"));
                    if !inp("path").is_empty() {
                        t.push_str(&format!(" in {}", self.short_path(inp("path"))));
                    }
                    (truncate_chars(&t, 130), FeedKind::Tool)
                } else if name == "Glob" {
                    (format!("▸ glob {}", inp("pattern")), FeedKind::Tool)
                } else if name == "WebFetch" {
                    (format!("▸ fetch {}", truncate_chars(inp("url"), 100)), FeedKind::Tool)
                } else if name == "WebSearch" {
                    (format!("▸ search \"{}\"", truncate_chars(inp("query"), 90)), FeedKind::Tool)
                } else {
                    let args = serde_json::to_string(input).unwrap_or_default();
                    (format!("▸ {name} {}", truncate_chars(&args, 60)), FeedKind::Tool)
                };
                if agent.is_none() {
                    self.push_ctx(ts, CtxKind::Tool, text.clone());
                }
                let io_input = if name == "Bash" {
                    inp("command").to_string()
                } else {
                    serde_json::to_string_pretty(input).unwrap_or_default()
                };
                let fi = self.push_feed(ts, agent.clone(), text, kind, ToolStatus::Pending);
                let ti = self.push_tio(ts, agent, name, io_input);
                self.pending.insert(
                    id.to_string(),
                    Pending {
                        feed_idx: Some(fi),
                        tio_idx: Some(ti),
                        start_ts: ts,
                        ..Default::default()
                    },
                );
            }
        }
    }

    fn apply_user(&mut self, ts: i64, v: &Value, agent: &Option<String>, sidechain: bool) {
        let is_meta = v.get("isMeta").and_then(|x| x.as_bool()).unwrap_or(false);
        let Some(msg) = v.get("message") else { return };
        let content = msg.get("content");

        // post-compaction summary: huge injected user message, not a prompt
        if v.get("isCompactSummary").and_then(|x| x.as_bool()) == Some(true) {
            let text = match content {
                Some(Value::String(t)) => t.clone(),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|b| s(b, "text"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            self.push_feed(
                ts,
                None,
                "⚠ compact summary injected".into(),
                FeedKind::Warn,
                ToolStatus::None,
            );
            self.push_ctx(ts, CtxKind::Summary, text);
            return;
        }

        // plain string content: a prompt, meta command, or agent task prompt
        if let Some(text) = content.and_then(|c| c.as_str()) {
            self.handle_user_text(ts, v, agent, sidechain, is_meta, text);
            return;
        }

        let Some(arr) = content.and_then(|c| c.as_array()) else { return };
        let mut texts: Vec<&str> = Vec::new();
        for block in arr {
            match s(block, "type").unwrap_or("") {
                "tool_result" => {
                    let tid = s(block, "tool_use_id").unwrap_or("");
                    let is_err = block.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
                    let rtext = result_text(block);
                    self.resolve_tool(ts, v, tid, is_err, rtext);
                    if !sidechain {
                        self.last_main_kind = LastKind::Tool;
                    }
                }
                "text" => {
                    if let Some(t) = s(block, "text") {
                        texts.push(t);
                    }
                }
                _ => {}
            }
        }
        if !texts.is_empty() {
            let joined = texts.join("\n");
            self.handle_user_text(ts, v, agent, sidechain, is_meta, &joined);
        }
    }

    fn handle_user_text(
        &mut self,
        ts: i64,
        _v: &Value,
        agent: &Option<String>,
        sidechain: bool,
        is_meta: bool,
        text: &str,
    ) {
        if sidechain {
            if let Some(a) = agent {
                if let Some(info) = self.agents.get_mut(a) {
                    if info.desc.is_empty() {
                        info.desc = first_line(text, 60);
                    }
                }
            }
            return;
        }
        // slash-command invocations arrive as (sometimes non-meta) messages
        // wrapping the command name in tags
        if let Some(start) = text.find("<command-name>") {
            let rest = &text[start + "<command-name>".len()..];
            if let Some(end) = rest.find("</command-name>") {
                let cmd = rest[..end].trim().to_string();
                if !cmd.is_empty() {
                    self.tally_skill(&cmd, ts);
                }
            }
            return;
        }
        if is_meta {
            return;
        }
        if text.trim().is_empty() || text.starts_with("<command-") || text.starts_with("<local-command") {
            return;
        }
        let line = format!("❯ {}", first_line(text, 130));
        self.push_feed(ts, None, line, FeedKind::Prompt, ToolStatus::None);
        self.push_ctx(ts, CtxKind::User, text.to_string());
        self.last_main_kind = LastKind::Prompt;
        if !self.turn_open {
            self.turn_open = true;
            self.turn_start_ts = ts;
        }
    }

    fn push_ctx(&mut self, ts: i64, kind: CtxKind, text: String) {
        self.ctx.push(CtxMsg { ts, kind, text });
        self.ctx_rev += 1;
    }

    fn push_tio(&mut self, ts: i64, agent: &Option<String>, name: &str, input: String) -> usize {
        self.tool_ios.push(ToolIO {
            ts,
            agent: agent.clone(),
            name: name.to_string(),
            input,
            output: None,
            err: false,
            dur_ms: 0,
        });
        self.tio_rev += 1;
        self.tool_ios.len() - 1
    }

    pub fn load_memory(&mut self) {
        let Some(sref) = self.sessions.get(self.sel) else { return };
        let dir = sref.project_dir.join("memory");
        let mut metas: Vec<(String, PathBuf, u64)> = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("md") {
                    continue;
                }
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let mtime = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                metas.push((name, p, mtime));
            }
        }
        let stamp = (metas.len() as u64, metas.iter().map(|(_, _, m)| m).sum());
        if stamp == self.mem_stamp {
            return;
        }
        self.mem_stamp = stamp;
        metas.sort_by_key(|(n, _, _)| (n != "MEMORY.md", n.clone()));
        self.memory_files = metas
            .into_iter()
            .filter_map(|(n, p, _)| fs::read_to_string(&p).ok().map(|c| (n, c)))
            .collect();
        self.mem_rev += 1;
    }

    fn resolve_tool(
        &mut self,
        ts: i64,
        v: &Value,
        tid: &str,
        is_err: bool,
        rtext: Option<String>,
    ) {
        if is_err {
            if let Some(t) = &rtext {
                self.note_hook_block(ts, t);
            }
        }
        let Some(p) = self.pending.remove(tid) else { return };
        if let Some(ti) = p.tio_idx {
            if let Some(io) = self.tool_ios.get_mut(ti) {
                io.output = Some(rtext.clone().unwrap_or_default());
                io.err = is_err;
                io.dur_ms = ts - p.start_ts;
            }
            self.tio_rev += 1;
        }
        if let Some(ri) = p.read_idx {
            if is_err {
                if let Some(r) = self.reads.get_mut(ri) {
                    r.err = true;
                }
            }
        }
        if let Some(wi) = p.write_idx {
            let tur = v.get("toolUseResult");
            let (mut adds, mut dels) = (0u64, 0u64);
            let mut have = false;
            if let Some(tur) = tur {
                if let Some(patches) = tur.get("structuredPatch").and_then(|x| x.as_array()) {
                    for hunk in patches {
                        if let Some(lines) = hunk.get("lines").and_then(|x| x.as_array()) {
                            for l in lines {
                                if let Some(ls) = l.as_str() {
                                    if ls.starts_with('+') {
                                        adds += 1;
                                    } else if ls.starts_with('-') {
                                        dels += 1;
                                    }
                                }
                            }
                        }
                    }
                    have = true;
                } else if s(tur, "type") == Some("create") {
                    if let Some(c) = s(tur, "content") {
                        adds = c.lines().count() as u64;
                        have = true;
                    }
                }
            }
            if let Some(w) = self.writes.get_mut(wi) {
                if have {
                    w.adds = Some(adds);
                    w.dels = Some(dels);
                }
                w.err = is_err;
            }
        }
        if let Some(fi) = p.feed_idx {
            let dur = ts - p.start_ts;
            if let Some(item) = self.feed.get_mut(fi) {
                item.status = if is_err { ToolStatus::Err } else { ToolStatus::Ok };
                if dur > 2000 {
                    item.text.push_str(&format!(" ({})", fmt_dur(dur)));
                }
            }
        }
    }

    /// Tool/prompt hooks that PASS are never logged to the transcript; the
    /// only trace is a denial baked into an error tool_result, shaped like:
    /// `PreToolUse:Bash hook error: [bash …/guard.sh]: Blocked by guard: …`
    fn note_hook_block(&mut self, ts: i64, text: &str) {
        let Some(pos) = text.find(" hook error: [") else { return };
        let event = text[..pos].to_string();
        let rest = &text[pos + " hook error: [".len()..];
        let Some(close) = rest.find("]:") else { return };
        let bracket = &rest[..close];
        let msg = rest[close + 2..].trim();
        let (key, name) = match self.hooks_config.iter().find(|c| bracket.contains(&c.name)) {
            Some(c) => (
                c.status_message.clone().unwrap_or_else(|| c.command.clone()),
                c.name.clone(),
            ),
            None => (truncate_chars(bracket, 40), truncate_chars(bracket, 28)),
        };
        let stat = self.hook_stats.entry(key).or_default();
        stat.acted += 1;
        stat.last_ts = ts;
        let label = format!("⛔ {event} blocked · {name}");
        let detail = msg.trim().to_string();
        let feed_text = format!("{label}: {}", first_line(msg, 60));
        self.push_feed(ts, None, feed_text, FeedKind::Warn, ToolStatus::None);
        self.hook_actions.push(HookAction { ts, label, detail, sev: 2 });
    }

    fn apply_system(&mut self, ts: i64, v: &Value, sidechain: bool) {
        let subtype = s(v, "subtype").unwrap_or("");
        if v.get("hookInfos").is_some() || subtype.contains("hook") {
            self.apply_hook_summary(ts, v);
            return;
        }
        match subtype {
            "turn_duration" => {
                if !sidechain {
                    self.turn_open = false;
                    self.last_turn_end_ts = ts;
                    let dur = v.get("durationMs").and_then(|x| x.as_i64()).unwrap_or(0);
                    let text = format!("· turn ended ({})", fmt_dur(dur));
                    self.push_feed(ts, None, text, FeedKind::Info, ToolStatus::None);
                }
            }
            "compact_boundary" => {
                let md = v.get("compactMetadata");
                let g = |k: &str| md.and_then(|m| m.get(k)).and_then(|x| x.as_u64()).unwrap_or(0);
                let (pre, post) = (g("preTokens"), g("postTokens"));
                let text = if pre > 0 {
                    format!("⚠ context compacted ({} → {})", fmt_tok(pre), fmt_tok(post))
                } else {
                    "⚠ context compacted".to_string()
                };
                self.push_feed(ts, None, text.clone(), FeedKind::Warn, ToolStatus::None);
                self.push_ctx(ts, CtxKind::Boundary, text);
            }
            _ => {}
        }
    }

    fn apply_hook_summary(&mut self, ts: i64, v: &Value) {
        let mut first_cmd = String::new();
        if let Some(infos) = v.get("hookInfos").and_then(|x| x.as_array()) {
            for hi in infos {
                let cmd = s(hi, "command").unwrap_or("(unknown hook)").to_string();
                if first_cmd.is_empty() {
                    first_cmd = cmd.clone();
                }
                let dur = hi.get("durationMs").and_then(|x| x.as_u64()).unwrap_or(0);
                let stat = self.hook_stats.entry(cmd).or_default();
                stat.count += 1;
                stat.total_ms += dur;
                stat.last_ts = ts;
            }
        }
        let mut actions: Vec<(String, String, u8)> = Vec::new();
        if v.get("preventedContinuation").and_then(|x| x.as_bool()) == Some(true) {
            let reason = s(v, "stopReason").unwrap_or("").to_string();
            actions.push(("⛔ hook blocked stop".into(), reason, 2));
        }
        if v.get("hasOutput").and_then(|x| x.as_bool()) == Some(true) {
            actions.push(("✱ hook output".into(), String::new(), 1));
        }
        if let Some(errs) = v.get("hookErrors").and_then(|x| x.as_array()) {
            if !errs.is_empty() {
                let msg = errs
                    .iter()
                    .filter_map(|e| e.as_str().map(str::to_string).or_else(|| Some(e.to_string())))
                    .collect::<Vec<_>>()
                    .join("; ");
                actions.push(("✗ hook error".into(), msg, 2));
            }
        }
        if let Some(ctx) = s(v, "hookAdditionalContext") {
            if !ctx.trim().is_empty() {
                actions.push(("✚ hook injected context".into(), ctx.trim().to_string(), 1));
            }
        }
        for (label, detail, sev) in actions {
            let label_full = if first_cmd.is_empty() {
                label.clone()
            } else {
                format!("{label} · {}", truncate_chars(&first_cmd, 40))
            };
            if let Some(stat) = self.hook_stats.get_mut(&first_cmd) {
                stat.acted += 1;
            }
            let feed_text = if detail.is_empty() {
                label_full.clone()
            } else {
                format!("{label_full}: {}", truncate_chars(&detail, 60))
            };
            self.push_feed(ts, None, feed_text, FeedKind::Warn, ToolStatus::None);
            self.hook_actions.push(HookAction { ts, label: label_full, detail, sev });
        }
    }

    fn apply_attachment(&mut self, ts: i64, v: &Value) {
        let Some(att) = v.get("attachment") else { return };
        match s(att, "type").unwrap_or("") {
            "hook_cancelled" => {
                let name = s(att, "hookName").unwrap_or("hook").to_string();
                let cmd = s(att, "command").unwrap_or("").to_string();
                let timed_out = att.get("timedOut").and_then(|x| x.as_bool()).unwrap_or(false);
                let dur = att.get("durationMs").and_then(|x| x.as_u64()).unwrap_or(0) as i64;
                let verb = if timed_out { "timed out" } else { "cancelled" };
                let label = format!("✋ {name} {verb}");
                let detail = if timed_out {
                    let limit = att.get("timeoutMs").and_then(|x| x.as_u64()).unwrap_or(0);
                    format!("hit its {} timeout — tool call proceeded without it", fmt_dur(limit as i64))
                } else {
                    format!("cancelled after {}", fmt_dur(dur))
                };
                if !cmd.is_empty() {
                    let stat = self.hook_stats.entry(cmd).or_default();
                    stat.acted += 1;
                    stat.last_ts = ts;
                }
                self.hook_actions.push(HookAction { ts, label: label.clone(), detail, sev: 1 });
                self.push_feed(ts, None, label, FeedKind::Warn, ToolStatus::None);
            }
            "hook_non_blocking_error" => {
                let detail = s(att, "error").or_else(|| s(att, "message")).unwrap_or("").to_string();
                self.hook_actions.push(HookAction {
                    ts,
                    label: "✗ hook error (non-blocking)".into(),
                    detail: detail.clone(),
                    sev: 2,
                });
                let text = format!("✗ hook error: {}", truncate_chars(&detail, 70));
                self.push_feed(ts, None, text, FeedKind::Warn, ToolStatus::None);
            }
            "file" => {
                let path = s(att, "displayPath").or_else(|| s(att, "filename")).unwrap_or("");
                if !path.is_empty() {
                    let sp = self.short_path(path);
                    self.reads.push(ReadEntry {
                        ts, agent: None, path: sp, count: 1, src: '@', err: false,
                    });
                }
            }
            "edited_text_file" => {
                let path = s(att, "filename").unwrap_or("");
                if !path.is_empty() {
                    let sp = self.short_path(path);
                    self.reads.push(ReadEntry {
                        ts, agent: None, path: sp, count: 1, src: '±', err: false,
                    });
                }
            }
            "plan_mode" => {
                self.push_feed(ts, None, "· entered plan mode".into(), FeedKind::Info, ToolStatus::None);
            }
            "plan_mode_exit" => {
                self.push_feed(ts, None, "· exited plan mode".into(), FeedKind::Info, ToolStatus::None);
            }
            _ => {}
        }
    }

    // ---------- view 0: machine-wide sessions ----------

    pub fn refresh_overview(&mut self) {
        let selected_id = self.overview.get(self.overview_sel).map(|o| o.id.clone());
        let mut fresh: Vec<Option<crate::overview::OverviewSession>> =
            crate::overview::scan(30 * 60 * 1000).into_iter().map(Some).collect();
        // stable order: existing sessions keep their positions (updated in
        // place), vanished ones drop out, new ones append at the bottom —
        // state changes show via color, not reordering
        let mut list: Vec<crate::overview::OverviewSession> = Vec::new();
        for old in &self.overview {
            if let Some(slot) = fresh
                .iter_mut()
                .find(|f| f.as_ref().map(|x| x.id == old.id).unwrap_or(false))
            {
                list.push(slot.take().unwrap());
            }
        }
        for f in fresh.into_iter().flatten() {
            list.push(f);
        }
        self.overview = list;
        self.overview_sel = selected_id
            .and_then(|id| self.overview.iter().position(|o| o.id == id))
            .unwrap_or_else(|| self.overview_sel.min(self.overview.len().saturating_sub(1)));
    }

    pub fn overview_move(&mut self, d: i64) {
        let n = self.overview.len();
        if n == 0 {
            return;
        }
        let cur = self.overview_sel as i64;
        self.overview_sel = (cur + d).clamp(0, n as i64 - 1) as usize;
    }

    /// Re-point the whole watcher at the selected session's folder and open it.
    pub fn jump_to_selected(&mut self) {
        let Some(o) = self.overview.get(self.overview_sel) else { return };
        if o.cwd.is_empty() {
            return;
        }
        let id = o.id.clone();
        self.cwd = PathBuf::from(&o.cwd);
        self.hooks_config = load_hook_config(&self.cwd);
        self.sessions = discover::discover_sessions(&self.cwd);
        if let Some(i) = self.sessions.iter().position(|x| x.id == id) {
            self.open_session(i);
            self.auto_follow = false;
            self.layout = 1;
            self.focus = PaneId::Feed;
        }
    }

    // ---------- view g / space: github + haunt ----------

    pub fn ghv(&self) -> &GhView {
        &self.gh_views[self.gh_mode.idx()]
    }

    fn view_mut(&mut self, m: GhMode) -> &mut GhView {
        &mut self.gh_views[m.idx()]
    }

    fn gh_stale(&self, m: GhMode) -> bool {
        let at = self.gh_views[m.idx()].gh.fetched_at_ms;
        at == 0 || now_ms() - at > m.ttl_secs() * 1000
    }

    /// Switch to a mode and show it immediately from whatever is already in
    /// memory or on disk; any refetch happens behind the rendered data.
    pub fn gh_open(&mut self, m: GhMode) {
        self.layout = 7;
        self.focus = PaneId::GitHub;
        if self.gh_mode != m {
            self.gh_mode = m;
            self.scroll.insert(PaneId::GitHub, 0);
        }
        self.load_gcache(m);
        if self.gh_stale(m) {
            self.gh_refresh(m);
        }
    }

    /// Space switches the g view between its two modes. It is a modifier on
    /// that pane, not a way into it — pressed anywhere else it does nothing.
    pub fn gh_toggle_digest(&mut self) {
        if self.layout != 7 {
            return;
        }
        let next = match self.gh_mode {
            GhMode::Digest => GhMode::Live,
            GhMode::Live => GhMode::Digest,
        };
        self.gh_open(next);
    }

    /// Claim the right to start a fetch for this mode, marking it in flight.
    fn gh_claim(&mut self, m: GhMode) -> bool {
        let v = &self.gh_views[m.idx()];
        let backoff = v
            .last_attempt
            .map(|t| t.elapsed() < Duration::from_secs(10))
            .unwrap_or(false);
        if v.gh.fetching || backoff {
            return false;
        }
        let v = self.view_mut(m);
        v.gh.fetching = true;
        v.last_attempt = Some(Instant::now());
        true
    }

    /// Populate both modes at startup so the first switch either way is instant.
    /// The two fetches share one thread — sequential keeps the burst of `gh`
    /// subprocesses to what a single mode already costs.
    pub fn gh_boot(&mut self) {
        let mut need = Vec::new();
        for m in GhMode::ALL {
            self.load_gcache(m);
            if self.gh_stale(m) && self.gh_claim(m) {
                need.push(m);
            }
        }
        if need.is_empty() {
            return;
        }
        let tx = self.gh_tx.clone();
        std::thread::spawn(move || {
            for m in need {
                fetch_mode(m, &tx);
            }
        });
    }

    /// Warm the security view at startup so the first `s` is instant. The
    /// advisories are one fast registry read now; the deploy heads are the slow
    /// half, so they get their own thread and their own clock.
    pub fn cve_boot(&mut self) {
        self.load_ccache();
        if self.cve_stale() {
            self.cve_refresh();
        }
        if self.dep_stale() {
            self.dep_refresh();
        }
    }

    /// Adopt this mode's cache if another instance wrote something newer.
    fn load_gcache(&mut self, m: GhMode) {
        let Some(c) = crate::gcache::load(m.tag()) else { return };
        let v = self.view_mut(m);
        if c.fetched_at_ms > v.gh.fetched_at_ms {
            v.gh.runs = c.runs;
            v.gh.prs = c.prs;
            v.gh.error = c.gh_error;
            v.gh.fetched_at_ms = c.fetched_at_ms;
            v.haunt = crate::haunt::HauntState {
                runs: c.haunt_runs,
                roadmap_err: c.roadmap_err,
                sites_err: c.sites_err,
            };
        }
    }

    pub fn gh_refresh(&mut self, m: GhMode) {
        if !self.gh_claim(m) {
            return;
        }
        let tx = self.gh_tx.clone();
        std::thread::spawn(move || fetch_mode(m, &tx));
    }

    // ---------- view v: dependabot across the managed sites ----------

    /// Open the security view, showing whatever is cached while any reload runs
    /// behind it.
    pub fn cve_open(&mut self) {
        // pressing s on the view already showing means "reload now"
        let reload = self.layout == 8;
        self.layout = 8;
        self.focus = PaneId::Cve;
        // a report reads from the top, unlike the feeds this offset convention
        // was built for; the render clamps this to "as far up as it goes"
        self.scroll.insert(PaneId::Cve, usize::MAX);
        self.load_ccache();
        if reload || self.cve_stale() {
            self.cve_refresh();
        }
        if reload || self.dep_stale() {
            self.dep_refresh();
        }
    }

    /// Cycle the tier filter — server → build → deploy → all. This is the axis
    /// that used to be faked by splitting npm in two.
    pub fn cycle_cve_tier(&mut self, dir: i64) {
        let n = self.cve.tier_count() as i64;
        if n <= 1 {
            return;
        }
        self.cve_filters.tier = (self.cve_filters.tier as i64 + dir).rem_euclid(n) as usize;
        self.scroll.insert(PaneId::Cve, usize::MAX);
    }

    /// Cycle the ecosystem filter — all → npm → composer → rubygems → all.
    pub fn cycle_cve_eco(&mut self, dir: i64) {
        let n = self.cve.eco_count() as i64;
        if n <= 1 {
            return;
        }
        self.cve_filters.eco = (self.cve_filters.eco as i64 + dir).rem_euclid(n) as usize;
        self.scroll.insert(PaneId::Cve, usize::MAX);
    }

    /// Lower the severity floor — critical+high → +medium → everything. The
    /// old panel could only ever ask GitHub for critical+high; the registry
    /// hands over the lot, so the floor is a choice now.
    pub fn cycle_cve_sev(&mut self) {
        self.cve_filters.sev = (self.cve_filters.sev + 1) % 3;
        self.scroll.insert(PaneId::Cve, usize::MAX);
    }

    /// Hide advisories with nowhere to upgrade to.
    pub fn toggle_cve_fixable(&mut self) {
        self.cve_filters.fixable = !self.cve_filters.fixable;
        self.scroll.insert(PaneId::Cve, usize::MAX);
    }

    /// A reload can drop a tier or an ecosystem entirely.
    fn clamp_cve_filters(&mut self) {
        if self.cve_filters.tier >= self.cve.tier_count() {
            self.cve_filters.tier = 0;
        }
        if self.cve_filters.eco >= self.cve.eco_count() {
            self.cve_filters.eco = 0;
        }
    }

    /// The registry pulls from GitHub on its own schedule and stamps the
    /// result; our read is cheap, so five minutes keeps `refreshed_at` honest
    /// without hammering it.
    fn cve_stale(&self) -> bool {
        self.cve.fetched_at_ms == 0 || now_ms() - self.cve.fetched_at_ms > 5 * 60_000
    }

    /// Deploys are ~25 GitHub calls, and a deploy lands maybe hourly.
    fn dep_stale(&self) -> bool {
        self.deploys.fetched_at_ms == 0 || now_ms() - self.deploys.fetched_at_ms > 15 * 60_000
    }

    fn load_ccache(&mut self) {
        if let Some(c) = crate::gcache::load_cve() {
            if c.fetched_at_ms > self.cve.fetched_at_ms {
                self.cve = c;
                self.clamp_cve_filters();
            }
        }
        if let Some(d) = crate::gcache::load_deploys() {
            if d.fetched_at_ms > self.deploys.fetched_at_ms {
                self.deploys = d;
            }
        }
    }

    pub fn cve_refresh(&mut self) {
        let backoff = self
            .cve_attempt
            .map(|t| t.elapsed() < Duration::from_secs(10))
            .unwrap_or(false);
        if self.cve.fetching || backoff {
            return;
        }
        self.cve.fetching = true;
        self.cve_attempt = Some(Instant::now());
        let tx = self.cve_tx.clone();
        std::thread::spawn(move || {
            let mut st = crate::cve::fetch();
            crate::gcache::store_cve(&st);
            st.fetching = false;
            let _ = tx.send(st);
        });
    }

    pub fn dep_refresh(&mut self) {
        let backoff = self
            .dep_attempt
            .map(|t| t.elapsed() < Duration::from_secs(30))
            .unwrap_or(false);
        if self.deploys.fetching || backoff {
            return;
        }
        self.deploys.fetching = true;
        self.dep_attempt = Some(Instant::now());
        let tx = self.dep_tx.clone();
        std::thread::spawn(move || {
            // the slow half is still worth serialising across instances
            if !crate::gcache::try_lock("deploys") {
                let _ = tx.send(crate::cve::DeployState::default());
                return;
            }
            let mut st = crate::cve::fetch_deploys();
            crate::gcache::store_deploys(&st);
            crate::gcache::unlock("deploys");
            st.fetching = false;
            let _ = tx.send(st);
        });
    }

    // ---------- view m: the sites registry ----------

    /// Same contract as the security view: instant from cache, and pressing the
    /// key again on the view already showing forces a reload.
    pub fn sites_open(&mut self) {
        let reload = self.layout == 9;
        self.layout = 9;
        self.focus = PaneId::Sites;
        self.scroll.insert(PaneId::Sites, usize::MAX);
        self.load_scache();
        if reload || self.sites_stale() {
            self.sites_refresh();
        }
    }

    pub fn sites_boot(&mut self) {
        self.load_scache();
        if self.sites_stale() {
            self.sites_refresh();
        }
    }

    /// The registry moves when someone patches a site — minutes, not seconds.
    fn sites_stale(&self) -> bool {
        self.sites_reg.fetched_at_ms == 0
            || now_ms() - self.sites_reg.fetched_at_ms > 10 * 60_000
    }

    fn load_scache(&mut self) {
        if let Some(c) = crate::gcache::load_sites() {
            if c.fetched_at_ms > self.sites_reg.fetched_at_ms {
                self.sites_reg = c;
            }
        }
    }

    pub fn sites_refresh(&mut self) {
        let backoff = self
            .sites_attempt
            .map(|t| t.elapsed() < Duration::from_secs(3))
            .unwrap_or(false);
        if self.sites_reg.fetching || backoff {
            return;
        }
        self.sites_reg.fetching = true;
        self.sites_attempt = Some(Instant::now());
        let tx = self.sites_tx.clone();
        std::thread::spawn(move || {
            let mut st = crate::sitelist::fetch();
            if st.error.is_none() {
                crate::gcache::store_sites(&st);
            }
            st.fetching = false;
            let _ = tx.send(st);
        });
    }

    fn apply_gh(&mut self, msg: GhMsg) {
        let v = self.view_mut(msg.mode);
        v.gh.fetching = false;
        if let Some((g, h)) = msg.data {
            v.gh.runs = g.runs;
            v.gh.prs = g.prs;
            v.gh.error = g.error;
            v.gh.fetched_at_ms = now_ms();
            v.haunt = h;
        }
    }

    // ---------- debug ----------

    pub fn dump(&self) {
        println!("sessions: {}", self.sessions.len());
        for (i, sr) in self.sessions.iter().enumerate() {
            let mark = if i == self.sel { "*" } else { " " };
            let wt = sr.worktree.as_deref().map(|w| format!(" [wt:{w}]")).unwrap_or_default();
            println!(" {mark} {}{wt}", sr.id);
        }
        println!("title: {} | model: {} | branch: {}", self.title, self.model, self.branch);
        let st = match self.status() {
            Status::Working => "WORKING",
            Status::Waiting => "WAITING",
            Status::Blocked => "BLOCKED",
        };
        println!("status: {st} | turn_open: {}", self.turn_open);
        let prompts = self.feed.iter().filter(|f| matches!(f.kind, FeedKind::Prompt)).count();
        println!(
            "feed: {} ({prompts} prompts) | reads: {} | writes: {} | narrative: {} | hooks: {} | actions: {} | skills: {} | agents: {}",
            self.feed.len(),
            self.reads.len(),
            self.writes.len(),
            self.thinking.len(),
            self.hook_stats.len(),
            self.hook_actions.len(),
            self.skills.len(),
            self.agents.len()
        );
        let t = self.totals();
        println!(
            "tokens: in {} out {} cache_read {} cache_write {} | ctx {} / {} | cost US${:.4} NZ${:.4}",
            fmt_tok(t.input),
            fmt_tok(t.output),
            fmt_tok(t.cache_read),
            fmt_tok(t.c5m + t.c1h),
            fmt_tok(self.ctx_tokens),
            fmt_tok(self.effective_window()),
            self.cost_usd(),
            self.cost_usd() * self.nzd_rate
        );
        println!("--- last feed ---");
        for it in self.feed.iter().rev().take(15).collect::<Vec<_>>().into_iter().rev() {
            let tag = self.agent_tag(&it.agent).map(|(t, _)| format!("{t} ")).unwrap_or_default();
            println!("{} {}{}", fmt_clock(it.ts), tag, it.text);
        }
        println!("--- hooks ---");
        for (cmd, st) in &self.hook_stats {
            println!("×{} acted:{} avg {}ms  {}", st.count, st.acted, if st.count > 0 { st.total_ms / st.count } else { 0 }, cmd);
        }
        for a in &self.hook_actions {
            println!("ACTED {} {} {}", fmt_clock(a.ts), a.label, a.detail);
        }
        println!("--- skills ---");
        for sk in &self.skills {
            println!("{} ×{}", sk.name, sk.count);
        }
        println!("--- agents ---");
        for k in &self.agent_order {
            if let Some(a) = self.agents.get(k) {
                println!("{}:{} model={} {}", price::model_short(&a.model), a.idx, a.model, a.desc);
            }
        }
    }
}

/// Split a shell command into pipeline/list segments of whitespace tokens,
/// respecting single/double quotes so quoted filters stay one token.
fn shell_segments(cmd: &str) -> Vec<Vec<String>> {
    let mut segs: Vec<Vec<String>> = Vec::new();
    let mut seg: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                }
                '|' | ';' | '&' | '\n' => {
                    if !cur.is_empty() {
                        seg.push(std::mem::take(&mut cur));
                    }
                    if !seg.is_empty() {
                        segs.push(std::mem::take(&mut seg));
                    }
                }
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        seg.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if !cur.is_empty() {
        seg.push(cur);
    }
    if !seg.is_empty() {
        segs.push(seg);
    }
    segs
}

/// Best-effort extraction of file paths a shell command reads into context.
fn extract_read_paths(cmd: &str) -> Vec<String> {
    const DIRECT: [&str; 6] = ["cat", "head", "tail", "bat", "less", "more"];
    const PATTERN_FIRST: [&str; 3] = ["grep", "rg", "jq"];
    let mut out: Vec<String> = Vec::new();
    for toks in shell_segments(cmd) {
        let Some(first) = toks.first() else { continue };
        let prog = first.rsplit('/').next().unwrap_or(first);
        let mut pattern_pending = PATTERN_FIRST.contains(&prog);
        if !DIRECT.contains(&prog) && !pattern_pending {
            continue;
        }
        for t in &toks[1..] {
            if t.starts_with('>') || t.starts_with('<') {
                break;
            }
            // embedded redirects like 2>/dev/null, 2>&1 are not paths
            if t.contains('>') || t.contains('<') {
                continue;
            }
            if t.starts_with('-') || t.starts_with('$') || t.starts_with('`') || t.starts_with('(')
            {
                continue;
            }
            if t.starts_with("/dev/") || t.starts_with("/proc/") {
                continue;
            }
            if pattern_pending {
                pattern_pending = false;
                continue;
            }
            if t.len() < 3 {
                continue;
            }
            let looks_like_path =
                t.contains('/') || std::path::Path::new(t.as_str()).extension().is_some();
            if looks_like_path {
                out.push(t.clone());
                if out.len() >= 6 {
                    return out;
                }
            }
        }
    }
    out
}

fn load_hook_config(cwd: &Path) -> Vec<ConfigHook> {
    let mut out: Vec<ConfigHook> = Vec::new();
    let paths = [
        discover::home().join(".claude").join("settings.json"),
        cwd.join(".claude").join("settings.json"),
        cwd.join(".claude").join("settings.local.json"),
    ];
    for p in paths {
        let Ok(txt) = fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let Some(hooks) = v.get("hooks").and_then(|h| h.as_object()) else { continue };
        for (event, groups) in hooks {
            let Some(groups) = groups.as_array() else { continue };
            for grp in groups {
                let Some(hs) = grp.get("hooks").and_then(|h| h.as_array()) else { continue };
                for h in hs {
                    let command = s(h, "command").unwrap_or("").to_string();
                    if command.is_empty() {
                        continue;
                    }
                    if out.iter().any(|c| c.event == *event && c.command == command) {
                        continue;
                    }
                    let name = command
                        .split_whitespace()
                        .find(|t| t.contains('/'))
                        .and_then(|t| t.rsplit('/').next())
                        .unwrap_or(&command)
                        .to_string();
                    out.push(ConfigHook {
                        event: event.clone(),
                        name: truncate_chars(&name, 28),
                        status_message: s(h, "statusMessage").map(str::to_string),
                        command,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_shell_read_paths() {
        assert_eq!(
            extract_read_paths("cat src/main.rs 2>/dev/null | grep foo"),
            vec!["src/main.rs".to_string()]
        );
        // pattern args are not paths; redirects are not paths
        assert_eq!(
            extract_read_paths("jq -r '.type | select(. != null)' logs/session.jsonl 2>/dev/null"),
            vec!["logs/session.jsonl".to_string()]
        );
        assert_eq!(
            extract_read_paths("grep -rl 'foo/bar' --include='*.rs' src/app.rs"),
            vec!["src/app.rs".to_string()]
        );
        // writes, devices and command output are not reads
        assert!(extract_read_paths("cat notes.txt > out.txt").len() == 1);
        assert!(extract_read_paths("echo hi 2>/dev/null").is_empty());
        assert!(extract_read_paths("cargo build 2>&1").is_empty());
    }
}
