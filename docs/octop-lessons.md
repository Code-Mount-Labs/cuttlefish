# Learning from Octop: Cuttlefish design decisions

Reviewed 2026-11-07. This is a source and architecture review, not a penetration
test, runtime benchmark, or claim that Octop is defective. A simpler local product
can reasonably choose tradeoffs that would be unsuitable for our distributed system.

## Evidence and scope

The reviewed Octop checkout is commit
[`4022e24ef6c34e9716350872b0e93bfaae7a20c3`](https://github.com/TencentCloud/Octop/tree/4022e24ef6c34e9716350872b0e93bfaae7a20c3).
Its lockfile selects `octop-harness` 1.0.8. We inspected that source distribution,
verified its SHA-256 (`fa586d69952eda010452a431093e46ad7685b55189e651c2a5810d2af340b83b`),
and did not install dependencies or execute their code. The archive URL is recorded
in the [reviewed lockfile](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/uv.lock).
The temporary checkout is not vendored into Cuttlefish; no implementation code or
artwork was copied.

The observations below distinguish inspected behavior from inferred risk and from
work we still need to implement. Public documentation can lag the executable source.

## Where the designs overlap

Both systems put a coordinating agent in front of specialist work, preserve
conversation context, and expose agent capabilities. Octop already has a much
broader user product: chat surfaces, coding-provider integrations, expert templates
and workspace abstractions. Our current advantage is architectural focus, not
feature completeness: a separate routing process, independent node runtimes,
durable dispatch and conservative execution recovery.

Octop's [architecture](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/docs/architecture.md)
composes a deployment primarily in one Python process. Its linked-instance
features should not be confused with Cuttlefish's shared dispatch ownership model.
Likewise, Cuttlefish's passing fake-adapter tests do not prove that a real external
coding provider survives restart or honors cancellation.

## Patterns to adopt

| Observed pattern | Cuttlefish decision | Evidence |
| --- | --- | --- |
| A team host delegates specialist work and receives member results | Keep reasoning, routing and execution separate. The host synthesizes results; router placement cannot authorize arbitrary execution. | [Team design](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/docs/expert-teams.md) |
| Team and member views retain distinct conversations and attribution | Keep stable child session IDs; build one activity view with agent, goal, task and session attribution. Do not merge unrelated chats into a shared specialist session. | [Team manager](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/src/octop/infra/agents/teams/team_manager.py) |
| ACP offers start, message, status, permission response and close operations | Add a provider-neutral execution adapter with these explicit operations. The Rust domain must not depend on a provider SDK. | [ACP workflow](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/docs/acp.md) |
| A member waiting for human input is not treated as finished | Add a durable runtime permission-wait state, separate from acquisition approval and from task completion. | Team manager, `stream_peer_turn` and `_wait_member_hitl`; pinned harness `acp/client.py` |
| Plugin archives have download/expansion limits and path-containment checks | Require equivalent archive validation before any future capability installer can write to a node. | [Plugin manager](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/src/octop/infra/agents/plugins/manager.py), `_validate_plugin_zip`, `install_archive` |
| Templates and workspace access are distinct from conversation checkpoints | Keep editable instructions, execution state and shared artifacts separate. Add an artifact interface rather than treating every backend as a shared filesystem. | Architecture and team design above |

## Tradeoffs and failure modes to avoid

### 1. Process memory cannot be the ownership authority

**Observed:** `TeamJobTracker` uses dictionaries and explicitly documents that its
roster locks vanish on restart. The pinned harness inbox also keeps messages,
queues, worker tasks and concurrency locks in memory.
[Tracker source](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/src/octop/infra/agents/teams/jobs.py).

**Risk for our use case:** a restarted coordinator could forget work while another
computer continues executing it. Missing contact must not be interpreted as a
successful cancellation or permission to execute the task twice.

**Decision:** PostgreSQL is the authority for shared assignments; SQLite journals
local acceptance and results. Keep exact operation identities, execution epochs
and verified cancellation. Independent processes surviving a supervisor restart
remain a required test. Event delivery may repeat; external side effects are not
promised to be exactly-once.

**Current status:** implemented and tested with the fake specialist. Immutable
provider bindings and conservative permission reconnection contracts now exist,
with fault and restart tests. Real-provider reconnection and external-side-effect
reconciliation remain unimplemented.

### 2. Limit admission as well as execution concurrency

**Observed:** harness 1.0.8 has per-target and per-source locks plus a concurrency
semaphore. It creates tasks from an unbounded `asyncio.Queue`; the semaphore is
acquired inside those tasks. Older mailbox documentation describes serial
execution, so that document is not a reliable description of current concurrency.

**Inference:** limiting running work alone does not bound queued requests or
waiting task allocations under sustained load. This is not a measured outage.

**Decision:** add per-group and per-node queued-work quotas, fair admission and
explicit overload responses. Serialize mutations within a specialist session
while permitting different sessions to run concurrently. Replace group-wide
transaction serialization only after correctness tests and contention measurements.

**Current status:** capacity checks and bounded goal plans exist. General queue
quotas, fairness and load testing do not.

### 3. Permission requests must survive process loss

**Observed:** Octop's team manager has in-memory permission waiters, and the pinned
ACP client awaits an explicit permission response. The mere presence of
`trusted: true` in runner configuration is not evidence that this callback is
bypassed. We found no basis for such a claim.

**Decision:** persist each runtime permission request with the group, actor,
node, task, session, execution epoch, exact action digest, expiry and decision.
Revalidate all of them before executing. A model's answer is not a human approval.
Duplicate decisions must be harmless; altered actions need new approval.

**Current status:** catalog acquisition approval is durable and scoped by goal,
node and release. Runtime permission requests now persist separately in SQLite,
including the exact action, digest, expiry and reviewer identity authenticated
with a separately enrolled credential. Delivery persists an intent before any
provider reply and never automatically replays that intent. See the
[runtime permission contract](runtime-permissions.md). The local operator remains
trusted; centralized human identity, remote review and router synchronization are
not implemented. Acquisition approval still records its existing operator actor.

### 4. Configuration must not silently grant broader authority

**Observed:** the global ACP configuration routes use an admin dependency, while
per-agent routes use current-user ownership checks and can also update that user's
runner definitions. This is a route-policy difference, not proof of an unauthorized
cross-user exploit. [ACP routes](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/src/octop/api/routers/acp.py).

**Decision:** centralize authorization in domain/service policy shared by HTTP,
CLI and future UI adapters. Separate tool installation, runner configuration,
capability activation and one execution approval. Test that alternate endpoints
cannot bypass the intended actor and scope checks.

**Current status:** nodes cannot approve acquisitions through the HTTP API.
User roles and human-facing authentication still need implementation.

### 5. A version label is not immutable executable provenance

**Observed:** the reviewed plugin URL path accepts HTTP(S), limits archive sizes,
and can check a manifest's expected ID/version. That path does not require a
caller-supplied cryptographic digest for the downloaded archive. Some documented
coding runners use package-launcher commands rather than a fixed artifact.
[Plugin manager](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/src/octop/infra/agents/plugins/manager.py)
and [runner settings](https://github.com/TencentCloud/Octop/blob/4022e24ef6c34e9716350872b0e93bfaae7a20c3/src/octop/infra/agents/settings/acp.py).

**Decision:** before downloads are implemented, make provenance structured:
source, exact version, digest, platform, architecture, declared permissions,
credential references and evaluation requirements. Use a verified staged install,
isolated evaluation, explicit activation and rollback. Replication repeats target
compatibility and approval checks; it does not copy permission grants.

**Current status:** catalog text snapshots are immutable, but artifact digests and
permissions are prose in the recipe, not enforced typed fields. Do not call the
current learning workflow a secure automated installer.

## Cuttlefish changes that this review recommends next

These are implementation requirements, not claims about features added by this
branding and review change.

1. **Runtime execution and permission contract — implemented locally.** Add a durable permission-wait
   state and provider session binding before connecting a real coding agent.
   Define which provider operations are safely retryable and which require
   reconciliation. Keep provider-specific IDs outside user conversation identity.
2. **One real provider adapter.** Implement ACP or another provider's documented
   protocol behind the execution interface. Cover follow-ups, streaming, bounded
   output, tool approvals, cancellation and reconnect capability. A provider that
   cannot resume must leave the execution unresolved, not silently start over.
3. **Structured acquisition manifests.** Add enforceable provenance and platform
   compatibility, a staging installer and reproducible evaluation evidence. Run
   user-approved setup outside the router process. Add per-node activation and
   rollback before enabling replication installation.
4. **Event-driven planning and admission.** The current super-agent polls every
   five seconds and can spend model turns even when nothing changed. Wake on
   relevant revisions/results, retain durable budgets, and add queue quotas and
   fair scheduling. Bounded input rejection is useful but is not context retrieval.
5. **Observable collaboration.** Add a replayable activity stream and user-facing
   pending approvals. Keep artifact contents out of routine routing context;
   use explicitly granted, expiring references for future sharing.

Automatic goal-owner failover is still absent. It requires its own ownership
protocol and stale-owner fencing; execution failover does not solve it implicitly.
Prompt files are editable today, but production model-call audit should also
record the exact prompt digest/version and model configuration used.

## Required acceptance scenarios for those increments

| Scenario | Required result |
| --- | --- |
| Router crashes after accepting dispatch but before replying | Retry returns the same assignment; no second execution |
| Supervisor dies while a real specialist is working | Specialist continues if its adapter supports it; replacement reconnects to the same session |
| Provider dies after an external effect, before acknowledging | Mark unresolved and reconcile; no blind replay |
| Approval is pending during process restart | Same request reappears with the exact action and authenticated decision history |
| Old owner reports after reassignment | Stale epoch cannot mutate current work |
| User edits an approved action or a package changes at its URL | Reject the old approval or digest mismatch before execution |
| Many tasks arrive while all nodes are busy | Bounded admission and an explicit overload response; no unbounded waiting tasks |
| Member emits huge output or loses its streaming connection | Bounded transport, persisted result identity and replay without duplicate final messages |
| Mac-only release is replicated to Linux | Reject incompatibility before download or setup |
| New capability passes syntactic checks but fails behavioral evaluation | Remain inactive; preserve the prior working version |
| Unchanged goal is idle for an hour | No repeated model calls solely because polling timers fire |

Maintain the existing 100% line/function/region coverage target, but do not equate
it with provider correctness, security assurance, fault tolerance under real
partitions, or good model decisions. Those need the scenarios above, integration
runs and explicit model evaluations.
