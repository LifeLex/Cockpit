# The review bottleneck — verified research synthesis

*Compiled 2026-07-01 from a deep-research pass (105 agents: 5 search angles → source fetch →
per-claim 3-vote adversarial verification → synthesis). Every claim below survived
verification against its primary source; three popular claims were refuted and are listed at
the end so nobody re-imports them. This document is the evidence base for
[`ROADMAP.md`](./ROADMAP.md).*

---

## 1. The bottleneck is real, quantified, and growing — with one nuance

- **Meta (peer-reviewed, PACM-SE 2026)** is the most direct quantification of cockpit's core
  thesis: significant lines of code per human-landed diff grew **+105.9% YoY**, per-developer
  diff volume rose **+51%** (agentic AI responsible for **>80%** of that growth), while the
  share of diffs reviewed within 24h **declined**. The paper's own words: *"a widening gap
  between code supply and reviewer bandwidth."*
  [arXiv:2605.30208](https://arxiv.org/pdf/2605.30208) — confidence: high.
- **DX telemetry** (51k+ developers, 435 companies, Q4 2025): daily AI users merge a median
  2.3 PRs/week vs 1.4 for non-users — *"Daily AI users ship 60% more PRs."* Independently,
  Faros.ai (1,255 teams): high-AI-adoption teams merge **+98% more PRs** with **PR review
  time up +91%**. [getdx.com Q4 2025 report](https://getdx.com/blog/ai-assisted-engineering-q4-impact-report-2025/)
  — confidence: high (correlational; DX flags self-selection).
- **DORA 2025** (n≈5,000): AI adoption now correlates **positively with throughput** and
  **negatively with delivery stability**; DORA's stated mechanism is that AI-accelerated
  change volume *exposes weaknesses downstream* where control systems (automated testing,
  fast feedback loops) are lacking.
  [2025 DORA report](https://cloud.google.com/blog/products/ai-machine-learning/announcing-the-2025-dora-report)
  — confidence: high.
- **The nuance:** the same DX data ranks *meetings and interruptions* — not review wait — as
  developers' biggest obstacle. The bottleneck claim is strongest specifically where
  agent-PR volume is high (Meta, Cloudflare). Cockpit's market is exactly that segment.
- **Review burden is also getting heavier per line:** GitClear (211M changed lines,
  2020–2024): cloned-line share rose 8.3%→12.3% while refactoring fell 25%→10%; block-level
  duplicates (5+ lines) grew ~8x during 2024.
  [GitClear 2025](https://www.gitclear.com/ai_assistant_code_quality_2025_research) —
  confidence: medium (vendor research).

## 2. Human review remains a required gate — full autonomy fails empirically

- **MSR 2026** (peer-reviewed, 3,109 agent-authored PRs): PRs reviewed *only* by code-review
  agents merged at **45.20% vs 68.37%** for human-only review (23pp gap, p<0.001) and were
  abandoned at 34.88% vs 21.60%. The paper's conclusion: *"CRAs cannot effectively replace
  human reviewers."* [arXiv:2604.03196](https://arxiv.org/pdf/2604.03196) — confidence: high.
- **DORA 2025:** 30% of respondents report little or no trust in AI-generated code (down from
  ~39% in 2024 — trust is rising, but slowly).
- **Rubber-stamping is the default failure mode, not a hypothetical:** in ≥100-star OSS
  repos, **61.38%** of agent-authored PRs receive *no recorded review activity at all*
  (EASE 2026, AIDev dataset, 33,596 PRs).
  [arXiv:2605.02273](https://arxiv.org/html/2605.02273v1) — confidence: high (recorded
  activity ≠ total oversight; silent inspection is invisible).

## 3. What actually works at scale: risk-tiered pre-review funnels

Independent at-scale deployments converged on the same architecture — **layered
deterministic gates before LLM judgment, conservative auto-accept, human as the default
route**:

- **Meta RADAR** (peer-reviewed): source-type eligibility gates (vetted codemods, runbooks
  with 60-day clean history, volume caps, denylists) → ML Diff Risk Score percentile
  threshold → LLM reviewer that auto-accepts only at confidence ≥8/10 in predefined safe
  categories; *any* risk signal routes to a human. Results on 535K+ diffs: revert rate **1/3**
  and production-incident rate **1/50** of non-RADAR diffs, median time-to-close ~4.3x faster
  (observational — eligibility gates select low-risk diffs; the ratios embed selection).
  Every production incident that did occur had been *expert-reviewed*, and none were judged
  human-detectable. — confidence: high, with the selection caveat.
- **Cloudflare** (self-reported): explicitly names human review as a primary bottleneck
  (median first-review wait "measured in hours"). Their CI-native multi-agent reviewer, first
  30 days: 131,246 review runs across 48,095 MRs in 5,169 repos; 3 size/risk tiers
  (trivial ≤10 lines ~$0.20 → full >100 lines / security-touching, 7+ specialist agents,
  ~$1.68); median review 3m39s, avg $1.19. Their own words: *"This isn't a replacement for
  human code review."* [blog.cloudflare.com/ai-code-review](https://blog.cloudflare.com/ai-code-review/)
  — confidence: medium (first-party telemetry).
- **Atlassian Rovo Dev Code Reviewer** (peer-reviewed, ICSE 2026 SEIP; 1,900+ repos, 54k+ AI
  comments over a year): **−30.8% median PR cycle time**, **−35.6% human review comments per
  PR**; 38.7% of AI comments led directly to code changes (humans: 44.45%).
  [arXiv:2601.01129](https://arxiv.org/abs/2601.01129) — confidence: high
  (quasi-experimental, vendor-on-own-repos).
- **Microsoft reports** >90% of PRs covered (600K+/month) and 10–20% median PR completion
  time improvement across ~5,000 repos.
  [Engineering@Microsoft](https://devblogs.microsoft.com/engineering-at-microsoft/enhancing-code-quality-at-scale-with-ai-powered-code-reviews/)
  — confidence: medium (self-reported, no methodology).

## 4. Reviewing agent PRs is *behaviorally different work* — direct validation of the loop

EASE 2026, holding repositories constant: human participation is nearly identical on agent-
vs human-authored PRs (30.12% vs 30.83%), **but** only 65.53% of human comments on agent PRs
are direct line review (vs 93.56% on human PRs), while **agent-steering commands make up
25.92%** of comments on agent PRs (vs 1.63%). Review of agent code shifts toward
**steering/rework direction** rather than line-level evaluation. — confidence: high.

This is the single most product-relevant finding: the natural unit of interaction with an
agent PR is a **steer→rework cycle**, exactly cockpit's gated `request_changes →
Dispatched → Reworked` loop with ephemeral comments — not a durable GitHub comment thread.

## 5. Implications for cockpit (evidence → product)

1. **The gated review→rework loop is the right model** (§4). Invest in making the cycle
   cheap, not in replicating GitHub's thread model.
2. **AI pre-review as a first pass, never a gate** (§2, §3): an advisory reviewer-subagent
   pass (SPEC already permits it) mirrors Atlassian's −30.8% cycle time — but per MSR 2026 it
   must not become the merge decision.
3. **Risk-based routing of human attention** (§3): cockpit can't train a Meta-scale risk
   model, but the minimum viable risk signal is available locally: diff size, file
   sensitivity (config/migrations/auth/CI), test-delta, CI status, stack position, agent
   trajectory anomalies. Rank the board with it; give small+green+low-risk a fast lane —
   *presentation*, not auto-approve (§9 guardrails stay).
4. **Verification instead of reading** (§3, DORA's control-systems mechanism): surface
   machine evidence — CI per card, test deltas, what the agent ran — before human eyes reach
   the diff.
5. **Design against rubber-stamping** (§2): with 61% of agent PRs unreviewed in the wild,
   cockpit's job is making real review cheap enough to actually happen — and being honest
   when it isn't happening (e.g., approve-without-opening metrics), never adding one-click
   batch approval.

## Refuted claims — do not cite

Adversarial verification killed these; they circulate widely:

1. *"60.2% of closed CRA-only PRs had 0–30% signal-to-noise"* — refuted 1-2.
2. *"PRs waited days-to-weeks at Microsoft pre-AI"* — refuted.
3. *"84% of agent PRs lack any direct human evaluation"* — the number (61.38% unreviewed +
   22.6% agent-only = 84.0%) is real, but the *"no direct human evaluation"* inference was
   refuted 0-3 (recorded-activity data can't support it).

## Open questions (candidate follow-up research)

1. Minimum viable risk signal for small teams / local-first tools without Meta-scale history.
2. RCT-grade defect-escape comparison of AI vs human review (all current safety evidence is
   observational with selection effects).
3. **The commercial tool landscape** (Graphite, CodeRabbit, Greptile, Cursor BugBot,
   Conductor, Terragon) and stacked-PR/batch-review quantitative evidence — findings here did
   **not** survive verification; competitive positioning needs a separate targeted pass.
4. At what signal-to-noise threshold AI review comments become net-negative.
