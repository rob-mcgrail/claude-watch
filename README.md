# claude-watch

A live terminal dashboard for [Claude Code](https://claude.com/claude-code) sessions. Run it in any project folder and it tails the most recent session's transcript — including every subagent — and shows you what's actually going on.

![main view: narrative, reads/writes/hooks/skills rail, activity feed](screenshots/view-1-main.png)

## Install

macOS (Apple Silicon), one command — downloads the release binary, clears the quarantine flag, installs to `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/rob-mcgrail/claude-watch/master/install-macos.sh | bash
```

Or from source (needs Rust): `git clone https://github.com/rob-mcgrail/claude-watch && cd claude-watch && ./install.sh`

## Use

```bash
cd some-project        # a folder with a Claude Code session
claude-watch
```

The whole frame changes color with session state: **green** working, **amber** waiting for your input, **red** stalled (usually a permission prompt).

## The views

Number keys switch views — plus `0` for the machine-wide session switcher and `g` for external activity.

`0` **sessions** — every non-trivial session on the machine with activity in the last 30 minutes: folder, branch, title, age, model, and the last five actions. Arrow keys (or mouse) select, `Enter` jumps into that session in the main view — repointing the whole watcher, worktrees and hooks config included.

`g` **github + haunt** — external activity, fetched on a background thread from your authenticated CLIs. Two modes over the same sources, each with its own cache so switching is instant:

- `g` **live** — what is happening right now: GitHub workflows in flight plus runs from the last 4 hours (across your recently-pushed repos), PRs from the last 6 hours, and roadmap/sites delivery runs from the last 16 hours. Auto-refreshes every 2 minutes while on screen.
- `Space` **10-day digest** — what has been happening lately: the newest 10 workflow runs, the newest 10 PRs (ranked open → merged → closed), and the newest 5 delivery runs each from roadmaps.haunt.digital and sites.haunt.digital, all over the last 10 days.

`Space` is a round trip: press it to peek at the digest, press it again to drop back into whatever view you came from. Both modes are warmed on startup and the one you are not looking at is kept warm while the pane is open, so switching either way renders instantly from data already in hand — never a reload. Anything still in flight sorts to the top of its section. Each source degrades gracefully to a one-line notice if its CLI is missing or unauthenticated.

`s` **security** — every open critical and high Dependabot alert across the sites registry, rolled up per advisory. The org-wide endpoint returns 17,000+ alerts, most of them against throwaway repos, so the scope is `sites list` — the 40 sites Haunt actually maintains. Ranked critical first, then by blast radius: the same CVE in fourteen sites is one row and one fix, not fourteen. `<` and `>` filter by ecosystem (all / npm / composer / rubygems — built from whatever the scan finds). Below that, the worst sites by critical count, and the most recent pushes to each site's `deploy-production` branch. A full scan is ~15 seconds across 40 repos; it is preloaded at startup, cached for 30 minutes and shared between instances, and pressing `s` again while it is showing forces a rescan.

`m` **sites** — the registry as a patching work queue: least recently patched first, with the facts that decide whether a stale site is urgent — stack and runtime versions, runtime EOL (flagged red once passed), proactive SLA, and whether it has tests. Same contract as `s`: preloaded, cached, and `m` again reloads it.

`1` **main** (above) — narrative pane, reads/writes/hooks/skills rail, full-width activity feed.

`2` **ops** — activity + rail, no narrative:

![ops view](screenshots/view-2-ops.png)

`3` **activity** — just the feed, full screen (shown here in amber waiting-for-input state):

![activity view](screenshots/view-3-activity.png)

`4` **tool i/o** — every Bash/MCP/agent call with **full, untruncated** input and result, JSON pretty-printed and highlighted:

![tool i/o view](screenshots/view-6-tool-io.png)

`5` **context** — the session's context window as a scrollable document: prompts and replies in full, tool one-liners, compaction boundaries with pre→post token counts, and injected compact summaries:

![context view](screenshots/view-5-context.png)

`6` **memory** — the project's memory files, live-reloading:

![memory view](screenshots/view-4-memory.png)

## What it shows

- **activity** — chronological feed of prompts, replies, Bash commands, MCP calls, web fetches, and agent spawns, with success/failure markers and durations. Subagent activity is merged in with colored `[model:n]` tags.
- **reads / writes** — file paths as content enters or leaves context. Reads carry a source marker: `R` Read tool, `$` shell command (`cat`/`head`/`tail`/`jq`/`grep`/`rg`, parsed quote-aware), `@` user-attached file, `±` re-read after an external edit. Writes carry `+adds −dels` diffstats. Both scroll back through the whole session.
- **hooks** — every configured hook with run counts and average duration (`×–` where Claude Code doesn't log runs), plus a sticky **acted** buffer for the moments a hook actually intervened — blocked a tool call, blocked a stop, errored, injected context — with the hook's full response.
- **skills** — slash commands and Skill invocations, sticky for the whole session.
- **narrative** — the assistant's full prose, main agent and subagents interleaved, scrollable and searchable, filterable per-agent. (Thinking text is never persisted to disk by Claude Code — every thinking block is stored as an empty string plus signature — so prose is the closest persisted layer. If a future version persists thinking, it will appear here automatically.)
- **status bar** — token totals, session cost in NZD at API rates, and a context-window gauge (window size is estimated per model — Fable sessions observably run 1M — and auto-adjusts if usage exceeds it).

## Keys

| key | action |
|-----|--------|
| `0`–`6`, `g` | switch view (`0` sessions machine-wide, `g` github + haunt live) |
| `Space` | peek at the github + haunt 10-day digest; press again to go back |
| `s` | security: open critical/high CVEs across the managed sites (`<`/`>` filter by ecosystem) |
| `m` | sites registry, least recently patched first |
| `Tab` / `Shift-Tab` | cycle sessions for this folder (worktree sessions included) |
| `↑` `↓` `Enter` | on view `0`: select a session and open it |
| `<` / `>` | agent filter: all → main → each subagent (applies to narrative, activity, and tool i/o) |
| `/` then `n` / `N` | search the focused pane (activity, narrative, memory, context, tool i/o), jump between matches |
| arrows / PgUp / PgDn / End | scroll focused pane |
| mouse | wheel scrolls the pane under the cursor; click focuses |
| `q` | quit |

Flags: `--nzd-rate 1.72` · `--context-window 500000` · `--session <id-prefix>` · `--dump` (parse and print, no TUI) · `--overview` (print the machine-wide session scan) · `--version`

## How it works

Claude Code writes session transcripts to `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`, with subagent transcripts in `<session-id>/subagents/agent-*.jsonl`. claude-watch tails these (plus any git-worktree variants of the cwd), merges entries by timestamp, and derives everything else — no hooks, no wrappers, no interference with the session itself.
