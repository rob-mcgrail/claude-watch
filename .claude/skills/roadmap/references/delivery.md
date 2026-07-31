# Delivery playbook — keeping the roadmap true while you build

Read this when you're doing development work against a project that has a roadmap. For command syntax see `cli.md`.

## Before you start

There used to be an interactive wizard (`roadmap execute`) that walked a human through these questions before any build began. It's gone — **you ask them now.** Don't skip them because the task looks small; the cost of a wrong branch or unclear scope is paid later by someone else.

Gather what you can yourself first, then ask only what's genuinely undecided. Ask in one batch, not one question at a time.

**0. Is this repo roadmapped, and is the roadmap current?**

Do this before anything else — it's one command, and it decides whether the rest of this file applies.

```bash
ls roadmap/.roadmap        # local clone marker: contains `slug:` and the `generation:` you last synced
```

If it's there, you have a clone. **Sync it before you trust it:**

```bash
roadmap status             # local vs remote diff — run this first if there are local edits
roadmap pull               # fast-forward to the server's version
```

`status` is the safe opener: if someone else has pushed since this clone was last synced, `pull` may overwrite local edits, and `status` tells you that before you lose anything. If the clone has local changes that *should* go up, push them (`roadmap push`) before starting new work — don't leave a half-synced clone behind you. Never `--force`.

If there's no local folder, find out whether the project exists remotely:

```bash
roadmap projects           # match on slug / name against this repo
```

`github_repo_link` lives in the briefing's frontmatter rather than the project list, so you can't match a git remote in one call — match on name, then **confirm with the user before writing**: *"This repo looks like `acme-client-portal` on the roadmap — that the right project?"* Guessing a slug means writing someone else's roadmap.

Once confirmed, pick a working mode:

| Situation | Do this |
|---|---|
| Substantial work — several stories, story bodies to edit | `roadmap clone <slug>` — the roadmap lands in `./roadmap/` next to the code |
| A few targeted updates (statuses, a run, a question) | Skip the clone; use `--slug <slug>` on each command |

If no project matches, say so once and get on with the task. A repo without a roadmap is normal; don't create one uninvited.

**1. What's in scope?**

Check the roadmap before asking:

```bash
roadmap --slug <slug> stories --scoping-status ready --delivery-status not_started
roadmap --slug <slug> questions --status open
roadmap --slug <slug> qa                       # pending QA feedback
```

Then confirm: *"I'm planning to pick up `story-a` and `story-b` — both ready and not started. Sound right?"* Flag anything odd rather than working around it: a story that's `draft` isn't ready to build (the spec isn't settled); a story already `in_progress` may be someone else's work; open questions attached to your stories may block you.

**2. What branch?**

Propose a name, don't ask an open question. Check for an existing branch first — the story or its epic may already have one:

```bash
roadmap --slug <slug> story <story-slug>       # shows branch / effective_branch
```

If there's an effective branch, use it. If not, propose one using the house prefixes:

| Prefix | For |
|---|---|
| `feature/` | New functionality |
| `support/` | Refactors, tooling, docs, chores, internal improvements |
| `fix/` | Bug fixes |
| `experiment/` | Spikes and throwaway exploration |
| `integration/` | The branch several feature branches fold into, and which opens one PR |

Ask like this: *"I'll work on `feature/client-portal-auth` — good, or do you want it somewhere else?"* Keep the name short, kebab-case, and descriptive of the work rather than the ticket.

**3. Where does it land?**

- **One branch, one PR** — the common case. Nothing more to decide.
- **Several branches** — you need an integration branch (e.g. `integration/sprint-4`) that they all merge into, so the work opens a *single* PR rather than five. Check whether one already exists before minting a new one: `roadmap --slug <slug> runs` and look at recent runs' `int=` values. Non-first runs usually extend the previous run's integration branch, reusing its PR.
- **Is there a deploy branch?** Some projects push integration into a deploy branch (e.g. `deploy-staging`) to trigger a UAT/staging deploy — no PR, the push is the trigger. Check recent runs; if one is set, ask whether this work should also land there.

**4. Anything the roadmap doesn't tell you?**

Read the developer context — it's where env quirks and no-go zones live, and it exists precisely so you don't have to rediscover them:

```bash
roadmap --slug <slug> dev-context
roadmap --slug <slug> plan
```

## Opening a run

Open the run *before* the work, not after — while it's `in_progress` the web Runs tab shows it as live, and a run that's never opened is a run that never gets logged.

```bash
roadmap --slug <slug> run new --mode build \
  --stories client-portal-auth,client-portal-shell \
  --integration-branch integration/sprint-4 \
  --in-progress
```

**Modes:**

- **`build`** — new work against stories. The default.
- **`qa`** — working through QA feedback / fixing defects on existing branches.
- **`fix`** — a one-off targeted fix that isn't really story work.

**What deserves a run:** a feature, an epic, a QA sweep, a migration, a spike with findings worth keeping — anything you'd want to explain in a standup. **What doesn't:** typo fixes, dependency bumps, roadmap-only edits, anything under ~30 minutes.

If you're unsure, open one. An extra run record costs nothing; an unrecorded week of work costs a lot.

## While you work

Move story statuses as reality changes, not in a batch at the end (a session that dies halfway leaves the roadmap lying).

**`delivery_status`** — where the code is:

| Value | Means |
|---|---|
| `not_started` | Nothing built |
| `in_progress` | Actively being built |
| `in_review` | PR is up, awaiting review |
| `done` | Merged |
| `blocked` | Can't proceed — always pair with a question or todo saying why |

**`scoping_status`** — whether the *spec* is settled: `draft` (still being written) → `ready` (safe to build). Don't set `ready` yourself unless you did the elaboration and are confident the story is unambiguous.

```bash
roadmap --slug <slug> story edit client-portal-auth --delivery-status in_progress
roadmap --slug <slug> story edit client-portal-auth --branch feature/client-portal-auth
```

Then, as things surface:

```bash
# A decision only a human can make
roadmap --slug <slug> question new "Should expired invites be re-sendable, or re-created?" --story client-portal-auth

# Must happen before go-live, but isn't this story
roadmap --slug <slug> todo new --category before-go-live --content "Rotate the staging SMTP credentials" --story client-portal-auth

# A defect you found (or caused)
roadmap --slug <slug> qa new --content "Invite email renders unstyled in Outlook" --story client-portal-auth

# A judgment call worth remembering in six months
roadmap --slug <slug> note new --content "Used signed URLs rather than session cookies for invite links — the portal is served from a different subdomain and cookie scoping got ugly."
```

Todo categories are required and meaningful — they're *when*, not *what*: `blocking-story` | `before-uat` | `before-go-live` | `after-go-live` | `production-setup`.

## Closing the run

Write the summary as markdown, with frontmatter carrying the status and PR URL:

```bash
cat <<'EOF' | roadmap --slug <slug> run complete 42 --from -
---
status: done
pr: https://github.com/haunt/client-portal/pull/118
---

Built invite-based auth for the client portal across both stories.

**Shipped**
- Invite issue + redeem flow, signed-URL based (see note #14 for why not cookies)
- Portal shell with role-gated nav

**Not done**
- Outlook email styling — filed as QA #31
- Rate limiting on redeem — filed as todo #22 (before-go-live)

**For the next person**
- `INVITE_SIGNING_KEY` must be set in staging before this deploys; it's in 1Password.
EOF
```

The PR URL is appended to each story the run touched as a `## Run #N → <url>` footnote, so a reader can navigate story → change. It's idempotent — re-running is safe. Pass `--no-append-pr` to opt out.

Then land the story statuses:

```bash
roadmap --slug <slug> story edit client-portal-auth --delivery-status in_review
roadmap --slug <slug> story edit client-portal-shell --delivery-status in_review
```

A good summary is honest about what didn't get done. "Shipped everything" when two things are filed as QA items is the failure mode that makes runs worthless.

## A note on scope creep

If the work turns out bigger than the story described, don't silently absorb it. Either:

- **Split the story** — `roadmap --slug <slug> story split <parent> <child-a> <child-b>` marks the parent split and stubs the children, or
- **Write a new story** for the extra surface and tell the user the estimate has moved.

Quietly building 3x the scope means the estimate history lies, and the next estimate for similar work is wrong.
