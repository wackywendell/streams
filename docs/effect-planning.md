# ADR: Plan effects as data, then execute them

Status: accepted design, partially implemented. [Observe and act](native-observe-and-act.md) supersedes native caller proofs, launch journals and runtime proof-state execution. Pure planning, one pass per trigger and separate catalog/frontend executors remain. Implementation of the old receipt-linking ports is not a reason to preserve them.

## Problem

Native card operations mix deciding what to do with doing it. Each step checks Herdr, decides, acts, then checks Herdr again to confirm its assumptions still hold. Steps do not share what they learned, so their costs add up.

`streams native project` shows this. It runs three projections in turn: summaries (workspace and pane metadata), names (workspace and tab names) and caption (the card pane's label). Each takes the workspace lock separately, takes its own fresh snapshot, and re-checks Herdr after each change. One run usually changes two or three things, yet it takes seven snapshots. Before the socket client lands it also runs a version check for every call: 19 Herdr calls in all.

Because decisions are made between I/O calls, the only way to test them is to run whole operations against a fake Herdr and count calls. Merging the three projections into one locked section is hard for the same reason: their checks are spread through code that also does the I/O.

## Decision

### Three layers in every system

Each system should split its work into three layers. Only the outer two do I/O.

1. **Information to data changes** (pure). Combine current state with something new — a snapshot, a record read or an event — and produce the new state. For example, an owner pane now in another tab means the Session's workspace has changed.
2. **Data changes to actions** (pure). Compare the new state with what Herdr or the catalog currently shows, and produce a list of actions: plain values that describe effects, such as "set this workspace's metadata" or "rename this tab".
3. **Actions to done** (shell). Carry out the list and report the outcome.

Reading is shell work too: it turns snapshots, files and events into typed values before layer 1 sees them.

This is a functional core with an imperative shell. Layers 1 and 2 can be tested on plain values, without a fake Herdr. Call budgets become a property of the shell: one execution per action list. Projection then batches naturally. Summaries, names and caption each add actions to one list, and the shell applies it in one locked section.

The catalog already partly works this way: `operations::reconcile` decides whether a captured operation applies without performing effects, and `CatalogBackend` performs them. The native card engine does not.

### One pass per trigger

Each external trigger — a Pi lifecycle event, a user command, a record file change or a Herdr event — gets exactly one pass: plan, execute, done. The executor does not feed its results back into planning. A long-lived process such as `streams serve` or a card runs each event through layers 1 and 2 once. Most events produce no actions, which keeps idle cost near zero.

There is deliberately no feedback loop that re-plans after a failure. A failed action is more likely to come from a bug in how Streams understands Herdr than from a transient fault, and re-planning would then repeat the same wrong action. Failures should be visible and fixed, not retried around.

### Data flows one way per field

Each system writes only fields the other does not decide from:

- Herdr structure — which workspace and tab a pane is in — feeds the catalog. A move changes Session membership.
- The catalog feeds Herdr display: names, metadata and the card caption.

Display actions never move the owner. Reconciliation may move its owned card, following the [card-follow policy](native-context-reconciliation.md#decision), but card placement does not select Stream membership. A projection therefore cannot cause a catalog change that causes another projection. Display reads compare the current value with the saved catalog value, as defined by [name ownership](name-ownership.md). They do not track who last changed a name. Once the values match, there is nothing left to write.

A process's own effects come back as events, such as `tab.renamed` after a rename. Those are triggers like any other and get their one pass, which finds nothing to do.

### Trust Herdr's reply, and stop on a mismatch

An executor should take Herdr's reply to an action as its confirmation, without a further snapshot. It should still check two things the reply already contains: that the reply concerns the same pane, tab or workspace the action targeted, and that the value matches what was asked for. Neither check costs a request.

If an action fails, or its reply does not match, the executor should stop the list there and report the action and the reply. Remaining actions in the list do not run. A missing reply may mean that the effect happened; report that without automatically repeating it.

Projections plan only real differences. The planner compares each desired value with the value observed in the trigger's snapshot, both normalised the way the frontend normalises them, and plans an action only where they differ. An echo of Streams' own write therefore plans nothing. Streams keeps no history of applied actions: whether an action runs depends only on the desired and observed values. Restoring a value that someone else cleared is an ordinary difference, and applies normally.

### Each system has its own action type and executor

Herdr effects and catalog changes should use separate action types, each with its own executor:

```rust
// Provisional sketch. The variants will come from native reconciliation.
enum HerdrAction { SetWorkspaceMetadata { .. }, RenameTab { .. }, RenamePane { .. }, .. }
enum CatalogAction { SaveSession { .. }, SaveStream { .. }, Publish }
```

The types enforce the boundary: the Herdr executor cannot be given a catalog action or hold a catalog lock. The native queue serializes its own work; catalog saves use their record locks. No plan needs a transaction across the two systems.

When one system's change requires the other to act, events connect them. For example, a conversation moves to another workspace. Herdr reports the move, the catalog side saves the Session under the new workspace's Stream, and that save is a record change the Herdr side then plans from, for example to rename the tab. Because data flows one way per field, this chain ends: the Herdr side's display actions are not inputs to the catalog side. It costs one extra pass when both systems change, which is cheap next to holding two locks.

### Observe once, then plan

The executor reads the concerned records and takes at most one native snapshot for a pass. Its pure plan compares those inputs with the requested behaviour. All actions consume that plan and the same observations; they do not attest the caller or re-read native state between effects.

The Pi child's queue serializes its work. Catalog writes retain their own record locks. Neither a local queue nor a native lock prevents the user changing Herdr between observation and action. Use pass-local addresses and one subscription-ended check, with the [accepted restart risk](native-observe-and-act.md#bound-targeting-to-the-current-pass). Repeated observations cannot make targeting atomic.

### Failures do not create native history

Native creation uses no record-intent → effect → record-outcome journal. A lost reply reports that the resource may have opened and ends the affected pass. The next explicit request observes current state. Card duplicates exit themselves; editor, Pi and workspace duplicates are an accepted best-effort trade-off. No saved attempt is replayed or repaired.

Actions may use handles returned by an earlier action in the same list. Resolving those handles from a checked reply is execution, not another observation. Catalog save receipts remain durable catalog data; removing native receipts does not change save admission or accepted-result reporting.

Card follow is selected only by an observed owner move. It moves an existing card and never creates one. There is no pending follow intent, startup catch-up or automatic retry after a failed move.

### Applies across the stack

This applies to native cards, the catalog and sync, `streams serve` reacting to owner moves, and card display. The Pi extension follows the same rule in TypeScript: its handlers translate Pi events into messages to the Rust child, and the child does the rest.

## Verification

The [observe-and-act tests](native-observe-and-act.md#demonstrate-safety-and-progress-in-tests) exercise the real controller against outside changes between reads and effects. Counting tests enforce one snapshot per pass and no work for an unchanged projection. They replace tests whose only purpose is to preserve launch phases or caller attestation.

## Open questions

Some Herdr replies may not carry the updated object, only an acknowledgement. Pane rename returns the renamed pane, and the caption projection already checks it; native reconciliation should establish what metadata, tab and workspace rename, and card-move replies contain. Where a reply carries no state, should the executor accept the acknowledgement alone? Proposal: only where the adapter models that field's normalisation exactly, for example metadata that Herdr stores verbatim. Otherwise a value Herdr normalised differently would look like a difference on every echo, and each pass would apply it again. A field whose normalisation the adapter can't model must get its value back in the reply, or not be projected.
