> Status: DRAFT design note. The reconciliation model here informed the
> orario scheduler spec (joshrotenberg/orario) and the telaio/spola
> reconcilers; this document is discussion material, not a spec.

# Autonomous software work as choreographed reconciliation

_cron + roba + personas + hermetic + GitHub(remote)/worktree(local) state. A discussion draft, not a spec._

"cron + roba + personas" reframes autonomous software work as choreographed reconciliation rather than orchestration. There is no conductor: each persona is a narrow, cron-fired, single-turn roba run that observes the current state of a shared durable store (GitHub issues/PRs/labels plus git), takes exactly one step toward a merged PR, and writes the result back. The fire-watch-react loop that a single `claude -p` turn structurally cannot run inside itself is externalized across ticks -- cron is the clock, the board is the queue, and no individual run ever has to span a turn. That is genuinely strong: level-triggered reconcilers over durable state get scheduling, retry, and crash-tolerance for free, and the scope line holds (roba stays the one-shot worker, no daemon, no new tool). But "narrow personas reacting to current state" is not, by itself, sufficient. The model silently assumes three things reconciliation does not hand you: an atomic way to CLAIM an item so two ticks do not both grab it, a durable and host-independent signal of what state an item is in (the local worktree cannot be that signal), and a bound on retries so a task nobody can finish does not become a money pump. Add those -- all expressible inside the same substrate, none a new tool -- and the hypothesis largely holds for a trusted, single-host, low-volume repo. The one thing it cannot fix by observing harder is the trust boundary at intake, where an untrusted issue body meets a full-auto worker; there, self-healing is irrelevant because the system executes the attack "correctly."


## Verdict at a glance

Sufficient with a small, bounded addition set -- all of it inside the same substrate (shell wrappers, git refs, labels, watcher personas), NONE of it a new tool or daemon. It is NOT sufficient as literally phrased ("cron + personas on current state"). The additions, in priority order: (1) an atomic CLAIM as each mutating persona's FIRST action -- create a deterministic branch ref `bot/issue-N` where N is the issue number only, via `gh api POST /git/refs` (422 if it already exists) or a non-force push of a UNIQUE stub commit; the git server becomes the lock manager. Never claim with a label (no compare-and-swap). (2) A durable, GitHub-native in-progress/done signal (a label, or draft->ready) set as the LAST action, replacing the "draft PR + no worktree = ready" heuristic, which lies at every crash boundary and is invisible cross-host. (3) A reaper/watcher persona plus an attempt counter that escalates to `needs-human` after N rounds -- bounded retry and dead-run reaping, the irreducible residue of Oban's Lifeline. Two supporting disciplines make those sound and are cheap: a deterministic pre-filter plus `flock` single-flight wrapper around each roba call (free no-op ticks, no same-persona overlap), and an external, off-host, non-Claude dead-man's-switch heartbeat (the failure it must catch is the cron host itself dying). If forced to name ONE addition, it is the atomic claim. All of this reconstructs Oban's CONTRACTS (atomic claim, bounded retry, reaping) thinly in git-refs plus labels plus watcher personas, while genuinely avoiding Oban's INFRASTRUCTURE (daemon, Postgres, job table). So the hypothesis is right that no new tool is needed and slightly wrong that nothing needs adding.


## The model: choreography, not orchestration

Name the pattern precisely, because the name settles most of the argument. An *orchestrator* is a distinguished process that owns the workflow state and issues commands to participants: Oban, a saga coordinator, a CI pipeline runner. *Choreography* has no such owner. Each participant carries a local rule of the form `precondition -> action -> postcondition`, observes shared state, and acts; global behavior is an emergent property of those local rules. "cron + roba + personas" is squarely choreography. No persona knows the pipeline as a whole. The groomer never calls the implementer; it just leaves the board in a state the implementer will later recognize. This is the same architecture as Kubernetes controllers, which never call each other -- the scheduler, kubelet, and replicaset controller each watch a shared store and drive their own slice -- even though the system as a whole is usually described as declarative orchestration. The precise claim is: this is a set of independent control loops coordinating only through shared state.

Each persona is a controller in the strict sense. It reads an observed slice of state (open issues carrying a label, draft PRs in some condition, CI status), diffs it against an implicit desired state (every ready issue eventually reaches a merged PR), and executes one reconcile step. The load-bearing property is that it is **level-triggered**, not edge-triggered. An edge-triggered design reacts to a delivered event exactly once; miss the event and the work is lost -- the exact fragility a push/notification stream has. A level-triggered design ignores events and re-derives what to do from current state on every pass. Cron-polling is level-triggered by construction: it does not care *why* the state is what it is, only *what* it is now. That is the entire source of the robustness. A crashed or missed tick costs nothing but latency, because the next tick re-observes the same state and continues. The price is that cron is the *only* trigger here (no watch/informer fast path), so an item can wait up to one cron period at each stage: end-to-end latency has a floor of roughly `cron_period x pipeline_depth`. For software work where each step already takes minutes, that is an acceptable trade.

**The store is the "etcd", but a materially weaker one.** GitHub plus git is the source of truth, and the analogy to a Kubernetes control plane is real but aspirational, not literal. The state is split across two stores (remote GitHub and local git worktrees), partly *derived* (CI status, mergeability are computed asynchronously), eventually consistent (read replicas can show stale lists), and -- decisively -- offers **no transactional compare-and-swap**. Labels and assignees are last-write-wins. Every hard problem below traces back to that one missing capability.

**Self-healing is real but scoped, and the scoping is the honest core of this doc.** What you get for free is tolerance of *transient* failure. Because a persona holds no state between ticks (all state is externalized to GitHub and git), a crash, timeout, or network blip leaves the durable state exactly where the last successful reconcile left it, and re-observation retries automatically. There is no message queue to wedge, no consumer to die with an in-flight message, no lock to leak. That is a genuine, large win over any queue-based design. But three things are explicitly **not** healed: *poison items* (a task no worker can ever complete is re-attempted every tick forever, burning money, because the pure model has no attempt counter); *logic errors* (reconciliation heals crashes, not wrong decisions -- a bad label is faithfully propagated downstream); and *stranded states* (an item that lands in a state no persona's precondition matches simply stops, silently and forever). So the correct statement is that infrastructure failure modes are *eliminated and replaced by state-machine design obligations*. Nothing gets stuck because of dead plumbing; things still get stuck because of gaps in coverage, convergence, or claiming.

**Why this sidesteps the single-turn wall.** The wall says a `claude -p` run is one non-interactive turn that cannot span turns, be woken, or run a fire-watch-react loop internally; the earlier "orchestrator-strand" finding is what happens when you try to run the loop *inside* one turn. Here the loop is never inside a turn. Each persona tick is one turn that performs one reconcile step and exits. The multi-turn control flow (implement, then watch CI, then react to review, then merge) is decomposed into many independent single-turn ticks whose only coupling is the durable state they read and write. The state spans ticks; no individual run has to. This is the same conclusion the earlier queue arc reached ("roba is the worker, the loop is external") but with the external loop reduced from Oban to cron -- legitimate precisely because level-triggered reconciliation needs far less from its scheduler than a job queue does. It needs a heartbeat, not a transactional dispatcher.

**The central design obligation the framing does not give you for free: a partitioned state machine.** With no lock and no CAS, the clean theoretical resolution to double-acting is not to add a lock but to make each pipeline state *owned by exactly one persona* whose precondition is mutually exclusive with every other persona's. If `new` is owned only by the groomer, `ready` only by the implementer, `in-review` only by the reviewer, and `approved` only by the merger, then at any instant at most one controller's precondition matches a given item, and mutual exclusion holds *by construction* without any lock. Mutual exclusivity buys safety (no double-acting); *exhaustiveness* (every reachable state has an owner) buys liveness (no stranding). This partition is a property of the persona *designs*, not something cron and roba supply, and maintaining it as personas are added or edited is the discipline the whole safety argument rests on.

Finally, the model lives on the **safety-versus-liveness tradeoff**. A controller that never acts is perfectly safe and perfectly useless. So every precondition you tighten and every human gate you insert to buy safety directly spends the autonomy the hypothesis is trying to demonstrate. Idempotency is a safety obligation on every action (level-triggered reconcilers are at-least-once, so an action must be naturally idempotent or guarded so that once taken its precondition is false). Convergence (does issue -> merged terminate?) is only *conditional*: the happy path is a monotone progression to the absorbing `merged` state and terminates, but the rework loop (reviewer requests changes, implementer fixes, reviewer rejects again) is unbounded in the pure model and needs an explicit bound. The reconciliation framing is the correct and honest one, it delivers crash-tolerance and single-turn-wall avoidance for free, and it reduces the problem to designing a partitioned, idempotent, convergence-bounded state machine plus an in-band claim and attempt-bound -- a real, non-trivial design task, but a smaller and more tractable one than building a durable orchestrator.


## The pipeline: states, signals, and per-persona transitions

The pipeline is a directed graph with a happy path and two off-ramps that any persona can push an item onto. The design discipline that makes it self-healing is: **for each state, pick ONE authoritative signal and treat everything else as advisory.** The reason is torn-state recovery -- when a persona dies mid-transition you can always ask "what does the authoritative signal say?" and repair the advisory signals to match, rather than reconciling three signals that disagree.

### State machine (text)

```
              (groomer, author_association gate)        (groomer splits oversize)
   [new] --------------------------------------> [triaged] -----------------> epic + [new] x N sub-issues
     |  \                                            |
     |   \-> ((closed / wont-fix / noise))       label:ready  (the TRUST boundary)
     |                                               v
     |                                            [ready] ---[implementer: CLAIM first]---> [in-progress]
     |                                               ^   push bot/issue-N (git-ref CAS); winner only
     |                     (reaper resets stale claim)|   then open DRAFT PR (closes #N); label in-progress
     |                                                |
     |                                   implementer works, then LAST ACTION:
     |                                   label:needs-review + summary comment
     |                                                v
     |                                          [needs-review]
     |                                            |         |
     |               reviewer approves:           |         |  reviewer requests changes:
     |               undraft + label:approved      |         |  label back to in-progress, attempt++
     |                                            v         v
     |                                      [approved]   [in-progress]  (bounded: after N rounds -> needs-human)
     |                                            |
     |          merger: CI GREEN and MERGEABLE and non-draft (RE-CHECKED at merge time)
     |                                            v
     |                                        ((merged))  issue auto-closed; branch + worktree GC'd
     |
     +-- any persona, ambiguity beyond its scope --> [needs-human] (+ question comment) [frozen: excluded from ALL queries]
     +-- dependency / persistent-red-CI / post-approval conflict --> [blocked] (+ blocked-by:#M) [watcher clears when #M resolves]
```

Absorbing terminals: `((merged))`, `((closed))`. Escape terminal: `[needs-human]` (the dead-letter analog; a human removing the label re-admits the item). Every non-terminal state MUST have exactly one owning precondition, mutually exclusive and exhaustive, or the item strands.

### State -> authoritative signal (advisory signals in the last column)

| State | Authoritative signal | Advisory (repaired to match) |
|---|---|---|
| new | issue open, no pipeline label | -- |
| triaged | label `triage` | groomer scoping comment |
| ready | label `ready` AND no open linked PR AND no open sub-issues | size / priority labels |
| in-progress | claim ref `bot/issue-N` exists AND open DRAFT PR (closes #N) | label `in-progress`, local worktree |
| needs-review | DRAFT PR AND label `needs-review` | summary comment, CI status |
| approved | PR non-draft AND label `approved` AND a review approval | CI green |
| merged | PR merged / issue closed-as-completed | branch deleted |
| blocked | label `blocked` (+ `blocked-by:#M`) | comment |
| needs-human | label `needs-human` | question comment |

Note two deliberate choices. First, `in-progress` is anchored on the *claim ref plus the draft PR*, never on the local worktree: the worktree is single-host, crash-ambiguous (a crashed implementer leaves it present; an implementer that pushed then died before `git worktree remove` also leaves it present), and an inferred/negative signal that torn state corrupts. Second, the done signal is *positive and GitHub-native* (`needs-review` set as the run's last action), so a reviewer on any host queries "draft PRs labeled `needs-review` with CI not failing" and never infers anything from a worktree.

### Personas as controllers

| Persona | Observes (precondition / query) | One reconcile step | Writes (postcondition) | Claim / idempotency |
|---|---|---|---|---|
| groomer (front-line) | open issues, no pipeline label; author gate | triage: ready / split / close | labels ready|new(subs)|closed; scoping comment | guard on already-labeled/already-split; single-flight |
| implementer | `ready`, trusted, no linked PR, no open sub-issues, not needs-human/blocked | CLAIM ref, worktree, draft PR, implement, push | claim ref `bot/issue-N`; label in-progress then needs-review | **git-ref CAS is the lock**; loser aborts on rejected create |
| reviewer | DRAFT PRs labeled `needs-review`, CI not failing | review; approve or request changes | undraft+approve+`approved`, OR `in-progress`+change-request comment | key on (PR, head SHA): only re-review if SHA changed |
| merger | PRs non-draft, `approved`, CI green, mergeable (re-checked) | merge | `((merged))`; delete branch | GitHub server-side merge is atomic; 2nd tick 405/409 no-ops |
| reaper / watcher | stale claim ref w/o PR; in-progress w/o commit activity > T; blocked w/ closed blocker; K-round flip-flop | repair torn / orphaned state | delete stale ref; reset -> ready or -> needs-human; clear blocked | pure reconciler; all writes idempotent + precondition-guarded |
| auditor (out-of-band) | main HEAD / merged PRs / latest release | flag regressions | files NEW issues (label `ready` or `needs-human`) | read-only + issue-create; output re-enters at the `new`/`ready` boundary |
| test-runner (out-of-band) | main, on a schedule | flag failures | files issue `ready` / `priority:high` | read-only + issue-create |

The out-of-band personas compose without special-casing precisely because their only write back into the system is *filing new issues* -- their output is just more input at the front of the pipeline. Human gates are encoded the same way: `needs-human` is a first-class state, not an exception path. Any persona that hits ambiguity beyond its narrow scope performs exactly two writes (add `needs-human`, post a question comment) and then stops touching that item; every persona's query EXCLUDES `needs-human`, so the item freezes in place with its context durably attached until a human clears it.

### The claim mechanism (the crux)

The question "how does an implementer atomically claim a ready issue so two ticks do not both grab it" has a precise answer, and it is not a label and not PR-creation. Labels have no CAS. PR-creation is not a claim either -- GitHub happily lets two PRs from two branches both say `closes #N`, yielding two implementations. The primitive that DOES give an atomic conditional mutation, with no database and no daemon, is the **git ref namespace**: a ref create that collides with an existing ref is rejected. The clean form is `gh api -X POST /repos/OWNER/REPO/git/refs -f ref=refs/heads/bot/issue-N -f sha=<sha>`, which returns 422 "Reference already exists" for the loser; the plumbing-level equivalent is a non-force push of a **unique** stub commit to `refs/heads/bot/issue-N` (unique so the loser's push is not a fast-forward of the winner's ref and is rejected -- a same-sha push to an existing ref is a silent no-op success and would defeat the CAS). Two rules make it sound: the claim key is a **pure function of the issue number** with no free-form slug (`bot/issue-N`, never `bot/issue-N-add-timeout`; two implementers choosing different slugs both win -- put the slug in the PR title), and the claim is the persona's **first** action, before opening the PR or relabeling. This is a genuine distributed compare-and-swap backed by the git server, and it is the single insight that lets "a durable async queue is really Oban" be pushed back on: the CAS that argument assumed only a transactional DB could provide is already sitting in the ref namespace.

Two residual windows remain, both tracing to the single-turn wall, and both are why the **reaper is not optional**. First, the winner can create the claim ref and then die before opening the PR: the ref exists, no PR exists, the issue still reads `ready`, and the next implementer cannot distinguish "someone is mid-claim right now" from "orphaned claim from a dead run." The reaper resolves it with a grace window: a claim ref older than T with no associated open PR is stale and gets force-deleted, re-admitting the issue. Second, every multi-write transition is non-atomic under the wall, so torn state (labeled in-progress but no PR; `needs-review` but CI still red; `approved` but main has since moved and the PR is now conflicted) is the *normal* condition. The mitigation is structural: every persona is written as an idempotent reconciler that filters its query to the state it acts on, re-checks each precondition immediately before each mutation, and repairs torn state it finds rather than assuming clean input. The merger in particular must re-check mergeable-and-CI-green *at merge time*, not trust the `approved` label, because approval and merge are separated by wall-clock during which main moves. That the GitHub merge is itself a server-side CAS is the one place the whole system has an atomic backstop: the worst case of a merger race is a *failed, safe, retryable* merge, not a bad one.


## The within-issue substrate: a git-native state machine

The pipeline above coordinates issues *across* their lifecycle. Zoom into one issue and there is a second state machine -- prep -> plan -> implement -> review -- with an elegant git-native realization worth calling out, because it answers a question the coordination layer leaves open (where per-item metadata lives).

Map the FSM directly onto git:

- **State** = a commit: an immutable, frozen snapshot at the end of a phase.
- **Phase pointer** = the checked-out branch (`phase-implement`).
- **Transition** = `checkout` -> run -> validate -> `commit`.
- **Metadata ledger** = `git notes --ref=agents/metadata`: a per-commit, ref-namespaced key/value store that rides alongside a commit without touching file content or its SHA. Build status, attempt counts, phase logs, the context handed forward -- all live here. This is the "sidecar database" the coordination layer wanted, but inside git itself: versioned, reconstructible, no separate store.
- **Isolation + cache reuse** = one worktree per issue (`.agent_worktrees/issue-123/`), with the phases as *sequential* branches inside it, so the compiler cache (`target/`, `_build/`) survives across phases instead of a cold rebuild each step.

Each phase runs a deterministic three-step recipe: an entry gate (`git checkout -b phase-implement`), context ingestion + execution (read the previous phase's note out of git, append it to the prompt, run `roba` once, exit), and an **exit gate that is mechanical, not self-assessed** (`cargo check` / `mix compile --warnings-as-errors`; non-zero freezes the worktree for a human, zero commits the frozen state and appends the note). That exit gate is the "deterministic bookends, probabilistic middle" discipline made concrete: the model does the creative middle, a compiler decides success. On completion a zero-footprint cleanup squashes to main, reads the final note into the PR body, and `git worktree remove --force` + `git branch -D` leaves no trace.

**One load-bearing correction: drive this FSM by reconciliation, not a resident loop.** A live per-issue orchestrator that holds one issue through all four phases is exactly the ephemeral driver the single-turn analysis warned against -- it dies mid-issue and the issue strands. Instead each phase is a *cron persona* that reads the current branch + note ("on `phase-implement`, note says build green -> advance to review") and takes one step. The branch-plus-note *is* what lets a fresh tick resume. Same git mechanics, fired by the same choreography as the macro pipeline.

Two properties of the git substrate matter for coordination:

- **The worktree is a single-host claim, for free.** `git worktree add .agent_worktrees/issue-N` fails if that branch is already checked out elsewhere, so worktree creation is a near-atomic *local* lock -- a nicer realization of the claim for one host than a label. Cross-host still needs the create-only remote ref, because two hosts can each make their own local worktree with no shared lock.
- **`git notes` are local by default** -- not pushed unless you `git push refs/notes/*`. So the note ledger is single-host-authoritative (fine: single-host is the sweet spot), but cross-host, GitHub (labels / draft->ready / the PR body) stays the *authoritative* signal and the notes are a rich local cache. Corollary: open the draft PR *early*, so "in-progress" is GitHub-visible, not merely a local branch.


## Where it breaks: failure modes, ranked

Ranked by how fatal each is to the "it just works / self-healing" claim, not by frequency. The recurring shape is worth stating up front: the mitigations for five of the six categories converge on the *same* missing thing -- a small deterministic selection/claim/recovery layer around each persona -- and exactly one category (injection) is immune to self-healing because the system executes it "correctly."

| Rank | Failure | Concrete scenario | Fatal to | Mitigation | Residual concession |
|---|---|---|---|---|---|
| 1 | Prompt injection | A crafted new-issue body steers a full-auto groomer to apply `ready` + `p0`; the implementer then trusts the label and implements attacker code; the merger merges | Correctness AND security; self-healing is irrelevant (the attack runs "correctly") | Gate on `author_association` (OWNER/MEMBER/COLLABORATOR) -- GitHub metadata, not injectable body text; strip the groomer of the power to apply the trust label; outside authors get `needs-human-triage`, never auto-promotion | Public repos regain a human intake gate; only SOLO / PRIVATE / trusted-author makes it moot |
| 2 | Orphaned / torn state | Implementer crashes after opening the draft PR; or a `git worktree prune` removes the scratch dir, so "draft PR + no worktree" is misread as DONE and an incomplete PR is reviewed and merged | The "self-healing" claim directly | Explicit single-homed status label as authority; a liveness timestamp per in-progress unit; a REAPER persona that resets units stale beyond ~2x the worker's hard run bound | Without the reaper, every crash is a permanent orphan; this is the required missing piece (Oban's Lifeline, reconstructed) |
| 3 | Thrash / non-convergence | Groomer closes a "stale" issue; a watcher reopens it; groomer re-closes -- forever. Or reviewer/implementer ping-pong on shifting subjective feedback with no ground truth | The "converges for free / self-healing" adjective | Make `ready` a function of *deterministic* signals (CI green, no unresolved threads, acceptance criteria met); a monotonic DAG (work moves forward only; reopened items get a `human-review` label the groomer may not auto-close); an attempt/round counter that escalates to a human after N | Human escalation at the edge; there is a design, not a convergence *proof* |
| 4 | Races (double-claim) | Two implementer ticks (cron overrun, or two hosts) both read "#42 ready, unclaimed" and both open a PR | Parallel throughput, not correctness | git-ref CAS on the deterministic `bot/issue-N` (loser's create is rejected); `flock -n` single-flight per persona kills same-persona overlap | `flock` caps throughput at one unit per persona per tick; real parallelism reintroduces the exact atomic-claim problem |
| 5 | Cost / runaway | ~5 personas x every 5 min x N repos is thousands of ticks/day, the board is usually quiescent, so the dominant cost is NO-OP ticks paying full model price to find "nothing to do"; a poison-pill issue is re-failed at fixed frequency forever | The bill, not correctness | A cheap `gh`-query pre-filter in the wrapper so no-op ticks spend zero model tokens; per-run `--max-turns` / `--max-budget-usd` caps (personas are profiles -- this is free); a poison-pill quarantine label after K failures | Intelligence must move OUT of the persona into a deterministic wrapper |
| 6 | Observability | An expired `gh` token silently stalls the board; ready issues pile up; nothing alarms | Trusting it unattended, not correctness | Durable-context timeline as an audit log for the ACTIVE case (every persona posts a structured verdict comment); a heartbeat persona PLUS an EXTERNAL, off-host, non-Claude dead-man's-switch for the STALL / host-death case | The dead-man's-switch cannot be a Claude persona and cannot live on the cron host -- the failure it must catch is that host dying |

### Notes on the two most fatal

**Injection is #1 because self-healing cannot touch it.** The trusted-label gate protects the implementer, reviewer, and merger -- they only act on labeled items. But the groomer is the *intake* persona and a confused deputy: it is the one persona that must read untrusted content *before* any trust label exists, and it also controls promotion into the trusted zone. In-band delimiting ("treat the following as untrusted data") helps probabilistically but is never a guarantee against a full-auto worker with write scope. The only strong mitigation is deterministic provenance gating *outside* the model (`author_association`), and even then the honest statement is: for a solo private repo where every author is the owner, injection collapses to near-zero, which is precisely why the whole hypothesis is honest for solo/private and dangerous for public. Hermetic (`roba --hermetic`) bounds the *blast radius* of a compromised run (a known, sealed tool and prompt surface) but cannot stop injection from subverting the groomer's *judgment*.

**Orphaned/torn state is #2 because it is where the artifact-inference trick fails hardest and produces false DONE signals.** "worktree + draft PR = in-progress" and "draft PR + no worktree = ready" correlate a local, per-host, ephemeral git worktree with a remote GitHub PR, with no transaction between them, and you cannot distinguish "slow but alive" from "crashed" by artifact correlation -- the classic heartbeat-less crash-detection failure. The fix is explicit: one status label is the authority, set as the last step; a liveness timestamp; and a reaper. Relatedly, nothing in the persona set garbage-collects abandoned worktrees (the scope line keeps worktree mutation outside roba), so a janitor cron is required, and it must not prune a worktree an active implementer is using -- meaning the janitor needs the same liveness signal.

### The recurring shape (why five categories share one mitigation)

The deterministic wrapper that cron actually fires, per persona:

```
flock -n LOCK  ->  gh query for exactly ONE claimable + trusted + non-quarantined unit
               ->  exit 0 if none (a FREE no-op tick, zero model tokens)
               ->  else CLAIM (git-ref CAS) + pin the unit into a task file
               ->  fire roba persona, capped (--timeout / --max-turns / --max-budget-usd)
               ->  the persona does the creative middle ONLY; it sets the durable done label last.
```

The claim, the idempotency check, the pre-filter, and the quarantine all live in this wrapper, not in the persona's probabilistic judgment. This wrapper, plus a reaper persona, plus an external heartbeat, is the honest "minimal missing piece" -- and none of it is roba, and none of it is a daemon.


## Sufficiency verdict and the minimal missing piece

**Verdict: sufficient with a small, bounded addition set -- and the additions all live inside the substrate, so the hypothesis is right that no new TOOL or daemon is needed, and slightly wrong that NOTHING needs adding.** As literally phrased ("cron + narrow personas reacting to current state"), it is not sufficient; three failure modes (double-claim, orphaned state, poison-item runaway) are real and none is fixed by observing harder.

**The minimal missing piece, in one line if forced: an atomic CLAIM run as each mutating persona's first action, with a durable GitHub in-progress/done signal replacing the local worktree.** Expanded, the irreducible set is three items plus two cheap supporting disciplines:

1. **Atomic claim (the one truly irreducible primitive).** A deterministic branch ref `bot/issue-N` created create-only via `gh api POST /git/refs` (422 on collision) or a non-force push of a unique stub commit. Mutual exclusion at the moment of taking work is a synchronous compare-and-swap; it is categorically not something reconciliation gives you by observing.
2. **A durable, host-independent status signal.** A label (or draft->ready) set as the last action. The worktree is scratch space, never load-bearing pipeline state.
3. **Bounded retry + dead-run reaping.** A reaper/watcher persona plus an attempt counter that escalates to `needs-human` after N rounds. Reconciliation supplies retry but never termination, so this is what converts "self-healing" into something other than "self-looping" on exactly the hard cases.

Supporting disciplines (cheap, and they make 1--3 sound): a `flock` single-flight + `gh`-query pre-filter wrapper around each roba call (free no-op ticks, no same-persona overlap), and an external off-host non-Claude dead-man's-switch heartbeat.

### Does state-as-queue smuggle Oban back in?

Partly -- and the distinction is the whole point. State-as-queue replaces Oban's **infrastructure** (Postgres, a job table, a daemon) but not its logical **contracts** (atomic claim, bounded retry, dead-run reaping). Those contracts reappear thinly, but inside the same substrate and with no separate process:

| Oban primitive | Provided here by | Free, or needs explicit work? |
|---|---|---|
| cron scheduler | cron | free |
| retries / backoff | reconciler re-attempts next tick | retry free; backoff needs a counter |
| transactional claim (SKIP LOCKED) | git-ref CAS / `flock` | NEEDS an explicit protocol -- the one real gap |
| uniqueness / dedup | deterministic branch = f(issue number) | discipline |
| Lifeline (dead-run reaper) | a watcher persona + roba's hard run bounds | in-model |
| Pruner / GC | a watcher persona | in-model |

roba's hard run bounds (`--timeout`, `--max-turns`, `--max-budget-usd`) are what make the reaper's staleness judgment *sound*: a hard-bounded worker is either done within its bound or provably dead, so "claimed for more than 2x the bound with no terminal state = orphaned" is a reliable reset rule. This is the strongest single argument that the reconstruction is genuinely thin rather than a re-implementation of Oban.

### Scope-line check

Does this violate roba's scope line (roba wraps/sugars the `claude` binary as a one-shot worker; it is NOT an orchestrator, daemon, or queue)? **No, and cleanly.** roba stays exactly the one-shot worker. Every added piece lives *outside* roba: cron is the clock, `flock` and git-refs are the lock, GitHub and git are the state, and the reaper/watchers are themselves just more one-shot roba runs. "Orchestration" is emergent from clock + lock + state + workers, with no persistent process and no added roba surface. This satisfies the earlier arc's conclusion ("roba is the worker, the durable orchestrator is a separate tool") rather than violating it: cron IS that separate tool, and the durable store Oban would have provided is provided by GitHub. The hypothesis's push-back on the Oban conclusion is vindicated at the infrastructure level and refined at the contract level: you need Oban's contracts, you do not need Oban.

The honest scope statement the doc should commit to up front: **this is sufficient for a solo (or trusted-team), private, single-host, low-volume, serial-per-role repo, with the claim protocol + reaper + external heartbeat added.** Outside those bounds -- public repos (injection stops being moot), parallel throughput (the atomic-claim problem returns in full), multi-host or multi-repo scale (the worktree signal must be dropped entirely and the claim ref becomes the only coordination primitive) -- the missing pieces keep growing until you have rebuilt a worse queue, and adopting Oban wins. The live question the doc should pose to readers is exactly that boundary: is the shell + git-refs + labels reconstruction actually simpler than the queue, and where does that flip?


## Where this lives: the mechanical complement, not roba

None of this belongs *inside* roba. roba is the one-shot worker: it seals a persona and runs a single `claude -p` turn. The state machine, the git ledger, the atomic claim, the reconciliation loop, the reaper -- that is a *harness*, the mechanical complement to roba, driving it from the outside. roba already exports the contract such a harness needs: the `roba-types` crate (the `--json` result envelope + the typed exit-code map), kept tokio-free precisely so a downstream harness can depend on it, and that contract is a CLI ABI, so the harness can be written in any language.

The minimal harness is not a program at all -- it is the setup sketch below: cron + shell + git + `gh`, calling `roba` per tick. That is the honest "cron + roba + personas is sufficient" claim in the flesh, and it is where to start. A *dedicated* harness (working name: *spola*, the shuttle that ferries work back and forth through the machine) earns its place only when the shell reconstruction of Oban's contracts -- the atomic claim, the bounded retry, the reaper -- outgrows shell: when you want them typed, tested, and observable rather than spread across `tick.sh` and label conventions. At that point the harness *implements* exactly the git-FSM and coordination described here; the model does not change, only where it is expressed. The progression: prove it in shell, graduate to a harness when the contracts demand it, and keep roba the sharp worker at the center either way.


## A concrete setup sketch: personas as .roba bundles + cron

This is deliberately buildable with today's roba. It uses the real surface: personas are `[profile.NAME]` blocks with `agent` + a run envelope, a `.roba/` bundle carries `roba.toml` + `system-prompt.md` + `mcp.json`, and `roba --hermetic` seals both axes (its own config pool and claude's ambient `~/.claude`), auto-discovering `./.roba` when no explicit `--bundle` is given.

### 1. One bundle per persona (the role travels with the run)

```
~/swarm/
  implementer.roba/
    roba.toml          # the persona: [profile.implementer]
    system-prompt.md    # the ROLE, carried hermetically (see note)
    mcp.json            # any MCP the role needs (adds to --mcp-config)
  reviewer.roba/
    roba.toml
    system-prompt.md
  groomer.roba/  ...   merger.roba/  ...   reaper.roba/  ...
```

`implementer.roba/roba.toml` (safe default, one named loaded gun -- the discipline this repo already enforces):

```toml
# safe by default: any bare run is read-only
readonly = true

[profile.implementer]
description = "Claim a ready issue, open a draft PR (full_auto worker)"
agent        = "implementer"     # a native claude role; see the hermetic note
full_auto    = true              # the named, explicit unsafe setting
max_turns    = 80                # the #308 runway lesson: cascades need room
max_budget_usd = 10.0            # cap so an unattended run self-arrests
effort       = "high"
```

Hermetic note (a real current rough edge, surface it honestly): under a full seal, a native `agent = "implementer"` file under `.claude/agents/` is NOT reachable. Two ways to carry the role today: (a) put the whole role prompt in the bundle's `system-prompt.md` and drop the `agent` pin -- fully hermetic now; or (b) keep `agent = "implementer"` and relax the seal just enough with `--setting-sources user` so a user-level agent file loads. Option (a) is the cleanest reproducible story; (b) is the "known-surface, minus one axis" story. Which one is right is one of the open questions.

### 2. The deterministic wrapper cron actually fires (claim + pre-filter live here, not in the persona)

`~/swarm/tick.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd ~/Code/github.com/OWNER/REPO
persona="$1"
case "$persona" in
  implementer)
    # pre-filter: exactly ONE trusted, ready, unclaimed, non-quarantined issue
    n=$(gh issue list --label ready --label trusted \
          --search '-label:needs-human -label:blocked -label:poison' \
          -L 1 --json number -q '.[0].number') || exit 0
    [ -z "$n" ] && exit 0                      # free no-op tick, zero model tokens
    # ATOMIC CLAIM: create-only git ref = f(issue number); 422 if already taken
    sha=$(git rev-parse origin/main)
    gh api -X POST "repos/OWNER/REPO/git/refs" \
        -f ref="refs/heads/bot/issue-$n" -f sha="$sha" \
        || { echo "issue-$n already claimed"; exit 0; }   # loser aborts cleanly
    gh issue edit "$n" --add-label in-progress --remove-label ready
    printf 'Implement issue #%s. Open a DRAFT PR that closes #%s from branch bot/issue-%s. When done, add label needs-review and post a summary comment.\n' "$n" "$n" "$n" > /tmp/roba-task-$n.md
    roba --hermetic --bundle ~/swarm/implementer.roba --profile implementer \
         --timeout 1800 -f /tmp/roba-task-$n.md ;;
  reviewer) ;;   # gh query: draft PRs labeled needs-review, CI not failing; no claim needed (idempotent)
  merger)  ;;    # gh query: non-draft, approved, CI green, mergeable RE-CHECKED; gh pr merge
  groomer) ;;    # gh query: new issues; author_association gate; label ready|split|close
  reaper)  ;;    # gh: claim refs > T with no PR -> delete; in-progress stale -> reset ready or needs-human
esac
```

The claim, the pre-filter, the quarantine check, and the caps all live in this shell; the persona does only the creative middle and sets the durable `needs-review` label last.

### 3. crontab (single-flight per persona, staggered so ticks do not pile up)

```cron
*/5  * * * *  flock -n /tmp/roba-implementer.lock ~/swarm/tick.sh implementer >> ~/swarm/log/impl.log 2>&1
*/5  * * * *  flock -n /tmp/roba-reviewer.lock    ~/swarm/tick.sh reviewer    >> ~/swarm/log/rev.log  2>&1
*/7  * * * *  flock -n /tmp/roba-merger.lock       ~/swarm/tick.sh merger      >> ~/swarm/log/merge.log 2>&1
*/13 * * * *  flock -n /tmp/roba-groomer.lock      ~/swarm/tick.sh groomer     >> ~/swarm/log/groom.log 2>&1
*/11 * * * *  flock -n /tmp/roba-reaper.lock        ~/swarm/tick.sh reaper       >> ~/swarm/log/reap.log 2>&1
0    * * * *  ~/swarm/heartbeat.sh   # posts a status comment AND pings an EXTERNAL dead-man's-switch
```

`heartbeat.sh` emits a one-line status (X ready / Y in-progress / Z stalled / last merge N hours ago) and `curl`s an off-host healthchecks-style URL; if that ping stops, the off-host service alarms -- the one monitor that survives the cron host dying.

### 4. The label vocabulary (the entire coordination protocol)

- `trusted` -- the intake trust decision (author_association gate result); the gate the whole injection defense rests on.
- `ready` / `in-progress` / `needs-review` / `approved` -- the pipeline states (authoritative signals; see the state table).
- `blocked` (+ a `blocked-by:#M` marker) -- removed from every query until the blocker resolves.
- `needs-human` -- the frozen / dead-letter state; excluded from ALL persona queries; a human removing it re-admits the item.
- `poison` (or an `attempt-N` counter label / a parsed marker comment) -- quarantine after K failed rounds; the bounded-retry backstop.
- `size:*`, `priority:*` -- advisory only.

### 5. Try it smallest-first

Start with ONE repo, ONE persona (implementer) plus the reaper, all authors trusted (so injection is moot), `flock` single-flight (so races are moot), hard caps on every run, and the external heartbeat. That configuration is the honest sufficiency claim in the flesh. Add the reviewer and merger next; add the groomer (the untrusted-intake persona) LAST and only behind the `author_association` gate, because it is the one that turns a private experiment into a public-facing attack surface.


## Open questions

- Can the partitioned-state-machine discipline (mutually exclusive AND exhaustive persona preconditions) actually be MAINTAINED as personas are added or edited over time, or does precondition overlap creep in and reintroduce double-acting and stranding? The entire safety argument rests on this staying true.
- Single-host or multi-host? Single-host makes the claim nearly free (per-role flock serializes ticks) and lets the worktree be a local convenience; multi-host forces the git-ref CAS as the ONLY coordination primitive and forbids any worktree-derived signal. Is single-host an accepted permanent constraint or a temporary one?
- Who applies the trusted `ready` label -- an autonomous groomer, or a human? This one decision determines whether the system is truly unattended (private/trusted repos) or supervised-at-intake (public repos need a human gate to close the injection hole). Is deterministic author_association gating sufficient, or does ANY full-auto persona with write scope reading ANY untrusted content remain a standing loaded gun?
- Can `ready` ever be made a fully DETERMINISTIC predicate (CI green + no unresolved threads + acceptance criteria met) so the reviewer/implementer loop provably terminates, or does acceptance judgment irreducibly live in the probabilistic middle, making a bounded-round circuit-breaker mandatory rather than optional?
- The attempt-bound / poison counter and per-item metadata: `git notes --ref=agents/metadata` is a git-native, per-commit, reconstructible ledger for exactly this, and better than an `attempt-N` label or a marker comment. But notes are LOCAL by default, so single-host they are authoritative while cross-host they must be pushed (`git push refs/notes/*`) or GitHub stays authoritative and notes are a local cache. Does a notes ledger relax roba's 'owns zero runtime state' firewall, or is it fine because the HARNESS (not roba) owns it and it is reconstructible?
- Reaper authority: the reaper performs destructive writes (force-delete stale claim refs, reset labels, reopen). Should those be human-gated (file a needs-human instead of acting) or trusted full-auto? What grace window T safely separates 'orphaned' from 'slow but alive' without ever yanking a live implementer's claim, and is a periodic heartbeat on the ref/PR worth the complexity?
- CI-green versus CI-absent: the merger gates on 'CI green', but a repo with no checks, checks not yet started, and checks passed are three different states. How does the merger avoid merging on a false green?
- Under hermetic, how does a persona's native `agent` role file load when `~/.claude` is sealed? Carry the whole role in the bundle's system-prompt.md (fully hermetic, drop the agent pin) or keep the pin and relax to --setting-sources user? This is the open claude-hermetic-axis question and it decides how reproducible a sealed persona bundle really is.
- Is the shell + git-refs + labels reconstruction actually SIMPLER than adopting Oban, or does it asymptotically become a worse queue? The answer likely flips at the solo/private-vs-public and serial-vs-parallel boundaries -- where exactly, and should the doc state that boundary as the real decision criterion?
- What is the swarm's blast radius if the single gh write-token on the cron host is compromised, and does unattended operation demand a scoped/rotating credential rather than a standing owner PAT? Relatedly, quis custodiet: the watchers are themselves personas that can stall silently, so the external dead-man's-switch is the only net that is not itself part of the swarm.

