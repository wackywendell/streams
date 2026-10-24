# ADR: Plan effects as data, then execute them

Status: proposed. Nothing here is implemented. The first use will be native reconciliation and `streams native project`; this record will be revised from what that implementation shows. It builds on the [bounded work ADR](bounded-work-and-card-events.md), which sets the cost budgets this structure is meant to make reachable.

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

Display actions never move the owner. Reconciliation may move its owned card, following the [card-follow policy](native-context-reconciliation.md#decision), but card placement does not select Stream membership. A projection therefore cannot cause a catalog change that causes another projection. Names are the one place Streams reads display state: it compares the current Herdr name with the name it last wrote, to tell a manual rename from its own. That comparison only decides whether to write, and once the name matches there is nothing left to do.

A process's own effects come back as events, such as `tab.renamed` after a rename. Those are triggers like any other and get their one pass, which finds nothing to do.

### Trust Herdr's reply, and stop on a mismatch

An executor should take Herdr's reply to an action as its confirmation, without a further snapshot. It should still check two things the reply already contains: that the reply concerns the same pane, tab or workspace the action targeted, and that the value matches what was asked for. Neither check costs a request.

If an action fails, or its reply does not match, the executor should stop the list there and report the action and the reply. Remaining actions in the list do not run: lists are short and ordered, and "record intent, act, record outcome" depends on that order.

Projections plan only real differences. The planner compares each desired value with the value observed in the trigger's snapshot, both normalised the way the frontend normalises them, and plans an action only where they differ. An echo of Streams' own write therefore plans nothing. Streams keeps no history of applied actions: whether an action runs depends only on the desired and observed values. Restoring a value that someone else cleared is an ordinary difference, and applies normally.

### Each system has its own action type and executor

Herdr effects and catalog changes should use separate action types, each with its own executor:

```rust
// Provisional sketch. The variants will come from native reconciliation.
enum HerdrAction { SetWorkspaceMetadata { .. }, RenameTab { .. }, RenamePane { .. }, .. }
enum CatalogAction { SaveSession { .. }, SaveStream { .. }, Publish }
```

The types enforce the boundary: the Herdr executor cannot be given a catalog action, so it only ever needs the host-local native writer lock, and the catalog executor only the catalog lock. No plan holds both locks or needs a transaction across them.

When one system's change requires the other to act, events connect them. For example, a conversation moves to another workspace. Herdr reports the move, the catalog side saves the Session under the new workspace's Stream, and that save is a record change the Herdr side then plans from, for example to rename the tab. Because data flows one way per field, this chain ends: the Herdr side's display actions are not inputs to the catalog side. It costs one extra pass when both systems change, which is cheap next to holding two locks.

### Plan under the lock

An executor should take its lock, read fresh state once, and plan from that state, rather than checking whether a plan made before the lock still holds. A plan made before the lock only decides whether there is anything to do. Planning is pure, so doing it under the lock costs almost nothing, and the plan that runs is always the one fresh state calls for. This is not a retry: each trigger is still planned and executed once. This replaces the per-step re-checks that account for most of today's snapshots. The existing rule stands: effects act only on a fresh snapshot taken under the host-local native writer lock.

### Durable records are actions too

Some effects must be preceded by a durable record, so a crash leaves evidence. These include card and editor launch evidence (the pending fence written before dispatch) and the record of which names Streams wrote. An action list should express the order directly: record the intent, act, record the outcome.

These files describe Herdr state and are protected by the host-local native writer lock, so they are `HerdrAction`s written by the Herdr executor. That keeps the whole "record, act, record" sequence in one executor under one lock.

A crash between those steps leaves a state layer 1 must recognise: the intent is recorded but the outcome is unknown. Uncertainty then becomes an explicit state rather than checks spread through the effect code. Pending launch and editor outcomes stay fenced; reappearance or reattestation alone cannot clear them.

[Owner-event card follow](native-context-reconciliation.md#decision) is different: putting an existing owned card in its owner's tab is idempotent. It needs no pending intent. An uncertain reply ends the pass with a warning, without replanning; the next owner move is a new trigger.

### Applies across the stack

This applies to native cards, the catalog and sync, `streams serve` reacting to owner moves, and card display. The Pi extension follows the same rule in TypeScript: its handlers translate Pi events into messages to the Rust child, and the child does the rest.

## Order of work

1. Apply this structure to the Pi child's native reconciliation and `native project`: one fresh snapshot per locked pass, and native actions confirmed by Herdr's replies. Revise this record from what it shows.
2. Then extend it to the remaining card open and close, capture and card-display paths.

The socket client and `streams serve` should land first, because they rewrite the same native card code.

## Open questions

Some Herdr replies may not carry the updated object, only an acknowledgement. Pane rename returns the renamed pane, and the caption projection already checks it; native reconciliation should establish what metadata, tab and workspace rename, and card-move replies contain. Where a reply carries no state, should the executor accept the acknowledgement alone? Proposal: only where the adapter models that field's normalisation exactly, for example metadata that Herdr stores verbatim. Otherwise a value Herdr normalised differently would look like a difference on every echo, and each pass would apply it again. A field whose normalisation the adapter can't model must get its value back in the reply, or not be projected.
