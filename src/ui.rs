use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{
    fmt_clock, fmt_dur, fmt_tok, now_ms, tail_truncate, truncate_chars, App, CtxKind, FeedItem,
    FeedKind, PaneId, Status, TLine, ThinkFilter, ToolStatus,
};

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
        2 => layout_ops(f, app, rows[0], accent),
        3 => render_feed(f, app, rows[0], accent),
        4 => render_memory(f, app, rows[0], accent),
        5 => render_context(f, app, rows[0], accent),
        6 => render_toolio(f, app, rows[0], accent),
        _ => layout_default(f, app, rows[0], accent),
    }
    status_bar(f, app, rows[1], status, accent);
}

fn layout_default(f: &mut Frame, app: &mut App, area: Rect, accent: Color) {
    let main = Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)]).split(area);
    let top = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(main[0]);
    render_thinking(f, app, top[0], accent);
    let rail = Layout::vertical([
        Constraint::Percentage(26),
        Constraint::Percentage(26),
        Constraint::Percentage(28),
        Constraint::Percentage(20),
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
        Constraint::Percentage(26),
        Constraint::Percentage(26),
        Constraint::Percentage(28),
        Constraint::Percentage(20),
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
    let total = app.feed.len();
    let (start, end) = window(app, PaneId::Feed, total, h);
    let lines: Vec<Line> = app.feed[start..end].iter().map(|it| feed_line(app, it)).collect();
    let block = pane_block(app, PaneId::Feed, format!("activity {total}"), accent);
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
                format!("sa:{m}:{i}{desc}")
            }
            None => format!("sa:?:{i}"),
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

    // search matches
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

    let total = app.think_lines.len();
    if let Some(idx) = app.pending_jump.take() {
        let off = total
            .saturating_sub(idx + h / 2 + 1)
            .min(total.saturating_sub(h));
        app.scroll.insert(PaneId::Thinking, off);
    }
    let (start, end) = window(app, PaneId::Thinking, total, h);

    let cur_match_line = app
        .search
        .matches
        .get(app.search.cur)
        .copied()
        .unwrap_or(usize::MAX);
    let query = app.search.query.clone();
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
    if let Some(q) = &app.search.query {
        if app.search.matches.is_empty() {
            title.push_str(&format!(" · \"{q}\" no matches"));
        } else {
            title.push_str(&format!(
                " · \"{q}\" {}/{}",
                app.search.cur + 1,
                app.search.matches.len()
            ));
        }
    }
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
    let key = (width, app.mem_rev);
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
        app.mem_cache.lines = lines;
        // memory reads top-down: anchor to the top on rebuild
        app.scroll.insert(PaneId::Memory, usize::MAX / 2);
    }
    let total = app.mem_cache.lines.len();
    let (start, end) = window(app, PaneId::Memory, total, h);
    let visible = app.mem_cache.lines[start..end].to_vec();
    let title = format!("memory · {} files", app.memory_files.len());
    let block = pane_block(app, PaneId::Memory, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn render_context(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::Context, rect));
    let h = inner_h(rect);
    let width = (rect.width.saturating_sub(2) as usize).max(10);
    let key = (width, app.ctx_rev);
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
        app.ctx_cache.lines = lines;
    }
    let total = app.ctx_cache.lines.len();
    let (start, end) = window(app, PaneId::Context, total, h);
    let visible = app.ctx_cache.lines[start..end].to_vec();
    let title = format!(
        "context · {} msgs · {}",
        app.ctx.len(),
        fmt_tok(app.ctx_tokens)
    );
    let block = pane_block(app, PaneId::Context, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn render_toolio(f: &mut Frame, app: &mut App, rect: Rect, accent: Color) {
    app.pane_rects.push((PaneId::ToolIO, rect));
    let h = inner_h(rect);
    let width = (rect.width.saturating_sub(2) as usize).max(10);
    let key = (width, app.tio_rev);
    if app.tio_cache.key != key {
        app.tio_cache.key = key;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for io in &app.tool_ios {
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
            payload_into(&mut lines, &io.input, width, false);
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
        app.tio_cache.lines = lines;
    }
    let total = app.tio_cache.lines.len();
    let (start, end) = window(app, PaneId::ToolIO, total, h);
    let visible = app.tio_cache.lines[start..end].to_vec();
    let title = format!("tool i/o · {} calls", app.tool_ios.len());
    let block = pane_block(app, PaneId::ToolIO, title, accent);
    f.render_widget(Paragraph::new(visible).block(block), rect);
}

fn status_bar(f: &mut Frame, app: &mut App, rect: Rect, status: Status, _accent: Color) {
    if let Some(input) = &app.search.input {
        let line = Line::from(vec![
            Span::styled(
                " search: ",
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
        label.push_str(&format!(" ⎇{}", truncate_chars(wt, 16)));
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
        Span::styled("1-6 views · ⇥ session · <> think · / find · q", dim),
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
