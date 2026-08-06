mod app;
mod discover;
mod gcache;
mod gh;
mod cve;
mod haunt;
mod sitelist;
mod overview;
mod price;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::Position;
use ratatui::Terminal;

use app::{App, GhMode, PaneId};

fn main() -> io::Result<()> {
    let mut nzd_rate = 1.68f64;
    let mut ctx_window = 200_000u64;
    let mut dump = false;
    let mut session: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--nzd-rate" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    nzd_rate = v;
                }
            }
            "--context-window" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    ctx_window = v;
                }
            }
            "--dump" => dump = true,
            "--overview" => {
                for o in overview::scan(30 * 60 * 1000) {
                    let st = match o.state {
                        overview::SessState::Working => "WORKING",
                        overview::SessState::Waiting => "WAITING",
                        overview::SessState::Stalled => "STALLED",
                        overview::SessState::Idle => "idle",
                    };
                    println!(
                        "[{:<7}] {} ⎇ {} · {} · {}",
                        st, o.cwd, o.branch, o.title, o.model
                    );
                    for (_, a) in &o.actions {
                        println!("    {a}");
                    }
                }
                return Ok(());
            }
            "--session" => session = args.next(),
            "-V" | "--version" => {
                println!("claude-watch {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!(
                    "claude-watch — live dashboard for Claude Code sessions in this folder\n\n\
                     usage: claude-watch [--nzd-rate N] [--context-window N] [--session ID-PREFIX]\n\
                                        [--dump] [--overview] [--version]\n\n\
                     keys: 0 sessions machine-wide · 1-6 views (1 main · 2 ops · 3 activity · 4 tool i/o\n\
                           · 5 context · 6 memory) · g github+haunt (space toggles digest) · s security · m sites\n\
                           tab/shift-tab sessions · </> agent filter (narrative, activity, tool i/o)\n\
                           / search focused pane · n/N matches · arrows/pgup/pgdn scroll\n\
                           mouse: wheel scrolls pane under cursor, click focuses · q quit"
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let cwd = std::env::current_dir()?;
    let mut app = App::new(cwd, nzd_rate, ctx_window);
    if let Some(prefix) = &session {
        app.select_session_by_prefix(prefix);
    }

    if dump {
        app.dump();
        return Ok(());
    }

    // warm both g-view modes in the background so either is populated on
    // first visit and switching between them costs nothing
    app.gh_boot();
    app.cve_boot();
    app.sites_boot();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        orig_hook(info);
    }));

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let res = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    res
}

fn run<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if handle_key(app, k) {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => handle_mouse(app, m),
                _ => {}
            }
        }
        if last_tick.elapsed() >= Duration::from_millis(200) {
            app.tick();
            last_tick = Instant::now();
        }
    }
}

fn scroll_by(app: &mut App, pane: PaneId, delta: i64) {
    let off = app.scroll.entry(pane).or_insert(0);
    if delta >= 0 {
        // upward: clamped against content length at render time
        *off = off.saturating_add(delta as usize);
    } else {
        *off = off.saturating_sub((-delta) as usize);
    }
}

fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    // search input mode captures everything
    if app.search.input.is_some() {
        match k.code {
            KeyCode::Esc => app.search.input = None,
            KeyCode::Enter => {
                let q = app.search.input.take().unwrap_or_default();
                if q.is_empty() {
                    app.search.query = None;
                } else {
                    app.search.query = Some(q);
                    app.search.jump_pending = true;
                }
            }
            KeyCode::Backspace => {
                if let Some(inp) = &mut app.search.input {
                    inp.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(inp) = &mut app.search.input {
                    inp.push(c);
                }
            }
            _ => {}
        }
        return false;
    }

    if app.layout == 0 {
        match k.code {
            KeyCode::Up => {
                app.overview_move(-1);
                return false;
            }
            KeyCode::Down => {
                app.overview_move(1);
                return false;
            }
            KeyCode::PageUp => {
                app.overview_move(-5);
                return false;
            }
            KeyCode::PageDown => {
                app.overview_move(5);
                return false;
            }
            KeyCode::Enter => {
                app.jump_to_selected();
                return false;
            }
            _ => {}
        }
    }

    match k.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Tab => app.next_session(1),
        KeyCode::BackTab => app.next_session(-1),
        KeyCode::Char('1') => app.layout = 1,
        KeyCode::Char('2') => app.layout = 2,
        KeyCode::Char('3') => {
            app.layout = 3;
            app.focus = PaneId::Feed;
        }
        KeyCode::Char('4') => {
            app.layout = 4;
            app.focus = PaneId::ToolIO;
        }
        KeyCode::Char('5') => {
            app.layout = 5;
            app.focus = PaneId::Context;
        }
        KeyCode::Char('6') => {
            app.layout = 6;
            app.focus = PaneId::Memory;
            app.load_memory();
        }
        KeyCode::Char('0') => {
            app.layout = 0;
            app.focus = PaneId::Overview;
            app.refresh_overview();
        }
        KeyCode::Char('g') => app.gh_open(GhMode::Live),
        KeyCode::Char('s') => app.cve_open(),
        KeyCode::Char('m') => app.sites_open(),
        // space toggles the g view's mode; it does not open the view
        KeyCode::Char(' ') => app.gh_toggle_digest(),
        // on the security view these cycle the ecosystem filter instead
        KeyCode::Char('<') | KeyCode::Char(',') => {
            if app.layout == 8 {
                app.cycle_cve_eco(-1)
            } else {
                app.cycle_think_filter(-1)
            }
        }
        KeyCode::Char('>') | KeyCode::Char('.') => {
            if app.layout == 8 {
                app.cycle_cve_eco(1)
            } else {
                app.cycle_think_filter(1)
            }
        }
        KeyCode::Char('/') => {
            let searchable = matches!(
                app.focus,
                PaneId::Feed | PaneId::Thinking | PaneId::Memory | PaneId::Context | PaneId::ToolIO
            );
            let target = if searchable {
                app.focus
            } else {
                match app.layout {
                    4 => PaneId::ToolIO,
                    5 => PaneId::Context,
                    6 => PaneId::Memory,
                    _ => PaneId::Feed,
                }
            };
            if app.search.target != target {
                app.search.query = None;
                app.search.matches.clear();
                app.search.cur = 0;
            }
            app.search.target = target;
            app.search.input = Some(String::new());
            app.focus = target;
        }
        KeyCode::Esc => {
            app.search.query = None;
        }
        KeyCode::Char('n') => {
            if !app.search.matches.is_empty() {
                app.search.cur = (app.search.cur + 1) % app.search.matches.len();
                app.pending_jump = Some(app.search.matches[app.search.cur]);
                app.focus = app.search.target;
            }
        }
        KeyCode::Char('N') => {
            if !app.search.matches.is_empty() {
                let n = app.search.matches.len();
                app.search.cur = (app.search.cur + n - 1) % n;
                app.pending_jump = Some(app.search.matches[app.search.cur]);
                app.focus = app.search.target;
            }
        }
        KeyCode::Up => scroll_by(app, app.focus, 1),
        KeyCode::Down => scroll_by(app, app.focus, -1),
        KeyCode::PageUp => scroll_by(app, app.focus, 10),
        KeyCode::PageDown => scroll_by(app, app.focus, -10),
        KeyCode::Home => scroll_by(app, app.focus, i64::MAX / 2),
        KeyCode::End | KeyCode::Char('G') => {
            app.scroll.insert(app.focus, 0);
        }
        _ => {}
    }
    false
}

fn pane_at(app: &App, x: u16, y: u16) -> Option<PaneId> {
    app.pane_rects
        .iter()
        .find(|(_, r)| r.contains(Position::new(x, y)))
        .map(|(p, _)| *p)
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    if app.layout == 0 {
        match m.kind {
            MouseEventKind::ScrollUp => {
                app.overview_move(-1);
                return;
            }
            MouseEventKind::ScrollDown => {
                app.overview_move(1);
                return;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((_, r)) = app
                    .pane_rects
                    .iter()
                    .find(|(p, _)| *p == PaneId::Overview)
                {
                    if r.contains(Position::new(m.column, m.row)) {
                        let line =
                            app.overview_top + (m.row.saturating_sub(r.y + 1)) as usize;
                        if let Some(ci) = app
                            .overview_ranges
                            .iter()
                            .position(|(s, e)| line >= *s && line < *e)
                        {
                            app.overview_sel = ci;
                        }
                    }
                }
                return;
            }
            _ => {}
        }
    }
    match m.kind {
        MouseEventKind::ScrollUp => {
            if let Some(p) = pane_at(app, m.column, m.row) {
                scroll_by(app, p, 3);
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(p) = pane_at(app, m.column, m.row) {
                scroll_by(app, p, -3);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(p) = pane_at(app, m.column, m.row) {
                app.focus = p;
            }
        }
        _ => {}
    }
}
