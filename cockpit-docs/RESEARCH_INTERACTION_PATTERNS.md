# Human-AI review interaction — verified research synthesis (round 2)

*Compiled 2026-07-02 from a second deep-research pass (107 agents, 3-vote adversarial
verification per claim; competitor claims anchored to primary vendor sources because the
first pass's landscape findings failed verification). Companion to
[`RESEARCH_REVIEW_BOTTLENECK.md`](./RESEARCH_REVIEW_BOTTLENECK.md); drives the interaction
design decisions listed at the end and the roadmap amendments in [`ROADMAP.md`](./ROADMAP.md).*

---

## 1. The competitive field — convergence, and the gap cockpit occupies

- **Graphite (Diamond → "Graphite Agent")**: positions on *near-instant review latency*
  ("every PR in seconds, not hours") and a *published noise metric* — per-comment downvotes
  with a claimed <5% negative rate (dashboard example 3.5%). Findings are inline comments
  categorized by a **seven-type taxonomy** (logic bug, edge case, security, accidentally
  committed code, performance, quality/style, docs), most with one-click committable fixes;
  steering is **org-level natural-language rules** (+ OWASP/Airbnb/Google/PEP templates),
  not per-review configuration. [graphite.com/features/ai-reviews](https://graphite.com/features/ai-reviews)
  — confidence: high (first-party; noise metric partly reflects low volume, ~0.62 comments/PR).
- **CodeRabbit**: the price band is **$24–48/user/mo** (free tier exists; local IDE/CLI
  review is free but rate-limited). Its paid tiers make the reviewer *perform rework
  itself* — one-click autofixes, docstrings, generated unit tests, merge-conflict
  resolution, committed as follow-up PRs.
  [coderabbit.ai/pricing](https://www.coderabbit.ai/pricing) — confidence: high/medium.
- **The ceiling argument (use this, never vendor rankings):** even in Greptile's own
  vendor-favorable benchmark, the best AI reviewer caught **58% of critical bugs** — ~4 in
  10 missed by the *winner*; independent re-runs (Martian: best tool 53.5% recall; Augment;
  Macroscope: 48%) land in the same band. No benchmark anywhere shows an AI reviewer near
  100% on critical bugs. [greptile.com/benchmarks](https://www.greptile.com/benchmarks) —
  confidence: medium (vendor-run), directionally corroborated 3×.
- **Devin's MultiDevin** ships hierarchical *mission control for building*: one coordinator
  scoping, delegating to ≤10 worker sessions, monitoring, merging.
  [cognition.ai blog](https://cognition.ai/blog/devin-can-now-manage-devins) — confidence: high.
  (Refuted 1-2: claims about steering child agents mid-task by message — do not cite.)

**Strategic read:** the market is converging on always-on inline AI review with one-click
fixes — *collapsing the review/rework boundary*. Nobody verified occupies (a) an explicit
**human-gated review→rework loop**, or (b) **batch review mission control** — reviewing
fleets of agent PRs the way Devin builds with fleets. That is cockpit's lane, now with a
quantitative justification: AI review alone misses ~40%+ of critical bugs.

## 2. HCI evidence — the gated architecture is how experts already work

- **Four oversight surfaces** (interview study, n=17, FAccT 2026): a-priori control,
  co-planning, real-time monitoring, post-hoc review — oversight is *preventative and
  proactive*, not only after-the-fact. These map one-to-one onto cockpit: config/skills →
  plan gate → agent timeline → diff gate. A tool offering only post-hoc diff review covers
  one of four modes. [arXiv:2606.05391](https://arxiv.org/abs/2606.05391) — medium (n=17).
- **Experts control through plans, in small chunks** ("Professional Software Developers
  Don't Vibe, They Control", UCSD+Cornell, Dec 2025): all 11 feature-building participants
  used an explicit plan step — but **9 of 11 authored the plan themselves** (only 2
  approve-agent-drafts), and even 70-step plans were handed to the agent **~2.1 steps at a
  time** (max 5–6), verifying between chunks.
  [arXiv:2512.14012](https://arxiv.org/abs/2512.14012) — medium.
  → Cockpit's plan gate must support **editing/authoring**, not just approve/reject; and
  chunked checkpoints beat one monolithic approval.
- **Zero of 99 surveyed developers** consider agents safe for full autonomy; modification
  rate of agent code ≈ half the time; dominant stance: "always reading the output and
  steering." → Optimize the read-and-steer path, not an approve-all path.
- **Verification-first already happens in the wild** — developers treat passing tests as a
  correctness proxy ("we don't even need to look at the source folder anymore"). Evidence of
  *adoption*, not *safety*: surface test/check results as first-class signals, but never let
  green silently substitute for the gate.

## 3. The trust-miscalibration hazard (read before building any trust UI)

Microsoft Research (3 user studies, Feb 2026): **raw step-by-step agent traces are
cumbersome and ineffective** for verifying agent work — and a *better-designed* oversight
interface cut error-finding time (g −0.65) while **inflating reviewer confidence without
improving accuracy** (g 0.18); confidence rose most precisely when errors were missed
(g 0.85). [arXiv:2602.16844](https://arxiv.org/abs/2602.16844) — medium (n=12, CUA domain).

Implications for cockpit, binding until better evidence exists:
1. Never present the raw trajectory dump as the transparency mechanism (D2's compact
   summary is the right call; the raw log stays one click away, not the default).
2. Any confidence/provenance display must be judged by **detection accuracy**, not by how
   confident or fast it makes the reviewer feel. "Faster and more confident" is the
   *failure mode* unless accuracy holds.
3. Guided reading aids should order attention **without pre-supplying the verdict**
   (see priming, below).

## 4. Code-reading science — concrete sizing mechanics

Cisco/SmartBear study (2,500 reviews, 3.2M LOC, 2006 — old but the only verified sizing
data; medium confidence, correlational):
- Defect detection falls off sharply **above ~200 LOC per review** (guideline: under 200,
  never above 400) — cockpit's small-stacked-PR dogfooding rule, now with a number.
- Reading **faster than ~450–500 LOC/hour** degrades detection (recommended <300/hr).
- Author **pre-annotation** correlates with near-zero defect density — attributed to forced
  self-review, with a live rival explanation: **annotations prime reviewers and dull
  criticism**. Agent-generated walkthroughs must guide *order*, not conclusions.
- Refuted 1-2: the popular "60–90 minute session ceiling" — do not build timer mechanics on it.

## 5. Design decisions this research locks in (with the phase that owns each)

| # | Decision | Basis | Owner |
|---|----------|-------|-------|
| 1 | AI findings are a **triage layer, never a verdict**; advisory pins, dismissible | §1 ceiling (≤58% critical catch) | Phase B2 (shipping) |
| 2 | Findings get a **category taxonomy chip** + **per-finding downvote** feeding a local noise metric | Graphite's field-tested mechanics | Phase F (new) |
| 3 | Steering is **amortized**: org/repo-level natural-language review rules fed into the pre-pass prompt, not per-review config | Graphite rules model; skills system already exists | Phase F (new) |
| 4 | Plan gate gains **direct plan editing/authoring** (human-authored is the 9/11 expert mode) | §2 | Phase F (new) |
| 5 | Plans execute in **chunked checkpoints** (~small step batches), not monolithic fan-out | §2 (2.1 steps/prompt) | Phase F (candidate, needs design) |
| 6 | Trajectory transparency = **compact summary default, raw log one click away** | §3 | Phase D2 (as designed) |
| 7 | Evidence strip stays **deterministic signals**; no model-generated confidence scores without an accuracy-validated design | §3 | Phase B1 (as designed) |
| 8 | Guided reading order (risk-sorted file tree) **without verdict annotations** | §4 priming | Phase F (new) |
| 9 | Size discipline surfaced: warn when a single review unit exceeds ~400 changed LOC ("consider splitting") | §4 | Phase C (fold into size chips) |
| 10 | Batch **review mission control** is the positioning: the coordinator view over N gated reviews — the unoccupied niche | §1 | Product framing |

## Refuted / unusable claims (transparency)

1. Devin child-agent steer-by-message/pause/terminate mechanics — refuted 1-2.
2. The 60–90-minute review-session ceiling — refuted 1-2.
3. Vendor benchmark *rankings* (Greptile 82% vs Graphite 6% etc.) — contradicted by
   independent re-runs; only the ceiling argument survives.

## Under-researched (verified-empty, not settled)

Copilot code review / Cursor BugBot / Baz / Ellipsis / CodeAnt / Conductor / Terragon /
Sourcegraph Amp UX specifics; modern (2024–26) diff-presentation evidence (side-by-side vs
interleaved, semantic/AST diffs, reading tours); voice/spec-driven review; whether the
confidence-inflation hazard replicates on code diffs specifically. These produced no
verified claims — treat as open questions, not absence of activity.
