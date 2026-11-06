---
title: Agents sidebar summary for working in-process subagents
date: 2026-11-06
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
parent: projects-al8
---

## Goal Capsule

Show Working in the Agents sidebar while a settled root owns working OSC 7501 child records, without changing what Core says about the root or its turn.
Authority: approved request and repository `AGENTS.md`; branch `fix/subagent-status-summary`.
Use a small mod-only change with test-first render coverage; stop and escalate if implementation requires model/shared API growth or new attention semantics.
This artifact is planning only: implementation, independent review, PR/CI, and landing remain with the parent workflow; Can approves merges.

## Product Contract

The sidebar currently reads root agent status, so an idle/done root can look settled while its in-process workers continue.
Separate child terminals already have urgency handling; this fix concerns records owned by the same terminal, not extra child rows.

### Requirements

- R1. In the Agents sidebar only, derive `working` from an `idle` or `done` agent when its exact-address terminal owns at least one nonempty child record whose direct `state` is `working`, regardless of program brand or record-ID prefix.
- R2. Preserve root `working` and `blocked` status; root errors must retain existing Needs you behavior. Child `blocked`, `error`, `done`, and `idle` alone do not promote or introduce new attention semantics. With mixed records, any working child qualifies under R1.
- R3. Recompute each render: missing/empty records or the final working child's completion/clear restores existing root Done/Idle behavior, including normal focused-agent handling. Never mutate source agent/terminal snapshots or invent root/turn events.
- R4. Apply the derived status consistently to sidebar ranking, row spinner, grouping, folded headers, and narrow rail. Keep existing separate-terminal tree behavior, pinning, and exact local/remote identity rules.
- R5. Document the sidebar-only distinction in `defaults/AGENTS.md`; no model/Core/shared API, notifications, tab/space summary, generated typings, release changelog, or separate pi-config Ask User patch changes.

## Planning Contract

- KTD1. Derive within the Agents mod, not Core/model (session-settled: user-approved — chosen over rewriting root/turn semantics: sidebar presentation can consume already-published records). Covers R1–R5. All three core-growth questions are no: this is presentation, mods see the data, and can derive it correctly.
- KTD2. Join once by full terminal `address`; use a tiny pure summary helper and shallow-copy only agents whose displayed status changes. Apply before `this.listed`, `track`, and `hold` so every existing sidebar consumer sees one consistent derived status. Do not alter `RANK` to solve this: Done normally outranks Working, so the settled root's displayed status itself must change.
- KTD3. Only working records affect this summary. Supervisor confirmed that blocked/error child propagation is outside the approved fix; preserve existing `kidStatus`/`tree` handling for separate terminals. Do not manufacture records as terminals or traverse record IDs as addresses.

Grounding: `src/window.rs:2003–2027` preserves arbitrary terminal fields, including `records`, while converting the root report to a status word, publishes the list, and refreshes mods on changes. `defaults/mods/agents/main.js` currently builds `this.listed` before reading `terminals()`; `tree()` only follows live terminal parents. Its existing `status`, `rank`, `track`, `hold`, and `header` consumers are the integration seam. `scripts/agents.test.ts` evaluates the real mod with a lightweight builder harness.

Record-schema evidence supplied by the supervisor from Core v0.1.18 `status.rs`: records map nonempty IDs to direct status objects, e.g. `records: { "subagents/worker": { state: "working", app: "pi-subagents", title: "worker", msg: "Running" } }`. Root status/report is separate, not a normal records entry; ignore empty keys. Do not inspect nested `record.status` or hard-code the example brand. Source: https://raw.githubusercontent.com/herdrdev/herdr-core/v0.1.18/crates/herdr-core/src/status.rs (supervisor inspection, not independently fetched here).

## Implementation Units

### U1. Derived sidebar summary with regression coverage

**Files:** `defaults/mods/agents/main.js`, `scripts/agents.test.ts`. **Covers:** R1–R4. **Dependencies:** none.
First add failing real-mod render tests, then implement the small exact-address derivation at the existing render seam. Extend the element test builder only enough to retain SVG source and `spin` calls so tests prove an actual row spinner rather than only a helper return value.

Test scenarios:
1. Idle and done roots with one/multiple working records enter priority Working (`sec-2`, not Done/Idle) and render a spinning row icon; grouped view retains its space header and renders the same spinner.
2. Working root remains working; blocked root remains Needs you even with a working record. Represent raw root error through the existing agent-facing blocked status, with terminal status/report error, to verify no promotion.
3. Child done/idle/error/blocked, missing/empty records, and an empty-key-only working entry do not promote. Mixed working plus blocked/error records still promote only eligible settled roots. Include a generic non-pi program and record ID.
4. On the same view instance, remove/clear the last working record and separately change it to done/idle/error/blocked; rerender restores the underlying root status and existing Done/Idle/focus behavior. No stale spinner or status cache.
5. Freeze or compare agent and terminal snapshots before/after render; no mutation and no new command writes occur. `t1`, `host-a/t1`, and `host-b/t1` must not share records; a missing exact match is unchanged.
6. Exercise both sorts, narrow rail, folded space header, and pinned row; spinner and navigation ordering follow the displayed summary where applicable. Existing kept/paneless child-terminal tests stay green, including parent urgency from separate blocked children.

### U2. Explain the presentation boundary

**File:** `defaults/AGENTS.md`, near Agents/Sub-agents. **Covers:** R5. **Dependency:** U1 behavior settled.
Add a short description distinguishing in-process OSC records from `hc run` child terminals: sidebar Working can reflect owned workers while root/turn and tab/space status remain unchanged; root Needs you retains precedence. Do not claim new inspect/peek functionality for records.
Verification: inspect the wording against U1 tests and run the existing documentation checks within the normal suite on a supported environment; no new documentation-specific test is required.

## Verification Contract

- During implementation, run `bun test scripts/agents.test.ts` red then green, followed by the Bun portion of `just test`: `bun test scripts/files.test.ts scripts/picker.test.ts scripts/palette.test.ts scripts/agents.test.ts scripts/spaces.test.ts scripts/worktree.test.ts`.
- Native gates are `just build` and `just test` (including `cargo test --locked`). Parent reports Bun available, but cargo/rustc/just/Weston/Xwayland absent locally. Do not install system dependencies or claim native validation; carry these gates to a supported environment/CI.
- For native UI proof on a supported Linux host, use only `scripts/sandbox` with isolated home/core: verify settled-root plus working-record sidebar, root-blocked precedence, and restoration after clear, in both sorts and narrow mode; capture a screenshot and run `scripts/sandbox down`. Never manipulate a live user app/core or fake GUI evidence. Native sandbox verification remains explicitly pending until performed.
- Independent review is required before acceptance; review specifically for raw snapshot mutation, status precedence, exact-address isolation, and unintended notification/model changes.

## Definition of Done

U1 render regressions pass and U2 documents only the sidebar behavior; changed production scope is limited to the Agents mod and guide, with no new API or Core semantics.
Record local test results separately from pending native/CI/UI checks; report remaining validation gaps rather than treating implementation-ready as implementation-complete.
Confidence check: lightweight, grounded seam and confirmed record shape; no launch-blocking product question remains. No deepening or subagent fanout required.
Document-review state: `ce-doc-review` has not run; absence of a dedicated skill-invocation tool does not make skills unreachable (their instructions can be read and followed). Independent review remains the parent workflow's gate.
