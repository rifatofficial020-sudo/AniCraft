# ADR 0125: The roadmap as a guideline: a parity matrix, a prioritised backlog, and a slot for every pull request

* Status: accepted (2026-11-20)
* Relates to: `docs/roadmap.md`, `docs/parity.md`, `docs/release.md`, `docs/adr/README.md`
  (reserved numbers), AGENTS.md rules 3 to 8 and 33 to 37, `standards/claims.md`,
  `standards/pull-requests.md` and `standards/clean-room.md` in storytold/craft-contrib

## Context

The roadmap was a timeline: nine milestones, M0 to M8, each with exit criteria, and a table of
parallel tracks saying which milestone's task came next. That worked while the product was
being built up. Three things have changed.

* **The milestones are done.** Every row of the milestone table is ticked or carries a "not
  yet" list. The lists are the real plan now, and they were spread over cells thousands of
  characters long.
* **The work arrives from outside.** On 2026-11-20 there were 27 open pull requests that
  were not ours and about a hundred open issues. Several pull requests had waited two weeks
  with no word on whether they would merge. Some fixed code that had since moved to
  storytold/craft-libs, some had been overtaken by first-party work, and nobody had told their
  authors. A timeline of our own milestones has no place to put them.
* **The person set a new direction:** bug fixes, user feedback and outside contributions come
  first, and above all full feature parity with the reference app (Adobe Animate). The roadmap
  is to be a guideline, not a strict timeline. Every open outside pull request is to have a
  specific spot, so that its fate is never uncertain. Work is ordered by priority, correctness,
  completeness, reliability, robustness and independence. Every decision, first-party included,
  is questioned as a principal engineer would.

There was also no one place that said what parity means. "Coming from Animate" in the guide
lists what an artist will miss, and had fallen behind the code.

## Decision

### 1. Parity is written down, feature by feature

`docs/parity.md` lists the reference app's features by area (workspace, tools, drawing model,
colour, text, symbols, timeline, tweens, bones, camera, layers, filters, sound, video,
interactivity, publishing, import, assets, accessibility, preferences, automation), each with a
status: **done**, **partial** (what is missing), **missing**, or **won't do** (why), and
**ours** for things only AniCraft has.

* It is clean-room: written in our own words from public documentation and general knowledge,
  with no copied wording, screenshots or assets.
* Status is checked in the code, the command reference and the guide. A row we could not check
  twice is marked, not guessed.
* **A PR that changes a feature's status changes its row in the same PR.** This replaces "tick
  the roadmap line" for feature work.
* "Won't do" is a decision, and is limited to proprietary formats and services and to targets
  their maker has retired. Anything else that is absent is "missing" and gets a backlog item,
  however far down.

### 2. The roadmap is a prioritised backlog with no dates

`docs/roadmap.md` keeps the honest status by dimension, and replaces the milestone table, the
parallel-tracks table and "current focus" with a backlog.

* An item has a **stable id** (`P-012`; `T-01` for team work across Craft repositories), a
  one-line goal, its parity area, a size (S: one small PR; M: one slice; L: several slices and
  an ADR), what it needs first, and a lane.
* **Ids never move and are never reused.** Reordering changes an item's place, not its id. A
  finished item leaves the table; the changelog and `parity.md` record it. A new item takes the
  next free number wherever it is placed.
* **The order is a recommendation.** Anyone may pick an item out of order when they have a
  reason (a reporter is waiting, a contributor has it half built). The order says what we would
  pick with nothing else to go on, and what a reviewer's time goes to first.
* The milestones stay as a short "Done" table. New work is not given a milestone.

### 3. Six ordering criteria, in order

1. **Priority**: who is stopped, and how many. Crashes, hangs, lost work and failures to start
   come before any feature; among features, what artists use daily comes first.
2. **Correctness**: wrong behaviour before missing behaviour.
3. **Completeness**: finishing what is partly there before starting something new.
4. **Reliability**: the same result on every machine and every run.
5. **Robustness**: bounds on hostile or huge input, and tests that would catch a regression.
6. **Independence**: of two items otherwise equal, the one that can merge without waiting.

So the backlog starts with bugs and reliability, then parity gaps by how often artists hit
them, then partly finished features, then the long tail and what needs a decision.

**Lanes make independence visible.** A lane names the part of the code an item mostly touches
(`stage`, `timeline`, `shell`, `text`, `doc`, `geom`, `render`, `xfl`, `audio`, `player`,
`engine`, `platform`, `docs`). Items in different lanes with nothing under "Needs" can be built
and reviewed in parallel. Items in one lane are better done in turn. This replaces the
parallel-tracks table, which named milestones instead of code.

### 4. Every outside pull request gets a slot

A table in the roadmap lists **every open pull request that is not first-party**, with a backlog
id and one of four states.

| State | Meaning | What happens next |
|---|---|---|
| **planned** | We want it. | It merges when the conditions in section 5 hold. The row says what it still needs. |
| **superseded by** | The result landed another way. | The row says what landed. The lead closes the PR with thanks and credit (`AUTHORS.md` where an idea was taken), and names anything in it `main` still lacks, which the author may offer as a smaller PR. |
| **declined** | We won't take it. | The row says why, in terms of the product or the standards, not of the author. The lead closes it. |
| **held** | We can't decide yet. | The row says what we are waiting for and from whom. When it arrives, the state changes. If nothing arrives in 30 days the lead may close the PR, inviting it to be reopened. |

* **When a row is written.** Before the PR is reviewed, and in any case within the 3 days
  AGENTS.md rule 37 gives for answering. A PR that matches no backlog item gets a new item (if
  we want it) or is declined (if we don't); it is never left without a row.
* **Who writes it.** The project lead, in a small docs PR or as part of a batch. Track agents
  don't comment on, close or relabel pull requests they did not open (AGENTS.md rule 4); they
  report what they found to the lead.
* **A PR that does several things** is planned "in parts", and its row says how to split it
  (AGENTS.md rule 8: one issue per PR).
* **Team PRs** (craft-libs pins, workflow changes) map to the team rows and merge on the team's
  schedule, each in a PR of its own (rules 10 and 17).
* **Feature requests** are mapped the same way in a shorter table: issue, what it asks for,
  backlog id or "delivered". Bug reports are listed inside the backlog items that fix them.

### 5. When an outside pull request merges

All of these, and nothing else, decide it:

1. Its row says **planned**, and what the row said it still needed is done.
2. It is rebased or merged up to `main`, and the checks in AGENTS.md › Checks are green at that
   head. A check that did not run is not a pass.
3. Clean room and licences hold: provenance is stated where format knowledge is involved, every
   new dependency's licence is on `standards/licences.md`, and every asset has its provenance.
   Unclear provenance means **held**, however good the code.
4. It passed the principal-engineer review in section 6.
5. **Its slot has come, or it doesn't need one.** A ready PR that touches no lane with work in
   flight merges at once; that is what the independence criterion is for. A ready PR in a busy
   lane waits for the item ahead of it, or the item ahead is rebased onto it, whichever is less
   work for the outside author.

Merging is done by the lead with the person's standing approval (AGENTS.md rule 4); no agent
merges or approves its own PR. Batching ready outside PRs into one integration branch, as #496
and #497 did, stays allowed: the rows of the batched PRs then say "superseded by" the batch,
with credit kept in the merge commits.

### 6. A principal-engineer review for every pull request

Before any PR merges, first-party ones included, a reviewer who did not write it answers these
in the PR. The author answers them first, in the description ("what I questioned and what I
changed because of it").

* **Is it the right change?** Does it fix the cause, at the right layer, or a symptom? Is there
  a smaller change, or a better design the author didn't take, and why not?
* **Does it match the reference app, or differ for a reason we can state?** A difference
  without a reason is a bug. A reason goes in `parity.md` or the guide.
* **Can it crash, hang or grow without bound** on input a file or an agent could supply?
* **Is it a command?** Can the CLI and MCP do everything the panel does, inside the automation
  roots?
* **Would a test fail if this broke?** Does each new test guard a behaviour a plausible change
  could break (AGENTS.md rules 27 to 29)?
* **Does it say what it doesn't do?** Partial or unsupported behaviour is reported to the user
  and listed in `parity.md` (rule 15).
* **Would a senior designer ship it?** Found where an artist coming from the reference app
  looks, usable by keyboard, on theme tokens, with a screenshot looked at critically.
* **Is it one thing?** One issue, no drive-by refactors, moves separate from behaviour.

The review is recorded as a comment or review on the PR. "Looks good" is not a review; a
sentence per question that applies is. First-party work gets no lighter pass: the backlog's
first tier exists because first-party slices shipped with gaps.

### 7. What does not change

* The honest status table stays, and a PR that changes a dimension still updates its row.
* ADR numbers, reservations, the changelog, the checks and the standards are as they were.
* Commit subjects keep their conventional prefixes. A PR for a backlog item names the item's id
  in its description. Whether the `(M<N>)` suffix on commit subjects gives way to the backlog
  id is left to the lead, because it is written in AGENTS.md's conventions.

## Consequences

* An outside author can find their PR in one table and read what happens next. Of the 27 open
  on 2026-11-20, sixteen are planned (two of them team work), six are superseded, one is
  declined and four are held. Seven needed closing, not reviewing, and nobody had said so.
* The roadmap is shorter to read from the top, and the backlog can be reordered without anyone
  renumbering or renaming a milestone.
* There are two documents to keep honest, `parity.md` and the roadmap, plus the PR table. The
  rule that a status change edits `parity.md` in the same PR is the guard; the PR table is the
  lead's to keep. If either goes stale it does more harm than no table, so the roadmap's risk
  list names both.
* Row counts in `parity.md` weigh a small feature like a large one. The page says so; the
  backlog, not the count, is the measure of what is left.
* "The order is a recommendation" means two people can pick the same item. Claims
  (`standards/claims.md`) already handle that.
* A 30-day limit on **held** is a default we chose, not something we were asked for.

## Alternatives considered

* **Keep milestones and add an M9 "parity".** It keeps a familiar shape, but one milestone
  holding fifty unrelated items says nothing about order, and outside PRs still have no place.
* **Track the backlog only in GitHub issues, labels and a project board.** Good for
  conversation, and the issues stay the place for it. But the order, the reasons and the PR
  states would live outside the repository, unreviewed, and agents are told not to poll GitHub
  (AGENTS.md rule 7). A file in the repository is reviewed like code and read offline.
* **Order by size, smallest first.** It clears the queue fastest and feels generous to
  contributors, but it would put a hex-field tweak ahead of a hang. Size is shown, and breaks
  ties only through independence.
* **One strict queue with no parallel work.** Simple, and wrong for a project whose reviewers
  and contributors work at the same time. Lanes say what can overlap.
* **Let parity live only in the guide's "Coming from Animate".** That chapter is for artists and
  should stay short. A matrix with ADR and issue links is for contributors; the chapter can be
  checked against it (backlog item P-016).
