use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use std::collections::HashMap;

use crate::app::{
    fmt_clock, fmt_dur, fmt_tok, now_ms, tail_truncate, truncate_chars, App, CtxKind, FeedItem,
    FeedKind, GhMode, PaneId, SearchState, Status, TLine, ThinkFilter, ToolStatus,
};

pub fn pane_label(p: PaneId) -> &'static str {
    match p {
        PaneId::Feed => "activity",
        PaneId::Thinking => "narrative",
        PaneId::Memory => "memory",
        PaneId::Context => "context",
        PaneId::ToolIO => "tool i/o",
        PaneId::Reads => "reads",
        PaneId::Writes => "writes",
        PaneId::Hooks => "hooks",
        PaneId::Skills => "skills",
        PaneId::Overview => "sessions",
        PaneId::GitHub => "github",
        PaneId::Cve => "security",
    }
}

/// Recompute matches for the search target from its display-line texts.
fn engage_search(search: &mut SearchState, pending_jump: &mut Option<usize>, texts: &[String]) {
    search.matches.clear();
    let Some(q) = &search.query else { return };
    let ql = q.to_lowercase();
    for (i, t) in texts.iter().enumerate() {
        if t.contains(&ql) {
            search.matches.push(i);
        }
    }
    if search.jump_pending {
        search.jump_pending = false;
        if !search.matches.is_empty() {
            search.cur = search.matches.len() - 1;
            *pending_jump = Some(search.matches[search.cur]);
        }
    }
    if search.cur >= search.matches.len() {
        search.cur = search.matches.len().saturating_sub(1);
    }
}

/// Consume a pending search jump by scrolling the target pane to the line.
fn take_jump(
    scroll: &mut HashMap<PaneId, usize>,
    search: &SearchState,
    pending_jump: &mut Option<usize>,
    pane: PaneId,
    total: usize,
    h: usize,
) {
    if search.target != pane {
        return;
    }
    if let Some(idx) = pending_jump.take() {
        let off = total
            .saturating_sub(idx + h / 2 + 1)
            .min(total.saturating_sub(h));
        scroll.insert(pane, off);
    }
}

fn search_suffix(app: &App, pane: PaneId) -> String {
    if app.search.target != pane {
        return String::new();
    }
    match &app.search.query {
        None => String::new(),
        Some(q) => {
            if app.search.matches.is_empty() {
                format!(" · \"{q}\" no matches")
            } else {
                format!(" · \"{q}\" {}/{}", app.search.cur + 1, app.search.matches.len())
            }
        }
    }
}

/// Reverse-video the matched lines in the visible slice; current match in yellow.
fn highlight_matches(visible: &mut [Line<'_>], start: usize, matches: &[usize], cur: Option<usize>) {
    if matches.is_empty() {
        return;
    }
    for (i, line) in visible.iter_mut().enumerate() {
        let gi = start + i;
        if matches.binary_search(&gi).is_ok() {
            let st = if cur == Some(gi) {
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::REVERSED)
            };
            *line = std::mem::take(line).patch_style(st);
        }
    }
}

fn line_text_lower(l: &Line<'_>) -> String {
    l.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
        .to_lowercase()
}

const AGENT_COLORS: [Color; 6] = [
    Color::LightMagenta,
    Color::LightBlue,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightRed,
    Color::Cyan,
];

pub fn agent_color(idx: usize) -> Color {
    AGENT_COLORS[(idx.saturating_sub(1)) % AGENT_COLORS.len()]
}

fn accent_color(status: Status) -> Color {
    match status {
        Status::Working => Color::Green,
        Status::Waiting => Color::Yellow,
        Status::Blocked => Color::Red,
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let status = app.status();
    let accent = accent_color(status);
    app.pane_rects.clear();
    let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(f.area());
    match app.layout {
        0 => render_overview(f, app, rows[0], accent),
        7 => render_github(f, app, rows[0], accent),
        8 => render_cve(f, app, rows[0], accent),
        2 => layout_ops(f, app, rows[0], accent),
        3 => render_feed(f, app, rows[0], accent),
        4 => render_toolio(f, app, rows[0], accent),
        5 => render_context(f, app, rows[0], accent),
        6 => render_memory(f, app, rows[0], accent),
        _ => layout_default(f, app, rows[0], accent),
    }
    status_bar(f, app, rows[1], status, accent);
}

fn layout_default(f: &mut Frame, app: &mut App, area: Rect, accent: Color) {
    let main = Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)]).split(area);
    let top = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(main[0]);
    render_thinking(f, app, top[0], accent);
    let rail = Layout::vertical([
        Constraint::Percentage(30),
        Constraint::Percentage(30),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
    ])
    .split(top[1]);
    render_reads(f, app, rail[0], accent);
    render_writes(f, app, rail[1], accent);
    render_hooks(f, app, rail[2], accent, PaneId::Hooks, false);
    render_skills(f, app, rail[3], accent);
    render_feed(f, app, main[1], accent);
}

/// Ops view: no narrative at all — just the feed and the rail.
fn layout_ops(f: &mut Frame, app: &mut App, area: Rect, accent: Color) {
    let cols = Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)]).split(area);
    render_feed(f, app, cols[0], accent);
    let rail = Layout::vertical([
        Constraint::Percentage(30),
        Constraint::Percentage(30),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
    ])
    .split(cols[1]);
    render_reads(f, app, rail[0], accent);
    render_writes(f, app, rail[1], accent);
    render_hooks(f, app, rail[2], accent, PaneId::Hooks, false);
    render_skills(f, app, rail[3], accent);
}


fn pane_block(app: &App, pane: PaneId, title: String, accent: Color) -> Block<'static> {
    let focused = app.focus == pane;
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent));
    if focused {
        b = b.border_type(BorderType::Thick);
    }
    let mut style = Style::default().fg(accent);
    if focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    b.title(Span::styled(format!(" {title} "), style))
}

fn window(app: &mut App, pane: PaneId, total: usize, h: usize) -> (usize, usize) {
    let off = app.scroll.entry(pane).or_insert(0);
    let max_off = total.saturating_sub(h);
    if *off > max_off {
        *off = max_off;
    }
    let end = total - *off;
    let start = end.saturating_sub(h);
    (start, end)
}

fn inner_h(rect: Rect) -> usize {
    rect.height.saturating_sub(2) as usize
}

fn src_style(src: char) -> Style {
    match src {
        '$' => Style::default().fg(Color::Yellow),
        '@' => Style::default().fg(Color::LightCyan),
        '±' => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::DarkGray),
    }
}

fn feed_line<'a>(app: &'a App, it: &'a FeedItem) -> Line<'a> {
    let mut spans: Vec<Span> = vec![
        Span::styled(fmt_clock(it.ts), Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
    ];
    if let Some((tag, idx)) = app.agent_tag(&it.agent) {
        spans.push(Span::styled(tag, Style::default().fg(agent_color(idx))));
        spans.push(Span::raw(" "));
    }
    let style = match it.kind {
        FeedKind::Prompt => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        FeedKind::Reply => Style::default().fg(Color::Gray),
        FeedKind::Tool => Style::default(),
        FeedKind::Mcp => Style::default().fg(Color::Magenta),
        FeedKind::Skill => Style::default().fg(Color::LightBlue),
        FeedKind::Agent => Style::default().fg(Color::LightCyan),
        FeedKind::Info => Style::default().fg(Color::DarkGray),
        FeedKind::Warn => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    };
    spans.push(Span::styled(it.text.as_str(), style));
    match it.status {
        ToolStatus::Pending => spans.push(Span::styled(" ⋯", Style::default().fg(Color::Yellow))),
        ToolStatus::Ok => spans.push(Span::styled(" ✓", Style::default().fg(Color::Green))),
        ToolStatus::Err => spans.push(Span::styled(
            " ✗",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        ToolStatus::None => {}
    }
    Line::from(spans)
}

fn render_feed(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Feed, rect));
    let h = inner_h(rect);
    let idxs: Vec<usize> = (0..app.feed.len())
        .filter(|&i| app.agent_passes_filter(&app.feed[i].agent))
        .collect();
    let total = idxs.len();
    if app.search.target == PaneId::Feed && app.search.query.is_some() {
        let texts: Vec<String> =
            idxs.iter().map(|&i| app.feed[i].text.to_lowercase()).collect();
        engage_search(&mut app.search, &mut app.pending_jump, &texts);
        take_jump(&mut app.scroll, &app.search, &mut app.pending_jump, PaneId::Feed, total, h);
    }
    let (start, end) = window(app, PaneId::Feed, total, h);
    let mut lines: Vec<Line> =
        idxs[start..end].iter().map(|&i| feed_line(app, &app.feed[i])).collect();
    if app.search.target == PaneId::Feed {
        let cur = app.search.matches.get(app.search.cur).copied();
        highlight_matches(&mut lines, start, &app.search.matches, cur);
    }
    let mut title = format!("activity {total}");
    if !matches!(app.think_filter(), ThinkFilter::All) {
        title.push_str(&format!(" · {}", filter_label(app)));
    }
    title.push_str(&search_suffix(app, PaneId::Feed));
    let block = pane_block(app, PaneId::Feed, title, accent);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn render_reads(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Reads, rect));
    let h = inner_h(rect);
    let w = rect.width.saturating_sub(2) as usize;
    let total = app.reads.len();
    let (start, end) = window(app, PaneId::Reads, total, h);
    let mut lines: Vec<Line> = Vec::new();
    for r in &app.reads[start..end] {
        let mut spans: Vec<Span> = vec![
            Span::styled(fmt_clock(r.ts), Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
        ];
        let mut used = 9;
        if let Some((tag, idx)) = app.agent_tag(&r.agent) {
            used += tag.chars().count() + 1;
            spans.push(Span::styled(tag, Style::default().fg(agent_color(idx))));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!("{} ", r.src), src_style(r.src)));
        used += 2;
        let suffix = if r.count > 1 { format!(" ×{}", r.count) } else { String::new() };
        let err_w = if r.err { 2 } else { 0 };
        let pw = w.saturating_sub(used + suffix.chars().count() + err_w).max(8);
        let path_style = if r.err { Style::default().fg(Color::Red) } else { Style::default() };
        spans.push(Span::styled(tail_truncate(&r.path, pw), path_style));
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, Style::default().fg(Color::DarkGray)));
        }
        if r.err {
            spans.push(Span::styled(
                " ✗",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    let block = pane_block(app, PaneId::Reads, format!("reads {total}"), accent);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn render_writes(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Writes, rect));
    let h = inner_h(rect);
    let w = rect.width.saturating_sub(2) as usize;
    let total = app.writes.len();
    let (start, end) = window(app, PaneId::Writes, total, h);
    let mut lines: Vec<Line> = Vec::new();
    for e in &app.writes[start..end] {
        lines.push(write_line(app, e, w));
    }
    let block = pane_block(app, PaneId::Writes, format!("writes {total}"), accent);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn write_line<'a>(app: &'a App, e: &'a crate::app::WriteEntry, w: usize) -> Line<'a> {
    let mut spans: Vec<Span> = vec![
        Span::styled(fmt_clock(e.ts), Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
    ];
    let mut used = 9;
    if let Some((tag, idx)) = app.agent_tag(&e.agent) {
        used += tag.chars().count() + 1;
        spans.push(Span::styled(tag, Style::default().fg(agent_color(idx))));
        spans.push(Span::raw(" "));
    }
    let kind_style = if e.kind == 'W' {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
    };
    spans.push(Span::styled(e.kind.to_string(), kind_style));
    spans.push(Span::raw(" "));
    used += 2;
    let stat = match (e.adds, e.dels) {
        (Some(a), Some(d)) => format!(" +{a} −{d}"),
        _ => " ⋯".to_string(),
    };
    let pw = w.saturating_sub(used + stat.chars().count()).max(8);
    let path_style = if e.err {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };
    spans.push(Span::styled(tail_truncate(&e.path, pw), path_style));
    match (e.adds, e.dels) {
        (Some(a), Some(d)) => {
            spans.push(Span::styled(format!(" +{a}"), Style::default().fg(Color::Green)));
            spans.push(Span::styled(format!(" −{d}"), Style::default().fg(Color::Red)));
        }
        _ => spans.push(Span::styled(" ⋯", Style::default().fg(Color::Yellow))),
    }
    Line::from(spans)
}

fn event_short(event: &str) -> &str {
    match event {
        "PreToolUse" => "pre",
        "PostToolUse" => "post",
        "Stop" => "stop",
        "SubagentStop" => "sastop",
        "UserPromptSubmit" => "prompt",
        "SessionStart" => "start",
        "SessionEnd" => "end",
        "Notification" => "notif",
        "PreCompact" => "compact",
        e => e,
    }
}

fn render_hooks(
    f: &mut Frame,
    app: &mut App,
    rect: Rect,
    accent: Color,
    pane: PaneId,
    with_skills: bool,
) {
    app.pane_rects.push((pane, rect));
    let h = inner_h(rect);
    let mut lines: Vec<Line> = Vec::new();
    let mut matched: Vec<&str> = Vec::new();

    for c in &app.hooks_config {
        let stat = c
            .status_message
            .as_deref()
            .and_then(|sm| app.hook_stats.get_key_value(sm))
            .or_else(|| {
                app.hook_stats
                    .iter()
                    .find(|(k, _)| c.command.starts_with(k.trim_end_matches('…')) && !k.is_empty())
                    .map(|(k, v)| (k, v))
            });
        let (count, acted, avg) = match stat {
            Some((k, st)) => {
                matched.push(k.as_str());
                (st.count, st.acted, if st.count > 0 { st.total_ms / st.count } else { 0 })
            }
            None => (0, 0, 0),
        };
        // Only Stop-family hooks log their (passing) runs to the transcript;
        // for the rest a zero count means "unobservable", not "never ran".
        let runs_logged = matches!(c.event.as_str(), "Stop" | "SubagentStop");
        let active = count > 0 || acted > 0;
        let count_label = if count == 0 && !runs_logged {
            " ×–".to_string()
        } else {
            format!(" ×{count}")
        };
        let mut spans = vec![
            Span::styled(
                format!("{:<6}", event_short(&c.event)),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                c.name.clone(),
                if active { Style::default() } else { Style::default().fg(Color::DarkGray) },
            ),
            Span::styled(
                count_label,
                if count > 0 {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
        ];
        if count > 0 {
            spans.push(Span::styled(
                format!(" {avg}ms"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if acted > 0 {
            spans.push(Span::styled(
                format!(" ⚠{acted}"),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    for (cmd, st) in &app.hook_stats {
        if matched.contains(&cmd.as_str()) {
            continue;
        }
        let mut spans = vec![
            Span::styled("·     ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate_chars(cmd, 30)),
            Span::styled(
                format!(" ×{}", st.count),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ];
        if st.acted > 0 {
            spans.push(Span::styled(
                format!(" ⚠{}", st.acted),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    if !app.hook_actions.is_empty() {
        lines.push(Line::from(Span::styled(
            "── acted ──",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        let w = (rect.width.saturating_sub(4) as usize).max(10);
        for a in &app.hook_actions {
            let style = match a.sev {
                2 => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                _ => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            };
            lines.push(Line::from(vec![
                Span::styled(fmt_clock(a.ts), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(a.label.clone(), style),
            ]));
            // the response gets its own wrapped lines so it's actually readable
            if !a.detail.is_empty() {
                for wl in textwrap::wrap(&a.detail, w) {
                    lines.push(Line::from(Span::styled(
                        format!("  {wl}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
        }
    }
    if with_skills {
        lines.push(Line::from(Span::styled(
            "── skills ──",
            Style::default().fg(Color::LightBlue),
        )));
        lines.extend(skill_lines(app));
    }
    let total = lines.len();
    let (start, end) = window(app, pane, total, h);
    let visible = lines[start..end].to_vec();
    let acted_total: u64 = app.hook_stats.values().map(|s| s.acted).sum();
    let title = if acted_total > 0 {
        format!("hooks ⚠{acted_total}")
    } else {
        "hooks".to_string()
    };
    let title = if with_skills { format!("{title} + skills") } else { title };
    let block = pane_block(app, pane, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn skill_lines(app: &App) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for sk in &app.skills {
        out.push(Line::from(vec![
            Span::styled(fmt_clock(sk.last_ts), Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(
                sk.name.clone(),
                Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ×{}", sk.count), Style::default().fg(Color::White)),
        ]));
    }
    out
}

fn render_skills(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Skills, rect));
    let h = inner_h(rect);
    let lines = skill_lines(app);
    let total = lines.len();
    let (start, end) = window(app, PaneId::Skills, total, h);
    let visible = lines[start..end].to_vec();
    let block = pane_block(app, PaneId::Skills, format!("skills {total}"), accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn filter_label(app: &App) -> String {
    match app.think_filter() {
        ThinkFilter::All => "all".to_string(),
        ThinkFilter::Main => "main".to_string(),
        ThinkFilter::Agent(i) => match app.agent_by_idx(i) {
            Some(a) => {
                let m = if a.model.is_empty() { "?" } else { crate::price::model_short(&a.model) };
                let desc = if a.desc.is_empty() {
                    String::new()
                } else {
                    format!(" {}", truncate_chars(&a.desc, 30))
                };
                format!("{m}:{i}{desc}")
            }
            None => format!("?:{i}"),
        },
    }
}

fn render_thinking(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Thinking, rect));
    let h = inner_h(rect);
    let width = (rect.width.saturating_sub(2) as usize).max(10);
    let filter = app.think_filter();

    let key = (width, app.think_filter_pos, app.thinking.len(), app.agent_order.len());
    if app.think_cache_key != key {
        app.think_cache_key = key;
        let mut sel: Vec<(i64, Option<(String, usize)>, String)> = Vec::new();
        for t in &app.thinking {
            let tag = app.agent_tag(&t.agent);
            let keep = match filter {
                ThinkFilter::All => true,
                ThinkFilter::Main => t.agent.is_none(),
                ThinkFilter::Agent(i) => tag.as_ref().map(|(_, x)| *x == i).unwrap_or(false),
            };
            if keep {
                sel.push((t.ts, tag, t.text.clone()));
            }
        }
        app.think_lines.clear();
        for (ts, tag, text) in sel {
            let (label, idx) = match tag {
                Some((t, i)) => (t, Some(i)),
                None => ("[main]".to_string(), None),
            };
            app.think_lines.push(TLine {
                text: format!("── {} {} ──", fmt_clock(ts), label),
                agent_idx: idx,
                header: true,
            });
            for wline in textwrap::wrap(&text, width) {
                app.think_lines.push(TLine {
                    text: wline.into_owned(),
                    agent_idx: idx,
                    header: false,
                });
            }
        }
    }

    // search matches (only when this pane is the search target)
    if app.search.target == PaneId::Thinking {
        app.search.matches.clear();
        if let Some(q) = app.search.query.clone() {
            let ql = q.to_lowercase();
            for (i, l) in app.think_lines.iter().enumerate() {
                if !l.header && l.text.to_lowercase().contains(&ql) {
                    app.search.matches.push(i);
                }
            }
            if app.search.jump_pending {
                app.search.jump_pending = false;
                if !app.search.matches.is_empty() {
                    app.search.cur = app.search.matches.len() - 1;
                    app.pending_jump = Some(app.search.matches[app.search.cur]);
                }
            }
            if app.search.cur >= app.search.matches.len() {
                app.search.cur = app.search.matches.len().saturating_sub(1);
            }
        }
    }

    let total = app.think_lines.len();
    take_jump(&mut app.scroll, &app.search, &mut app.pending_jump, PaneId::Thinking, total, h);
    let (start, end) = window(app, PaneId::Thinking, total, h);

    let cur_match_line = app
        .search
        .matches
        .get(app.search.cur)
        .copied()
        .unwrap_or(usize::MAX);
    let query = if app.search.target == PaneId::Thinking {
        app.search.query.clone()
    } else {
        None
    };
    let mut lines: Vec<Line> = Vec::new();
    for (i, l) in app.think_lines[start..end].iter().enumerate() {
        let gi = start + i;
        if l.header {
            let color = l.agent_idx.map(agent_color).unwrap_or(Color::DarkGray);
            lines.push(Line::from(Span::styled(
                l.text.clone(),
                Style::default().fg(color).add_modifier(Modifier::DIM),
            )));
        } else {
            let base = Style::default().fg(Color::Gray);
            match &query {
                Some(q) => lines.push(highlight_line(&l.text, q, base, gi == cur_match_line)),
                None => lines.push(Line::from(Span::styled(l.text.clone(), base))),
            }
        }
    }

    let mut title = format!("narrative · {}", filter_label(app));
    title.push_str(&search_suffix(app, PaneId::Thinking));
    let block = pane_block(app, PaneId::Thinking, title, accent);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn highlight_line(text: &str, query: &str, base: Style, is_current: bool) -> Line<'static> {
    let lt = text.to_lowercase();
    let lq = query.to_lowercase();
    // Byte-offset math is only safe when lowering didn't change lengths.
    if lq.is_empty() || lt.len() != text.len() {
        return Line::from(Span::styled(text.to_string(), base));
    }
    let hl = if is_current {
        Style::default().fg(Color::Black).bg(Color::LightRed).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    };
    let mut spans: Vec<Span> = Vec::new();
    let mut pos = 0;
    while let Some(found) = lt[pos..].find(&lq) {
        let s = pos + found;
        let e = s + lq.len();
        if !text.is_char_boundary(s) || !text.is_char_boundary(e) {
            break;
        }
        if s > pos {
            spans.push(Span::styled(text[pos..s].to_string(), base));
        }
        spans.push(Span::styled(text[s..e].to_string(), hl));
        pos = e;
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_string(), base));
    }
    if spans.is_empty() {
        return Line::from(Span::styled(text.to_string(), base));
    }
    Line::from(spans)
}

fn wrap_into(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    for raw in text.lines() {
        if raw.trim().is_empty() {
            lines.push(Line::default());
            continue;
        }
        for wl in textwrap::wrap(raw, width) {
            lines.push(Line::from(Span::styled(wl.into_owned(), style)));
        }
    }
}

/// Minimal JSON syntax highlighting: keys cyan, string values green,
/// everything else dim. Applied per (pre-wrapped) line.
fn highlight_json_line(line: &str) -> Line<'static> {
    let punct = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);
    let strv = Style::default().fg(Color::Green);
    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            if !cur.is_empty() {
                spans.push(Span::styled(cur.clone(), punct));
                cur.clear();
            }
            let mut sbuf = String::from('"');
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                sbuf.push(d);
                if d == '\\' {
                    if i + 1 < chars.len() {
                        i += 1;
                        sbuf.push(chars[i]);
                    }
                } else if d == '"' {
                    break;
                }
                i += 1;
            }
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let is_key = chars.get(j) == Some(&':');
            spans.push(Span::styled(sbuf, if is_key { key } else { strv }));
        } else {
            cur.push(chars[i]);
        }
        i += 1;
    }
    if !cur.is_empty() {
        spans.push(Span::styled(cur, punct));
    }
    Line::from(spans)
}


/// Lightweight shell highlighting: command words bold, strings green,
/// flags cyan, operators magenta, variables yellow.
fn highlight_bash_line(line: &str) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    let mut expect_cmd = true;
    let mut in_quote: Option<char> = None;
    for (i, tok) in line.split_whitespace().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let st = if let Some(q) = in_quote {
            if tok.matches(q).count() % 2 == 1 {
                in_quote = None;
            }
            Style::default().fg(Color::Green)
        } else if matches!(tok, "|" | "||" | "&&" | ";" | ">" | ">>" | "<" | "<<" | "2>&1" | "&") {
            expect_cmd = true;
            spans.push(Span::styled(tok.to_string(), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)));
            continue;
        } else {
            for qc in ['\'', '"'] {
                if tok.matches(qc).count() % 2 == 1 {
                    in_quote = Some(qc);
                }
            }
            if tok.starts_with('#') {
                Style::default().fg(Color::DarkGray)
            } else if tok.starts_with('\'') || tok.starts_with('"') || in_quote.is_some() {
                Style::default().fg(Color::Green)
            } else if expect_cmd {
                expect_cmd = false;
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else if tok.starts_with('-') && tok.len() > 1 {
                Style::default().fg(Color::Cyan)
            } else if tok.starts_with('$') {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            }
        };
        spans.push(Span::styled(tok.to_string(), st));
    }
    Line::from(spans)
}

/// Wrap + highlight a tool payload: pretty-printed JSON gets colored,
/// plain text gets diff-aware styling. Never truncates.
fn payload_into(lines: &mut Vec<Line<'static>>, text: &str, width: usize, err: bool) {
    let trimmed = text.trim_start();
    let as_json = (trimmed.starts_with('{') || trimmed.starts_with('['))
        .then(|| serde_json::from_str::<serde_json::Value>(text).ok())
        .flatten()
        .and_then(|v| serde_json::to_string_pretty(&v).ok());
    if let Some(pretty) = as_json {
        for raw in pretty.lines() {
            for wl in textwrap::wrap(raw, width) {
                lines.push(highlight_json_line(&wl));
            }
        }
        return;
    }
    for raw in text.lines() {
        let style = if err {
            Style::default().fg(Color::LightRed)
        } else if raw.starts_with('+') {
            Style::default().fg(Color::Green)
        } else if raw.starts_with('-') {
            Style::default().fg(Color::Red)
        } else if raw.starts_with("error") || raw.contains("error[") {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if raw.starts_with("warning") {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Gray)
        };
        if raw.trim().is_empty() {
            lines.push(Line::default());
            continue;
        }
        for wl in textwrap::wrap(raw, width) {
            lines.push(Line::from(Span::styled(wl.into_owned(), style)));
        }
    }
}

fn render_memory(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Memory, rect));
    let h = inner_h(rect);
    let width = (rect.width.saturating_sub(2) as usize).max(10);
    let key = (width, app.mem_rev, 0);
    if app.mem_cache.key != key {
        app.mem_cache.key = key;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (name, content) in &app.memory_files {
            lines.push(Line::from(Span::styled(
                format!("── {name} ──"),
                Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            )));
            wrap_into(&mut lines, content, width, Style::default().fg(Color::Gray));
            lines.push(Line::default());
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "no memory files for this project",
                Style::default().fg(Color::DarkGray),
            )));
        }
        app.mem_cache.texts = lines.iter().map(line_text_lower).collect();
        app.mem_cache.lines = lines;
        // memory reads top-down: anchor to the top on rebuild
        app.scroll.insert(PaneId::Memory, usize::MAX / 2);
    }
    let total = app.mem_cache.lines.len();
    if app.search.target == PaneId::Memory && app.search.query.is_some() {
        engage_search(&mut app.search, &mut app.pending_jump, &app.mem_cache.texts);
        take_jump(&mut app.scroll, &app.search, &mut app.pending_jump, PaneId::Memory, total, h);
    }
    let (start, end) = window(app, PaneId::Memory, total, h);
    let mut visible = app.mem_cache.lines[start..end].to_vec();
    if app.search.target == PaneId::Memory {
        let cur = app.search.matches.get(app.search.cur).copied();
        highlight_matches(&mut visible, start, &app.search.matches, cur);
    }
    let title = format!(
        "memory · {} files{}",
        app.memory_files.len(),
        search_suffix(app, PaneId::Memory)
    );
    let block = pane_block(app, PaneId::Memory, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn render_context(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Context, rect));
    let h = inner_h(rect);
    let width = (rect.width.saturating_sub(2) as usize).max(10);
    let key = (width, app.ctx_rev, 0);
    if app.ctx_cache.key != key {
        app.ctx_cache.key = key;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for m in &app.ctx {
            match m.kind {
                CtxKind::User => {
                    lines.push(Line::from(Span::styled(
                        format!("── {} you ──", fmt_clock(m.ts)),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    )));
                    wrap_into(&mut lines, &m.text, width, Style::default());
                    lines.push(Line::default());
                }
                CtxKind::Assistant => {
                    lines.push(Line::from(Span::styled(
                        format!("── {} claude ──", fmt_clock(m.ts)),
                        Style::default().fg(Color::Green),
                    )));
                    wrap_into(&mut lines, &m.text, width, Style::default().fg(Color::Gray));
                    lines.push(Line::default());
                }
                CtxKind::Tool => {
                    lines.push(Line::from(vec![
                        Span::styled(fmt_clock(m.ts), Style::default().fg(Color::DarkGray)),
                        Span::raw(" "),
                        Span::styled(
                            truncate_chars(&m.text, width.saturating_sub(10)),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                CtxKind::Summary => {
                    lines.push(Line::from(Span::styled(
                        format!("══ {} compact summary ══", fmt_clock(m.ts)),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )));
                    wrap_into(
                        &mut lines,
                        &m.text,
                        width,
                        Style::default().fg(Color::Yellow),
                    );
                    lines.push(Line::default());
                }
                CtxKind::Boundary => {
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled(
                        format!("══════ {} ══════", m.text),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::default());
                }
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "no context yet",
                Style::default().fg(Color::DarkGray),
            )));
        }
        app.ctx_cache.texts = lines.iter().map(line_text_lower).collect();
        app.ctx_cache.lines = lines;
    }
    let total = app.ctx_cache.lines.len();
    if app.search.target == PaneId::Context && app.search.query.is_some() {
        engage_search(&mut app.search, &mut app.pending_jump, &app.ctx_cache.texts);
        take_jump(&mut app.scroll, &app.search, &mut app.pending_jump, PaneId::Context, total, h);
    }
    let (start, end) = window(app, PaneId::Context, total, h);
    let mut visible = app.ctx_cache.lines[start..end].to_vec();
    if app.search.target == PaneId::Context {
        let cur = app.search.matches.get(app.search.cur).copied();
        highlight_matches(&mut visible, start, &app.search.matches, cur);
    }
    let title = format!(
        "context · {} msgs · {}{}",
        app.ctx.len(),
        fmt_tok(app.ctx_tokens),
        search_suffix(app, PaneId::Context)
    );
    let block = pane_block(app, PaneId::Context, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn render_toolio(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::ToolIO, rect));
    let h = inner_h(rect);
    let width = (rect.width.saturating_sub(2) as usize).max(10);
    let key = (width, app.tio_rev, app.think_filter_pos);
    if app.tio_cache.key != key {
        app.tio_cache.key = key;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for io in &app.tool_ios {
            if !app.agent_passes_filter(&io.agent) {
                continue;
            }
            let (mark, mark_style) = match (&io.output, io.err) {
                (None, _) => ("⋯", Style::default().fg(Color::Yellow)),
                (_, true) => ("✗", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                (_, false) => ("✓", Style::default().fg(Color::Green)),
            };
            let mut header: Vec<Span> = vec![
                Span::styled(
                    format!("─ {} ", fmt_clock(io.ts)),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if let Some((tag, idx)) = app.agent_tag(&io.agent) {
                header.push(Span::styled(format!("{tag} "), Style::default().fg(agent_color(idx))));
            }
            header.push(Span::styled(
                io.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            header.push(Span::raw(" "));
            header.push(Span::styled(mark.to_string(), mark_style));
            if io.dur_ms > 1000 {
                header.push(Span::styled(
                    format!(" {}", fmt_dur(io.dur_ms)),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(header));
            if io.name == "Bash" {
                for raw in io.input.lines() {
                    if raw.trim().is_empty() {
                        lines.push(Line::default());
                        continue;
                    }
                    for wl in textwrap::wrap(raw, width) {
                        lines.push(highlight_bash_line(&wl));
                    }
                }
            } else {
                payload_into(&mut lines, &io.input, width, false);
            }
            match &io.output {
                None => lines.push(Line::from(Span::styled(
                    "  ⋯ awaiting result",
                    Style::default().fg(Color::Yellow),
                ))),
                Some(o) if o.is_empty() => lines.push(Line::from(Span::styled(
                    "  (empty result)",
                    Style::default().fg(Color::DarkGray),
                ))),
                Some(o) => {
                    lines.push(Line::from(Span::styled(
                        "  ↳ result",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    payload_into(&mut lines, o, width, io.err);
                }
            }
            lines.push(Line::default());
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "no tool calls yet",
                Style::default().fg(Color::DarkGray),
            )));
        }
        app.tio_cache.texts = lines.iter().map(line_text_lower).collect();
        app.tio_cache.lines = lines;
    }
    let total = app.tio_cache.lines.len();
    if app.search.target == PaneId::ToolIO && app.search.query.is_some() {
        engage_search(&mut app.search, &mut app.pending_jump, &app.tio_cache.texts);
        take_jump(&mut app.scroll, &app.search, &mut app.pending_jump, PaneId::ToolIO, total, h);
    }
    let (start, end) = window(app, PaneId::ToolIO, total, h);
    let mut visible = app.tio_cache.lines[start..end].to_vec();
    if app.search.target == PaneId::ToolIO {
        let cur = app.search.matches.get(app.search.cur).copied();
        highlight_matches(&mut visible, start, &app.search.matches, cur);
    }
    let shown = app
        .tool_ios
        .iter()
        .filter(|io| app.agent_passes_filter(&io.agent))
        .count();
    let mut title = format!("tool i/o · {shown} calls");
    if !matches!(app.think_filter(), ThinkFilter::All) {
        title.push_str(&format!(" · {}", filter_label(app)));
    }
    title.push_str(&search_suffix(app, PaneId::ToolIO));
    let block = pane_block(app, PaneId::ToolIO, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}


fn action_style(t: &str) -> Style {
    match t.chars().next().unwrap_or(' ') {
        '❯' => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        '▷' => Style::default().fg(Color::Gray),
        '⚑' => Style::default().fg(Color::LightCyan),
        '◇' => Style::default().fg(Color::Magenta),
        '◆' => Style::default().fg(Color::LightBlue),
        'R' => Style::default().fg(Color::DarkGray),
        'E' => Style::default().fg(Color::Blue),
        _ => Style::default(),
    }
}

fn render_overview(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Overview, rect));
    let h = inner_h(rect);
    let now = now_ms();
    let home = crate::discover::home().to_string_lossy().to_string();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (ci, o) in app.overview.iter().enumerate() {
        let selected = ci == app.overview_sel;
        let start = lines.len();
        let bar = if selected {
            Span::styled("▌", Style::default().fg(accent))
        } else {
            Span::raw(" ")
        };
        let (glyph, scolor, sname) = match o.state {
            crate::overview::SessState::Working => ("●", Color::Green, "working"),
            crate::overview::SessState::Waiting => ("⏸", Color::Yellow, "waiting"),
            crate::overview::SessState::Stalled => ("⚠", Color::Red, "stalled"),
            crate::overview::SessState::Idle => ("○", Color::DarkGray, "idle"),
        };
        let dot = Span::styled(
            format!("{glyph} "),
            Style::default().fg(scolor).add_modifier(Modifier::BOLD),
        );
        let mut folder = o.cwd.clone();
        if let Some(rest) = folder.strip_prefix(&home) {
            folder = format!("~{rest}");
        }
        let mut head: Vec<Span> = vec![
            bar.clone(),
            dot,
            Span::styled(
                truncate_chars(&folder, 44),
                if selected {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                },
            ),
        ];
        if !o.branch.is_empty() {
            head.push(Span::styled(
                format!(" ⎇ {}", truncate_chars(&o.branch, 24)),
                Style::default().fg(Color::Cyan),
            ));
        }
        if !o.title.is_empty() {
            head.push(Span::styled(
                format!("  {}", o.title.clone()),
                Style::default().fg(Color::Gray),
            ));
        }
        head.push(Span::styled("  · ", Style::default().fg(Color::DarkGray)));
        head.push(Span::styled(sname, Style::default().fg(scolor)));
        head.push(Span::styled(
            format!(
                " {} · {}",
                fmt_dur(now - o.mtime_ms),
                crate::price::model_short(&o.model)
            ),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::from(head));
        for (ts, act) in &o.actions {
            lines.push(Line::from(vec![
                bar.clone(),
                Span::styled(
                    format!("  {} ", fmt_clock(*ts)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(act.clone(), action_style(act)),
            ]));
        }
        lines.push(Line::from(bar.clone()));
        ranges.push((start, lines.len()));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no non-trivial sessions active in the last 30 minutes",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let total = lines.len();
    let (sel_s, sel_e) = ranges
        .get(app.overview_sel)
        .copied()
        .unwrap_or((0, total.min(1)));
    let mut top = app.overview_top.min(total.saturating_sub(h));
    if sel_s < top {
        top = sel_s;
    }
    if sel_e > top + h {
        top = sel_e.saturating_sub(h);
    }
    app.overview_top = top;
    app.overview_ranges = ranges;
    let end = (top + h).min(total);
    let visible = lines[top..end].to_vec();
    let title = format!(
        "sessions · active last 30m · {} found · ↑↓ select · ⏎ open",
        app.overview.len()
    );
    let block = pane_block(app, PaneId::Overview, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn haunt_run_line(r: &crate::haunt::HauntRun, now: i64) -> Line<'static> {
    let (mark, st) = if r.running {
        ("●", Style::default().fg(Color::Yellow))
    } else if r.ok {
        ("✓", Style::default().fg(Color::Green))
    } else {
        ("✗", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    };
    Line::from(vec![
        Span::styled(format!("  {mark} "), st),
        Span::styled(r.label.clone(), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" · {}", r.what), Style::default().fg(Color::Gray)),
        Span::styled(
            format!(" · {} · {} ago", r.status, fmt_dur(now - r.created_ms)),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// "4h" / "16h" / "10d" — how a window reads in a section header.
fn win_label(ms: i64) -> String {
    let hours = ms / 3_600_000;
    if hours >= 24 && hours % 24 == 0 {
        format!("{}d", hours / 24)
    } else {
        format!("{hours}h")
    }
}

/// "· newest 10", or nothing when the section is uncapped.
fn keep_label(keep: usize) -> String {
    if keep == usize::MAX {
        String::new()
    } else {
        format!(" · newest {keep}")
    }
}

fn render_github(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::GitHub, rect));
    let h = inner_h(rect);
    let now = now_ms();
    let mode = app.gh_mode;
    let gp = mode.gh_params();
    let hp = mode.haunt_params();
    let hdr = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
        ))
    };
    let none_line = || {
        Line::from(Span::styled(
            "  · none".to_string(),
            Style::default().fg(Color::DarkGray),
        ))
    };
    let err_line = |e: &str| {
        Line::from(Span::styled(
            format!("  ⚠ {}", truncate_chars(e, 110)),
            Style::default().fg(Color::Red),
        ))
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    let v = app.ghv();
    if v.gh.fetching && v.gh.fetched_at_ms == 0 {
        lines.push(Line::from(Span::styled(
            "⋯ fetching github / roadmap / sites…",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::default());
    }

    lines.push(hdr(&format!(
        "── github · workflow runs · last {}{} ──",
        win_label(gp.run_window_ms),
        keep_label(gp.keep_runs)
    )));
    if let Some(e) = &v.gh.error {
        lines.push(err_line(e));
    }
    if v.gh.runs.is_empty() && v.gh.error.is_none() {
        lines.push(none_line());
    }
    for r in &v.gh.runs {
        let running = r.status != "completed";
        let (mark, st) = if running {
            ("●", Style::default().fg(Color::Yellow))
        } else {
            match r.conclusion.as_str() {
                "success" => ("✓", Style::default().fg(Color::Green)),
                "failure" => ("✗", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                "cancelled" => ("⊘", Style::default().fg(Color::DarkGray)),
                _ => ("·", Style::default().fg(Color::Gray)),
            }
        };
        let state = if running { r.status.as_str() } else { r.conclusion.as_str() };
        let tail = if running {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {mark} "), st),
            Span::styled(r.repo.clone(), Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" · {} ⎇ {}", r.workflow, r.branch),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!(" · {} · {} ago", state, fmt_dur(now - r.created_ms)),
                tail,
            ),
        ]));
    }
    lines.push(Line::default());

    lines.push(hdr(&format!(
        "── github · prs · last {}{} ──",
        win_label(gp.pr_window_ms),
        keep_label(gp.keep_prs)
    )));
    if v.gh.prs.is_empty() {
        lines.push(none_line());
    }
    for p in &v.gh.prs {
        let (mark, st) = if p.draft {
            ("◌", Style::default().fg(Color::DarkGray))
        } else {
            match p.state.to_ascii_uppercase().as_str() {
                "OPEN" => ("○", Style::default().fg(Color::Green)),
                "MERGED" => ("⇄", Style::default().fg(Color::Magenta)),
                // closed-unmerged is not a failure, just done with
                _ => ("⊘", Style::default().fg(Color::DarkGray)),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {mark} "), st),
            Span::styled(
                format!("{}#{}", p.repo, p.number),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", truncate_chars(&p.title, 70)),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!(" · {} · {} ago", p.author, fmt_dur(now - p.created_ms)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::default());

    lines.push(hdr(&format!(
        "── roadmap · delivery runs · last {}{} ──",
        win_label(hp.window_ms),
        keep_label(hp.keep_per_source)
    )));
    if let Some(e) = &v.haunt.roadmap_err {
        lines.push(err_line(e));
    } else {
        let rs: Vec<_> = v.haunt.runs.iter().filter(|r| r.source == "roadmap").collect();
        if rs.is_empty() {
            lines.push(none_line());
        }
        for r in rs {
            lines.push(haunt_run_line(r, now));
        }
    }
    lines.push(Line::default());

    lines.push(hdr(&format!(
        "── sites · maintenance runs · last {}{} ──",
        win_label(hp.window_ms),
        keep_label(hp.keep_per_source)
    )));
    if let Some(e) = &v.haunt.sites_err {
        lines.push(err_line(e));
    } else {
        let rs: Vec<_> = v.haunt.runs.iter().filter(|r| r.source == "sites").collect();
        if rs.is_empty() {
            lines.push(none_line());
        }
        for r in rs {
            lines.push(haunt_run_line(r, now));
        }
    }

    // a refresh keeps the timestamp visible — dropping it made a pane holding
    // perfectly good data look like it had been reset
    let updated = match (v.gh.fetching, v.gh.fetched_at_ms) {
        (true, 0) => "fetching…".to_string(),
        (true, at) => format!("updated {} ago · refreshing…", fmt_dur(now - at)),
        (false, 0) => String::new(),
        (false, at) => format!("updated {} ago", fmt_dur(now - at)),
    };
    let total = lines.len();
    let (start, end) = window(app, PaneId::GitHub, total, h);
    let visible = lines[start..end].to_vec();
    let label = match mode {
        GhMode::Live => "live",
        GhMode::Digest => "10d digest",
    };
    let title = format!("github + haunt · {label} · {updated}");
    let block = pane_block(app, PaneId::GitHub, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

/// Dependabot across the sites registry. Rolled up per advisory rather than
/// listed per alert: the same CVE in fourteen sites is one fix, not fourteen.
fn render_cve(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Cve, rect));
    let h = inner_h(rect);
    let now = now_ms();
    let hdr = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
        ))
    };
    let c = &app.cve;
    let mut lines: Vec<Line<'static>> = Vec::new();

    if c.fetching && c.fetched_at_ms == 0 {
        lines.push(Line::from(Span::styled(
            format!("⋯ scanning {} managed sites…", c.sites_scanned.max(40)),
            Style::default().fg(Color::Yellow),
        )));
    }
    if let Some(e) = &c.error {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {}", truncate_chars(e, 110)),
            Style::default().fg(Color::Red),
        )));
    }
    if c.fetched_at_ms > 0 {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} critical", c.critical),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" · {} high", c.total.saturating_sub(c.critical)),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!(
                    " · {} distinct CVEs · {}/{} sites affected",
                    c.distinct, c.sites_affected, c.sites_scanned
                ),
                Style::default().fg(Color::Gray),
            ),
        ]));
    }
    lines.push(Line::default());

    lines.push(hdr("── worst CVEs · critical first, then blast radius ──"));
    if c.worst.is_empty() && c.fetched_at_ms > 0 {
        lines.push(Line::from(Span::styled(
            "  · none".to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    for r in &c.worst {
        let (mark, st) = if r.severity == "critical" {
            ("✗", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        } else {
            ("▲", Style::default().fg(Color::Yellow))
        };
        // an unscored advisory is not a zero-risk one — say so rather than
        // printing a 0.0 that reads as "harmless"
        let score = if r.cvss > 0.0 {
            format!("{:>4.1}", r.cvss)
        } else {
            "   —".to_string()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {mark} "), st),
            Span::styled(
                format!("{:<18}", truncate_chars(&r.id, 18)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {score} "), Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:<22}", truncate_chars(&r.package, 22)),
                Style::default().fg(Color::LightCyan),
            ),
            Span::styled(
                format!(" {:>2} sites", r.sites),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(
                format!(" · open {}", fmt_dur(now - r.oldest_ms)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::default());

    lines.push(hdr("── worst sites ──"));
    for s in &c.by_site {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<26}", truncate_chars(&s.repo, 26)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:>4} critical", s.critical),
                Style::default().fg(Color::Red),
            ),
            Span::styled(
                format!(" · {:>4} high", s.high),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    let updated = match (c.fetching, c.fetched_at_ms) {
        (true, 0) => "scanning…".to_string(),
        (true, at) => format!("scanned {} ago · rescanning…", fmt_dur(now - at)),
        (false, 0) => String::new(),
        (false, at) => format!("scanned {} ago", fmt_dur(now - at)),
    };
    let total = lines.len();
    let (start, end) = window(app, PaneId::Cve, total, h);
    let visible = lines[start..end].to_vec();
    let block = pane_block(
        app,
        PaneId::Cve,
        format!("security · dependabot · managed sites · {updated}"),
        accent,
    );
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn status_bar(f: &mut Frame, app: &mut App, rect: Rect, status: Status, _accent: Color) {
    if let Some(input) = &app.search.input {
        let line = Line::from(vec![
            Span::styled(
                format!(" search {}: ", pane_label(app.search.target)),
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(input.clone()),
            Span::styled("▏", Style::default().fg(Color::Cyan)),
            Span::styled(
                "  enter=go · esc=cancel · n/N=next/prev",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(line), rect);
        return;
    }

    let now = now_ms();
    if app.sessions.is_empty() {
        let line = Line::from(Span::styled(
            " no claude sessions found for this folder — waiting… ",
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        f.render_widget(Paragraph::new(line), rect);
        return;
    }

    let (badge, badge_style, bar_style) = match status {
        Status::Working => (
            format!(" ⚡ WORKING {} ", fmt_dur(now - app.turn_start_ts)),
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD),
            Style::default(),
        ),
        Status::Waiting => {
            let since = if app.last_turn_end_ts > 0 {
                format!("{} ", fmt_dur(now - app.last_turn_end_ts))
            } else {
                String::new()
            };
            (
                format!(" ⏸ WAITING FOR INPUT {since}"),
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Black).bg(Color::Yellow),
            )
        }
        Status::Blocked => {
            let why = if app.has_pending_tools() { " — permission?" } else { "" };
            let idle = app.last_activity.elapsed().as_millis() as i64;
            (
                format!(" ⚠ STALLED{why} {} ", fmt_dur(idle)),
                Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::White).bg(Color::Red),
            )
        }
    };

    let sref = &app.sessions[app.sel];
    let mut label = if app.title.is_empty() {
        truncate_chars(&sref.id, 8)
    } else {
        truncate_chars(&app.title, 28)
    };
    if let Some(wt) = &sref.worktree {
        label.push_str(&format!(" ⎇ {}", truncate_chars(wt, 16)));
    }
    let t = app.totals();
    let win = app.effective_window();
    let pct = if win > 0 {
        ((app.ctx_tokens as f64 / win as f64) * 100.0).min(100.0)
    } else {
        0.0
    };
    let filled = ((pct / 100.0) * 8.0).round() as usize;
    let bar: String = "▓".repeat(filled.min(8)) + &"░".repeat(8 - filled.min(8));
    let model = if app.model.is_empty() {
        "?".to_string()
    } else {
        crate::price::model_short(&app.model).to_string()
    };

    let dim = if bar_style.bg.is_some() {
        bar_style
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let norm = if bar_style.bg.is_some() { bar_style } else { Style::default() };

    let spans = vec![
        Span::styled(badge, badge_style),
        Span::styled(
            format!(" {label} {}/{} · {model}", app.sel + 1, app.sessions.len()),
            norm,
        ),
        Span::styled(" │ ", dim),
        Span::styled(
            format!(
                "in {} · out {} · cache {}",
                fmt_tok(t.input + t.c5m + t.c1h),
                fmt_tok(t.output),
                fmt_tok(t.cache_read)
            ),
            norm,
        ),
        Span::styled(" │ ", dim),
        Span::styled(
            format!("NZ${:.2}", app.cost_usd() * app.nzd_rate),
            norm.add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", dim),
        Span::styled(
            format!("ctx {:.0}% ({}/{}) {bar}", pct, fmt_tok(app.ctx_tokens), fmt_tok(win)),
            norm,
        ),
        Span::styled(" │ ", dim),
        Span::styled(concat!("0-6·g · ⇥ session · <> agents · / find · q · v", env!("CARGO_PKG_VERSION")), dim),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar_style), rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn draws_all_layouts_without_panicking() {
        // nonexistent cwd → zero sessions → empty-state rendering
        let mut app = App::new(
            std::path::PathBuf::from("/nonexistent/claude-watch-test"),
            1.68,
            200_000,
        );
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        for layout in [1u8, 2, 3] {
            app.layout = layout;
            term.draw(|f| draw(f, &mut app)).unwrap();
        }
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("activity"));
        assert!(content.contains("no claude sessions found"));
    }
}
