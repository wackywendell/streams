# Stream references

Behaviour agreed and implemented; isolated local acceptance is complete.
Live-catalog activation requires separate approval.
This document records the reference contract. The
[implementation design](stream-reference-implementation.md) defines the record,
reporting, editing and verification mechanisms.

## Problem and goal

References should follow useful work and explain their purpose in the effort.
The former reporting path could mention a newly opened PR in progress text
while leaving the shared card without its link. Automatic reporting discovered
repository documents through [a separate addition path](session-document-additions.md),
which excluded PRs and issues. The shared reference operation closes that gap.

The proposed model should give documents, issues and PRs the same relationship
rules. A reader should recognise a resource, understand why it matters and see
whether its part in the effort is finished. Conversation evidence should supply
that work context. Live tracking of GitHub or issue tracker state is outside
scope. A merged PR may still require deployment before it fulfils the effort's
needs.

The [catalog decision](durable-work-catalog.md) defines the effort and its
Sessions. This proposal extends its reference semantics while preserving its
ownership and lifecycle boundaries. Reference state will not finish or archive a
Stream, change an external ticket or close a running conversation.

## One current relationship per resource

Each resource should have one reference within a Stream, with one current role
and one state. A role explains why the resource belongs. State explains where
that role stands. Different Streams can use the same resource differently.
Changing the current role will not retain a structured history of earlier roles.
Runtime matching enforces uniqueness for exact saved identifiers. Recognition
across equivalent identifiers depends on the summarizer.

A reference should also carry a recognisable name and a separate purpose. The
purpose should explain why this effort needs the resource, including the required
outcome when applicable. For example, a PR's purpose might be to roll out work
configuration to two servers. Its remote title alone would not explain that
requirement.

Resource kind and target should remain structured. Kind identifies a PR, issue
or document; target identifies what to open. For now, kind should support labels
and grouping. Future specialised actions should be able to use these facts;
those actions and a service integration framework are outside this proposal.
A Link fallback should support relevant general HTTP(S) resources when no more
specific kind is established. Conversation evidence should guide kind
recognition; runtime validation should preserve the supplied identifier.

### Roles

| Role | Purpose in the effort |
| --- | --- |
| Deliverable | Work the effort produces or changes, such as an implementation PR or design document |
| Dependency | Something the effort needs from elsewhere, such as another team's fix or a platform rollout |
| Brief | A reference defining the requested outcome or acceptance criteria, such as an issue or agreed specification |
| Background | Material that helps explain or guide the work without defining its outcome or blocking progress |

A Dependency can already be available. An existing implementation required by the
effort can be a fulfilled Dependency. A related implementation consulted for
ideas can be Background. Resource kind alone cannot determine role.

### States

The state labels are Active, Fulfilled and Dropped.

| State | Meaning |
| --- | --- |
| Active | A Deliverable or Dependency still has a required outcome pending. A Brief or Background reference is currently in use as context |
| Fulfilled | The resource has served its intended purpose in the effort. The reference stays saved and visible |
| Dropped | The saved reference is excluded from the normal pane. Automatic reporting cannot restore that saved identity |

Fulfilment should follow the intended outcome. Opening a PR can fulfil an effort
whose assignment is to prepare a reviewable change. Another effort might require
that change to be merged and deployed. External merge or closure alone should
not decide reference state.

A fulfilled reference can remain useful. If a completed PR becomes the basis of
ongoing work, its current role can change to Dependency or Background. Background
in Active state means context currently in use. This interpretation keeps
one role and one state for every reference without adding a Context state.

## Reporting and authority

The Session report should propose reference changes from its captured
conversation and catalog context. Actions and clear statements can establish a
relationship. Mere mentions and uncertain relationships should be omitted. An
attempted file operation alone is not proof that a document exists.

The existing trimmed reporting projection should remain unchanged. It retains
conversation text and attempted file paths, and excludes tool-result text.
Reference inference should use that evidence conservatively. A useful link
appearing only in discarded output can remain undiscovered; no new action-result
extraction is part of this feature.

Automatic reporting should be allowed to add references and update their role,
state, name and purpose. It should also be allowed to drop references when
clear evidence establishes that they are no longer wanted. Restoring any Dropped
reference should require explicit user direction. Automatic proposals for the
same saved identity must leave its Dropped state intact. The summarizer should
reuse that reference when it recognises an equivalent identifier.

Manual edits to reference names, purposes, roles and non-Dropped states should
remain automatically maintained. Reports should treat saved values as their
baseline and may revise them on clear evidence. References should have no
per-field manual protection modes. Dropped retains its explicit-restoration rule.

References should belong to the whole Stream. Changes to their shared role or
state should require evidence about that whole effort. A Session finishing its
assignment or ceasing to need a dependency is insufficient. Uncertainty about
wider use should preserve the saved relationship. The design should not maintain
separate Session relationships or scan all Sessions to determine shared state.
Absence from a later report should not imply abandonment.

Automatic additions and drops should produce brief notices. Role, name, purpose
and other state changes should appear through the updated card. Save failures
and conflicts should always be reported. Automatic updates should not
require confirmation on the normal path.

## Targets and repository identity

Ordinary PR and issue links should identify their parent work item. A particular
comment should be a separate reference when that passage has a distinct purpose,
such as supplying a Brief or Background. Their full URLs should distinguish
parent items from separately useful passages.

Runtime identity comparison and checkout matching should use exact saved
identifiers. The summarizer should receive generic guidance to recognise the
same resource under equivalent URLs or identifiers and reuse a supplied known
identifier. It should avoid inventing canonical forms. Recognising equivalence
across different spellings is a model judgement; the runtime guarantees only
exact-identifier matching. This choice adds no URL normalisation rules or
separate equivalence registry.

A document target should carry its established repository URL and
repository-relative path directly. Repeating the URL across documents keeps each
reference self-contained and independent of mutable Stream repository aliases.
Automatic reporting should not require separate manual registration or create
aliases as a prerequisite to linking. The model should preserve unknown identity
as unknown and avoid guessing a repository or branch.

Existing document navigation guarantees should remain. An unknown ref or null
Session checkout does not prevent a link when its repository identity and path
are established. Missing local files or unavailable services do not establish
that a reference's role is finished. Existing links should survive unavailable
opening targets. The implementation design preserves current-file opening and
explicit checkout selection, with historical context available for explicit
recovery.

## Dropping and permanent deletion

Both actions should remain available. Dropping should hide a reference while
retaining the instruction against automatic restoration. Permanent deletion
should erase the reference completely, allowing reporting to discover the
resource again, including through a report already underway. Deletion should
retain no per-resource deletion markers for suppression or timing guards.
Prompts should emphasise dropping for ordinary cleanup.

Dropping suppresses the saved identity. If the summarizer fails to recognise an
equivalent identifier, the same resource may be added as a separate reference.

The normal pane should hide Dropped references and continue showing Fulfilled
ones. The References editor should include Dropped entries and offer Restore
with a choice of Active or Fulfilled. Ordinary text and role edits should leave
a Dropped entry hidden.

## Save and failure contract

Reports should propose changes to identified references and preserve references
outside those proposals. A stale proposal must leave a newer edit intact. If one
reference proposal conflicts, valid changes to independent references should
still save. The result should identify unapplied proposals and leave their
references unchanged, without automatic retries.

Session progress and reference saves should remain independent. Successful
progress should stay saved if a later reference save is refused. Local save and
publication should remain separate, preserving the
[record-local operation contract](record-local-operations.md). Reference updates
must fit its named-record reads, pure apply and single-target replacement.

The new path must preserve existing host, conversation, branch and membership
guards. A result captured before a Session moves, including a move away and back,
must not acquire authority to change the new membership's Stream references.
Unrelated Session progress must not invalidate a reference proposal. Resource
identity changes must not silently retarget a captured proposal. The
[implementation design](stream-reference-implementation.md) specifies the
captured operation and per-reference comparison rules. Independent acceptance
fits within a single Stream replacement.

## Representative cases

These cases define the required outcomes across reporting and editing entry
points.

| Starting situation and event | Expected result |
| --- | --- |
| A Session opens an implementation PR and the effort still requires review and rollout | Reporting adds a Deliverable in Active state, with a recognisable name and a purpose describing the needed outcome |
| A Session meaningfully uses a specification in a checkout with established repository identity but no Stream repository registration | Reporting can add the document as an Active Brief; no registration request or guessed branch is required |
| One Session finishes its work while another may still need a shared dependency | Its completion alone leaves the dependency's shared state unchanged |
| Clear evidence establishes that the whole effort abandoned a dependency | Reporting may drop it, hide it from the normal pane and show a brief notice |
| A later report uses the same saved identity or a recognised equivalent for a dropped dependency | The saved reference stays Dropped until the user explicitly directs restoration |
| A report misses that a new identifier is equivalent to a Dropped reference | Exact runtime matching may admit a separate visible reference; the original reference remains Dropped |
| The user permanently deletes a reference and reporting establishes its relevance again | Reporting may add it as a new reference |
| A report captures no reference A; the user adds and permanently deletes A before that report proposes adding it | The addition may save because deletion permits rediscovery and retains no history |
| A report proposes changes to reference A and adds B while the user makes a newer edit to A | A retains the user's edit, B saves, and the result reports the unapplied proposal without retrying |
| A review comment contains a particular explanation used as Background | It can have a separate reference when that passage serves a distinct purpose from the parent PR |

## Alternatives considered

| Alternative | Choice and trade-off |
| --- | --- |
| Keep document discovery separate and add a PR-specific patch | Chose shared relationship rules so the same authority and lifecycle questions are answered consistently across resource types |
| Require confirmation for each inferred link, or explicit registration only | Chose automatic saves from clear evidence to avoid a manual step whenever useful work creates or establishes a reference |
| Keep the three reference collections with common relationship fields | Chose one collection so shared mutation and presentation rules read one representation, accepting a broader initial consumer change |
| Restrict references to documents, PRs and issues | Chose a Link fallback so established relevant web context remains linkable when its more specific kind is unknown |
| Keep roles only in descriptions | Chose structured roles so the card can distinguish delivery, dependencies and context without interpreting prose |
| Automatically register Stream repository aliases for new document links | Chose a repository URL and relative path in each document target, accepting repeated URLs to keep reference meaning independent of alias changes |
| Add runtime rules for equivalent repository URL forms | Chose exact saved identifiers and generic summarizer guidance to reuse known equivalent identifiers; cross-form deduplication remains a model judgement |
| Protect manually edited reference fields | Chose continued automatic maintenance using saved values as the baseline, avoiding per-field modes while preserving the Dropped restoration rule |
| Record several roles or separate relationships for every Session | Chose one current role per Stream reference to avoid aggregation and coordination; earlier roles have no structured history |
| Infer completion from external lifecycle and fetch live status | Chose conversation-based work context; a merged prerequisite can still require deployment before its purpose is fulfilled |
| Use one mixed enum containing Context and Completed | Chose separate role and state because how a resource is used and whether its required outcome is achieved are independent facts |
| Use permanent deletion for ordinary cleanup | Kept both deletion and dropping; the retained dropped record makes suppression of rediscovery possible |
| Retain deletion evidence to stop every pending addition | Chose deletion that forgets the relationship and permits rediscovery, including from an already-running report |
| Reject every proposal when one reference conflicts | Chose independent acceptance so a stale edit does not discard unrelated additions |

## Limits and review questions

Reports can misinterpret evidence or miss a relationship. Conservative omission
and shared-scope checks limit unwanted changes, while notices make automatic
additions and drops observable. A mistaken drop can still hide a useful link and
requires explicit restoration. Relationships can remain stale when no Session
has evidence about the whole effort; this is a consequence of the chosen shared
scope and local reporting model.

Fulfilled and Dropped records will accumulate. Permanent deletion provides an
explicit cleanup action; no automatic expiry or historical ledger is proposed.
Saved states describe the effort's interpretation of evidence. They provide no
claim of fresh external verification.

The implementation design specifies the shared collection, revision guards,
request and result contract, editing and opening mechanisms. Its revised
contract, file-by-file plan and verification cases are author-approved.
Implementation-readiness review converged before implementation began.

This design supersedes the document-only scope, addition-only updates and manual
repository-registration prerequisite in [Session document additions](session-document-additions.md).
That record preserves the former design. Its evidence, provenance and admission
constraints remain relevant where this contract retains them. The
[catalog](durable-work-catalog.md), [Pi reporting](pi-work-card.md) and
[native presentation](native-herdr-work-cards.md) records retain their other
responsibilities.
