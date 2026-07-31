# claude-watch

A live terminal dashboard for [Claude Code](https://claude.com/claude-code) sessions. Run it in any project folder and it tails the most recent session's transcript — including every subagent — and shows you what's actually going on.

```
┌ activity ──────────────────────┬ reads ─────────────────────┐
│ 13:12:01 $ cargo build ✓       │ src/main.rs                │
│ 13:12:04 [sa:fable:1] ▸ grep … │ src/session.rs             │
│ 13:12:09 ⚑ agent: audit deps ⋯ ├ writes ────────────────────┤
│ 13:12:11 ◇ haunt·run {...} ✓   │ src/ui.rs      +42 −7      │
│                                ├ hooks ─────────────────────┤
│                                │ stop  lazy-stop ×12 58ms   │
│                                │ ── acted ──                │
│                                │ 13:02 ✋ hook cancelled     │
│                                ├ skills ────────────────────┤
│                                │ /model ×1                  │
├ narrative ─────────────────────┴────────────────────────────┤
│ ── 13:12:01 [main] ──                                       │
│ The tests fail because the offset is computed before…       │
├─────────────────────────────────────────────────────────────┤
│ ⚡ WORKING 2m14s │ in 45.2k · out 3.1k │ NZ$3.42 │ ctx 38%   │
└─────────────────────────────────────────────────────────────┘
```

## What it shows

- **activity** — chronological feed of prompts, replies, Bash commands, MCP calls, web fetches, and agent spawns, with success/failure markers and durations. Subagent activity is merged in with colored `[sa:model:n]` tags.
- **reads / writes** — file paths as they're touched; writes carry `+adds −dels` diffstats.
- **hooks** — every configured hook with run counts and average duration, plus a sticky **acted** buffer for the moments a hook actually intervened (blocked a stop, errored, injected context, got cancelled).
- **skills** — slash commands and Skill invocations, sticky for the whole session.
- **narrative** — the assistant's full prose, main agent and subagents interleaved, scrollable and searchable. (Thinking text is never persisted to disk by Claude Code — every thinking block is stored as an empty string plus signature — so prose is the closest persisted layer. If a future version persists thinking, it will appear here automatically.)
- **status bar** — token totals, session cost in NZD at API rates, and a context-window gauge.

The whole frame changes color with session state: **green** working, **amber** waiting for your input, **red** stalled (usually a permission prompt).

## Install

```bash
./install.sh
```

## Use

```bash
cd some-project        # a folder with a Claude Code session
claude-watch
```

| key | action |
|-----|--------|
| `1` `2` `3` | switch layout (feed-major / narrative-major / grid) |
| `Tab` / `Shift-Tab` | cycle sessions for this folder (worktree sessions included) |
| `<` / `>` | narrative filter: all → main → each subagent |
| `/` then `n` / `N` | search the narrative, jump between matches |
| arrows / PgUp / PgDn / End | scroll focused pane |
| mouse | wheel scrolls the pane under the cursor; click focuses |
| `q` | quit |

Flags: `--nzd-rate 1.72` · `--context-window 500000` · `--session <id-prefix>` · `--dump` (parse and print, no TUI)

## How it works

Claude Code writes session transcripts to `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`, with subagent transcripts in `<session-id>/subagents/agent-*.jsonl`. claude-watch tails these (plus any git-worktree variants of the cwd), merges entries by timestamp, and derives everything else — no hooks, no wrappers, no interference with the session itself.
