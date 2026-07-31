# `roadmap` CLI reference

Full command surface. `roadmap` with no arguments prints the same thing from the installed binary — if something here looks stale, trust the binary.

Every command takes `--slug <slug>` to target a project, or infers it from the local `./roadmap/` folder. All read commands take `--json` for parseable output. Write commands that take content accept `--from <file>` or `--from -` for stdin.

## Install / auth

```bash
curl -fsSL https://roadmaps.haunt.digital/install.sh | bash
roadmap login          # browser flow
roadmap update         # pull the latest binary (also auto-updates hourly)
roadmap projects       # verify auth by listing what you can see
```

## Creating projects

```bash
roadmap init <slug>                          # new project + local ./roadmap/ scaffold
roadmap brief "<Project Name>" briefing.md   # remote-only project from a briefing file
cat briefing.md | roadmap brief "<Project Name>"
roadmap clone <slug> [dir]                   # pull an existing project into a local folder
```

`brief` accepts optional structured fields, which round-trip via briefing frontmatter:

```bash
roadmap brief "<Project Name>" briefing.md \
  --budget-link https://app.productive.io/budget \
  --board-link https://app.productive.io/board \
  --sow-link https://drive.google.com/... \
  --sow-status signed_off
```

**Confirm the project name with the user before running `init` or `brief`** — both create remote records.

## Ad-hoc reads

```bash
roadmap projects                                  # all projects you can access
roadmap --slug <slug> overview                    # stories by status, open questions, estimates, owner
roadmap --slug <slug> owner                       # the project owner's email, or none
roadmap --slug <slug> briefing                    # briefing markdown
roadmap --slug <slug> dev-context                 # env quirks, no-go zones — read before building
roadmap --slug <slug> plan                        # plan markdown
roadmap --slug <slug> stories [--scoping-status ready] [--delivery-status in_progress] [--group X]
roadmap --slug <slug> story <story-slug>          # content, statuses, branch, estimates
roadmap --slug <slug> estimates [--ai]            # cash roll-up: hours x rate + multipliers + fixed price
roadmap --slug <slug> estimate-config             # hourly rate, % multipliers (PM/QA), fixed-price items
roadmap --slug <slug> questions [--status open|answered]
roadmap --slug <slug> question <id>
roadmap --slug <slug> todos [--status pending|in_progress|done] [--category X] [--story <slug>] [--group X]
roadmap --slug <slug> todo <id>
roadmap --slug <slug> qa [--status pending|done] [--story <slug>] [--run <id>]
roadmap --slug <slug> qa <id>
roadmap --slug <slug> milestones                  # delivery timeline (dated first, undated last)
roadmap --slug <slug> milestone <id>
roadmap --slug <slug> notes [--search "term"]     # free-form journal, newest first
roadmap --slug <slug> note <id>
roadmap --slug <slug> runs                        # delivery work log, newest first
roadmap --slug <slug> run <id>
roadmap --slug <slug> timeline [--limit N] [--type X]   # change log: who changed what, via which surface
roadmap --slug <slug> snapshots
roadmap --slug <slug> snapshot show <id|name> [--ai]
roadmap --slug <slug> snapshot story <id|name> <story-slug>
roadmap --slug <slug> snapshot diff <id|name>     # what's changed since the snapshot
```

## Ad-hoc writes

Each write bumps the project's generation, exactly like a push. Run from inside a local clone (or with `--dir`) and the matching local file is synced too.

### Docs

```bash
roadmap --slug <slug> briefing edit --from ./brief.md
roadmap --slug <slug> briefing set sow_status signed_off
#   fields: productive_budget_link | productive_board_link | sow_link |
#           sow_status (not_required|in_progress|with_client|signed_off) |
#           github_repo_link | notion_link
echo "..." | roadmap --slug <slug> plan edit --from -
roadmap --slug <slug> dev-context edit --from ./dev-notes.md
roadmap --slug <slug> owner set person@haunt.digital     # or: owner clear
```

### Stories

```bash
roadmap --slug <slug> story new <story-slug> [--from ./body.md] [--scoping-status draft]
roadmap --slug <slug> story content <story-slug> --from ./body.md
roadmap --slug <slug> story edit <story-slug> --scoping-status ready       # draft | ready
roadmap --slug <slug> story edit <story-slug> --delivery-status in_progress
#   delivery: not_started | in_progress | in_review | done | blocked
roadmap --slug <slug> story edit <story-slug> --branch feature/auth        # or --clear-branch
roadmap --slug <slug> story edit <story-slug> --estimate 5 --low 3 --high 8 --confidence 7
roadmap --slug <slug> story edit <story-slug> --groups epic-a,spike
roadmap --slug <slug> story split <parent> <child-a> <child-b>
roadmap --slug <slug> tag branch <epic> feature/foo                       # or --clear
```

### Estimates

```bash
roadmap --slug <slug> estimate-config set-rate 250
roadmap --slug <slug> estimate-config set --from ./cfg.json
#   JSON: { hourly_rate, multipliers[], fixed_price_items[] }
```

### Questions, todos, QA, notes, milestones

```bash
roadmap --slug <slug> question new "Why X?" [--story <story-slug>]
roadmap --slug <slug> question answer <id> "Because Y."
roadmap --slug <slug> question reopen <id>

# --category is REQUIRED: blocking-story | before-uat | before-go-live |
#                         after-go-live | production-setup
roadmap --slug <slug> todo new --category before-go-live --content "Set up 301 redirects" [--story <slug>] [--group X]
roadmap --slug <slug> todo edit <id> [--status done|in_progress|pending] [--content "..."] [--category X]
roadmap --slug <slug> todo done <id>
roadmap --slug <slug> todo delete <id>

roadmap --slug <slug> qa new --content "Invite email unstyled in Outlook" [--story <slug>] [--run <id>]
roadmap --slug <slug> qa edit <id> [--status pending|done] [--content "..."]
roadmap --slug <slug> qa done <id>
roadmap --slug <slug> qa delete <id>

roadmap --slug <slug> note new --content "Decided to defer SSO to phase 2."
echo "..." | roadmap --slug <slug> note new --from -
roadmap --slug <slug> note edit <id> --content "Revised thought."
roadmap --slug <slug> note delete <id>

roadmap --slug <slug> milestone new --name "UAT starts" [--date 2026-08-01] [--category Deployment] [--description "..."]
#   date optional (undated sort last); category is flexible — reuse an existing
#   one or type a new one. Seeds: Internal, Document, Deployment, Workshop
roadmap --slug <slug> milestone edit <id> [--name "..."] [--date YYYY-MM-DD | --clear-date] [--category X]
roadmap --slug <slug> milestone delete <id>
```

### Runs

The log of substantial delivery work. See `delivery.md` for when and how to use these — this is just the syntax.

```bash
roadmap --slug <slug> run new [--mode build|qa|fix] [--stories a,b,c] \
                              [--integration-branch X] [--deploy-branch X] [--in-progress]
roadmap --slug <slug> run status <id> <pending|in_progress|blocked|done|failed>
roadmap --slug <slug> run complete <id> --from ./summary.md [--pr <url>] [--no-append-pr]
roadmap --slug <slug> run delete <id> [--yes]
```

`run complete` reads `status:` and `pr:` from the summary's frontmatter (`--pr` overrides). When a PR URL is present it's appended to each story the run touched as a `## Run #N → <url>` footnote — idempotent, so re-running is safe.

> `roadmap execute` — the old phase orchestrator that drove Claude through build → quality → integration — **has been removed.** Drive the work with your own agent harness; record it with `run`.

### Snapshots

Immutable point-in-time copies of scope (briefing, plan, stories, epics, estimates) — "what the client signed off". Creating one does **not** bump generation. Names are required and unique per project.

```bash
roadmap --slug <slug> snapshot create "Sent to ACME"
```

Deleting a snapshot is deliberately hard — admin-API-key only, not exposed on the CLI.

## Local clone workflow

For substantial roadmap work — elaborating many stories, restructuring the plan, working through a pile of questions in one sitting. Usually done in the project's own repo so the roadmap sits beside the code.

```bash
roadmap clone <slug>      # creates ./roadmap/ and pulls
# edit ./roadmap/plan.md, ./roadmap/stories/*.md, ./roadmap/questions.md
roadmap status            # local vs remote diff
roadmap push              # send local changes
roadmap pull              # fetch remote changes
```

Folder layout: `project-briefing.md` (with frontmatter), `plan.md`, `questions.md`, `developer-context.md`, `todos.md`, `stories/*.md`, `images/`.

Story frontmatter:

- `scoping_status` — `draft | ready` (canonical; replaces the legacy `status:`)
- `delivery_status` — `not_started | in_progress | in_review | done | blocked`
- `branch` — optional; an epic's branch overrides at runtime
- `estimate`, `estimate_low`, `estimate_high`, `confidence`, `ai`
- `groups` — the Tags/Epics concept
- `split_from`

A legacy `status:` key is still parsed on push for backwards compatibility, but pull always writes `scoping_status:`.

**Conflicts:** if someone else pushed since your last pull, `push` is rejected and the CLI offers a menu (stash, commit, back up, or destructively pull over). **Never `--force`** — it overwrites others' work.

## Installing the agent bundle

The skill you're reading, plus the unrestricted-mode hooks and pi extensions, are served from the platform. To install into the current directory (`./.claude/` and `./.pi/`):

```bash
roadmap agent      # aliases: roadmap claude, roadmap pi
```

Scope is just the current directory — no subtree walk. Bundle files are always overwritten so you stay current; `.claude/settings.json` has its `hooks` block replaced wholesale while other keys (`permissions`, `$schema`) are preserved.
