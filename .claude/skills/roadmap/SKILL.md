---
description: Use when working on a project that has a roadmap on roadmaps.haunt.digital — both for reading/updating the roadmap directly AND for keeping it current while you do delivery work. Trigger phrases include "push the briefing", "what's on the roadmap for X", "mark story Y as ready", "answer question N", "add a question", "kick off a project", "pull the roadmap", "jot down a note", "log a run", "start work on story X", "what should I work on next". Also load this whenever you are about to start substantial development work in a repo that has a roadmap — the roadmap is where that work gets recorded.
---

# Roadmap

The `roadmap` CLI reads and writes projects on https://roadmaps.haunt.digital — briefings, plans, stories with estimates, questions, todos, milestones, QA, notes, runs and scope snapshots.

There are two distinct jobs here, and the second one is the one most often skipped:

1. **Roadmap work** — pushing a briefing, elaborating stories, answering questions, adjusting estimates. The roadmap is the artefact.
2. **Delivery work with roadmap upkeep** — you're building the thing, and the roadmap should end the session reflecting what actually happened. The code is the artefact; the roadmap is the record.

**If you are doing (2), the roadmap is not optional bookkeeping you do if asked.** A roadmap that says `not_started` for work that shipped last week is worse than no roadmap — the people reading it (project owner, client, the next agent) make decisions from it. Keep it true as you go.

## First: is this repo roadmapped?

Before substantial work in any repo, spend one command finding out. Cheapest signal first:

```bash
ls roadmap/.roadmap                     # a local clone? the file holds the slug + generation
roadmap projects                        # otherwise: match the repo against the project list
```

- **A `./roadmap/` folder exists** → this repo is roadmapped and you have a local clone. **Get it current before you read it**: `roadmap pull` (or `roadmap status` first if there are uncommitted local edits you'd rather not lose). A stale clone is how you end up building a story someone already rewrote.
- **No local folder, but a project matches** → the repo is roadmapped remotely. Confirm the match with the user before writing anything (*"This looks like `acme-client-portal` on the roadmap — right?"*) — **don't guess a slug**. Then either `roadmap clone <slug>` if you're going to do substantial work (the roadmap sits alongside the code, and you can edit story bodies as files), or just use `--slug <slug>` for a handful of targeted updates.
- **Nothing matches** → say so once, briefly, and carry on with the task. Don't nag, and don't create a project uninvited.

## Permission: what to just do, and what to ask

Keeping the roadmap true is part of the work, not a separate favour to ask for. **Do these unprompted, without checking in:** moving story delivery statuses, opening and closing runs, filing questions / todos / QA items / notes, setting a story or epic branch, correcting an estimate you now know is wrong.

**Ask first** only for: creating a project (`init` / `brief`), rewriting a briefing body, setting `scoping_status: ready` on someone else's draft, deleting anything, and the scope/branch/landing questions below.

## Keeping a roadmap current as you work

Everything below is a one-line command — see `references/cli.md` for exact syntax, and `references/delivery.md` for the full playbook including status vocabularies.

**Before you start substantial work** — sync the roadmap (above), then ask the user the handful of things you can't safely assume: scope, branch, where it lands. `references/delivery.md` has the full pre-flight; don't skip it because the task looks small — a wrong branch or unclear scope is paid for later by someone else.

**As you work:**

- **Move story delivery status.** `not_started` → `in_progress` when you actually begin; → `in_review` when the PR is up; → `done` when merged. `blocked` when you're stuck on something external, paired with a question or todo saying what would unblock it.
- **Open a run for substantial work** — a feature, an epic, a QA sweep, a migration. Open it *when you start* (`roadmap run new --mode build --stories a,b --in-progress`) so it shows as live, and close it with a written summary when you're done (`roadmap run complete <id> --from -`). A run is the durable answer to "what happened in that session, against which stories, and where did it land?" Skip it for one-line fixes and roadmap-only edits.
- **File what you discover, don't just mention it in chat.** Chat is lost at the end of the session; the roadmap isn't.
  - Something's ambiguous and you need a human decision → `question new`
  - Something must happen before UAT / go-live but isn't this story → `todo new --category …`
  - You found a defect or regression → `qa new`
  - You made a judgment call worth remembering → `note new`
  - Scope grew beyond the story → a new story, or `story split`
- **Keep estimates honest.** If a story took 12h against a 4h estimate, say so — update the estimate or leave a note. Silent overruns destroy the value of the estimate history.
- **Record where the code lives.** Set the story's branch (`story edit <slug> --branch …`) or the epic's branch (`tag branch <epic> …`) so the roadmap points at the work.

**When you finish:** close the run with a summary that names what shipped, what didn't, and anything the next person needs to know. Include the PR URL — it gets appended to each story the run touched automatically.

## Reaching for the reference files

Load these only when you need them — don't read them speculatively.

- **`references/cli.md`** — the full command reference: ad-hoc reads, ad-hoc writes, run commands, the local clone / push / pull workflow, story frontmatter, install/auth. Read it before running a command whose exact flags you're unsure of.
- **`references/delivery.md`** — the delivery playbook: the questions to ask before starting work, branch naming conventions, status vocabularies and what each value means, run modes, and worked examples of a session's worth of roadmap upkeep.

## Targeting a project

Almost every command takes `--slug <slug>`, or infers the project when run from inside a local roadmap folder:

```bash
roadmap projects                        # list projects you can access
roadmap --slug <slug> overview          # stats: stories by status, open questions, estimates, owner
```

If you don't know the slug, run `roadmap projects` and match on name — don't guess. If a repo has a `./roadmap/` folder, commands run from the repo root pick it up automatically.

## Rules

- **Confirm project names with the user** before `init` or `brief` — they create remote records.
- **Briefings are historical context.** Once a project is elaborated, don't rewrite `project-briefing.md`'s body — stakeholders own it via the web UI. Structured frontmatter fields (budget link, SOW status) are fine to update.
- **Never `roadmap push --force`.** It overwrites other people's changes. If push is rejected, pull and reconcile.
- **Don't invent stories, estimates or answers.** A question you can't answer stays open; an estimate you can't justify stays unset.
- **`roadmap execute` no longer exists.** It used to orchestrate Claude through build → quality → integration phases; an agent harness with skills and subagents does that better. Drive the work yourself and record it with `roadmap run`.
- If the CLI reports auth issues, run `roadmap login` (browser flow).
- "Tags" in the web UI == `groups` in the CLI and frontmatter. Keep using `--group` / `--groups`. An **Epic** is an exclusive tag.
