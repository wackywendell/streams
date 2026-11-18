# Stream reference implementation design

Implementation, documentation and isolated local acceptance are complete. Live-catalog activation remains unauthorised.

The [behavioural contract](stream-references.md) defines the required reference behaviour. This document specifies the saved model, captured requests, result handling, interfaces, implementation phases and verification.

## Record and operation boundary

### Implemented boundary

The Stream stores one required references collection. Each entry has a name,
purpose, role, state and structured target. Document targets use exact
repository URLs and relative paths; PR, Issue and Link targets share exact URL
identity.

| Component | Current responsibility |
| --- | --- |
| [`src/model.rs`](../src/model.rs) | Defines Stream reference fields and document identity |
| [`agent.ts`](../extensions/stream-checkpoint/agent.ts) | Projects conversation evidence and infers Session progress and reference proposals |
| [`core.ts`](../extensions/stream-checkpoint/core.ts) | Captures reference requests, exact selectors and retained editor intent |
| [`report-workflow.ts`](../extensions/stream-checkpoint/report-workflow.ts) | Saves Session progress before validating and applying reference proposals |
| [`src/operations/references.rs`](../src/operations/references.rs) | Applies independent rows with captured identity, membership, Stream revision and Dropped guards |
| [`src/application/record_update.rs`](../src/application/record_update.rs) | Reads the automatic operation's named Session dependency; user reference saves read only their Stream target |
| [`src/storage/records.rs`](../src/storage/records.rs) | Defines single-target saves, named dependency reads and typed save outcomes |

Reference maintenance has a separate captured operation within the shared save
path.
Shared references support additions, updates, drops and deletions. Document
targets use direct repository URLs. The existing ownership, conversation and
membership-period checks remain.

### Save boundary

A Session report produces progress changes and reference proposals from one capture. Runtime supplies captured values and authority checks. The reporting model supplies the proposed relationship and supporting context.

```text
Captured conversation, Stream and Session
                  |
           Session report
                  |
     Save Session progress, when changed
                  |
  Apply reference proposals to the captured Stream
                  |
      Saved receipt and updated card
```

An automatic reference operation reads its Stream target and named Session dependency. A user operation reads only its Stream target. Pure apply compares affected references and returns one replacement Stream with accepted and unapplied outcomes. The storage layer saves that target once when it changed. References outside the proposals remain intact.

Construct and save the Session progress request first, then construct reference intent from the original captured catalog. Later reference validation or save refusal leaves earlier saved progress intact. Keep already-collected receipts and the record of prior accepted saves when stopping that stage; report a construction or application error without retrying. Accepted reference receipts keep per-reference outcomes alongside the aggregate decision.

The shared references collection replaces the three lists across all consumers. The project's alpha policy permits starting with a fresh catalog. Activation preserves the former catalog offline and starts the strict new representation.

## Saved values and resource keys

These TypeScript declarations define the wire representation. Rust types must use the same field names and tags.

```ts
type ReferenceRole = "deliverable" | "dependency" | "brief" | "background";
type ReferenceState = "active" | "fulfilled" | "dropped";

type RepositoryDocument = {
  repository_url: string;
  path: string;
  ref?: string | null;
  commit?: string | null;
  source_session_id?: string | null;
};
type ReferenceTarget =
  | ({kind: "document"} & RepositoryDocument)
  | {kind: "pr"; url: string}
  | {kind: "issue"; url: string}
  | {kind: "link"; url: string};
type ResourceKey =
  | {kind: "document"; repository_url: string; path: string}
  | {kind: "url"; url: string};
type ReferenceValue = {
  name: string;
  purpose: string;
  role: ReferenceRole;
  state: ReferenceState;
  target: ReferenceTarget;
};
type Reference = ReferenceValue;

type AutomaticDocumentTarget = {
  kind: "document";
  repository_url: string;
  path: string;
  ref?: string;
};
type AutomaticReferenceValue = Omit<ReferenceValue, "target"> & {
  target: AutomaticDocumentTarget | Exclude<ReferenceTarget, {kind: "document"}>;
};
```

Every object rejects extra properties. Name, purpose and present recovery strings must pass the existing Rust nonempty text check.

Export isReferenceText from wire.ts and re-export it from streams.ts for core callers. Use it for every new reference-owned string check, including repository locators, portable paths, and resource-key strings. Its acceptance matches Rust check_text with empty_ok=false. Preserve the original string and apply the existing relative-path and URL rules after this check. Keep unrelated existing validators unchanged.

```ts
export function isReferenceText(value: unknown): value is string {
  return typeof value === "string"
    && !value.includes("\0")
    && !/[\ud800-\udfff]/u.test(value)
    && /[^\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/u.test(value);
}
```

The surrogate check rejects unpaired UTF-16 code units while permitting valid supplementary characters. The final expression requires a character outside Rust's Unicode White_Space set. U+FEFF alone is valid text; U+0085 alone is whitespace-only and invalid. Saved-reference validators and deferred proposal/request validation share this predicate. Run proposal checks in the existing reference stage after valid Session progress is saved.

RepositoryDocument replaces the old relationship-bearing Document type. It carries the established repository locator and portable path, with the existing optional recovery context. Preserve the current relative-path validation. Repository URLs retain the existing repository-locator text contract, including SSH forms.

Optional saved and user recovery fields accept an omitted value or null as None. Writers omit None fields. An omitted user recovery field clears that field because user desired values are complete values. Callers constructing an ordinary user edit must first copy the captured recovery fields and then change or clear only the selected fields. Recovery-field forms treat exactly empty input as an explicit clear; preserve and validate every nonempty input without trimming it.

Automatic document proposals exclude commit and source_session_id, including null values for those properties. Their optional ref is nonempty text. On an existing document, omission preserves its saved ref; a supplied ref replaces that hint. Clearing a saved ref requires a user edit. Existing commit and source Session hints survive automatic updates.

For a new automatic document, runtime assigns source_session_id from the captured Session and commit=None. Use a supplied established ref first. Otherwise use the captured Session checkout's explicit ref only when its repository URL exactly matches the document URL. With neither source, save ref=None. These recovery hints guide explicit recovery when opening a document.

Store `references: Reference[]` as a required Stream field. Serialize an empty array explicitly. Omit serde(default) and skip-empty handling for this field. Remove the old documents, issues and prs fields. Update minimal Stream fixtures with references=[], and reject old collection fields even when a references field is also present.

A resource key is derived from the target. URL kinds all produce `{kind:"url", url}`. Document keys contain only repository_url and path. Preserve their exact string values.

Target validation, painting and browser opening share one URL policy.

1. Reject raw whitespace, backslashes and Unicode control/format characters.
2. Reject malformed percent escapes and invalid UTF-8. TypeScript uses decodeURIComponent for this check. Reject decoded control/format characters while permitting encoded path characters such as spaces.
3. Require parsed HTTP(S), a host, an empty username and no password. TypeScript rejects nonempty URL username or password values.
4. Persist the original string. Parsing checks safety and supplies the browser argument after exact identity comparison.

Implement this policy in the model-level `reference_url(&str) -> Option<url::Url>` function. It combines the existing painter checks with the native browser credential restriction. Keep a painter wrapper for existing callers. The added credential restriction prevents painting controls for URLs rejected by browser safety checks.

### Model output

The model-facing Session output schema requests the following type. Keep the existing Session changes schema and allow an omitted or empty reference_proposals list.

```ts
type ReferenceReportResult = {
  changes: Record<string, unknown>;
  reference_proposals?: AutomaticReferenceValue[];
};
// Runtime output after parsing the Session envelope and changes only.
type SessionReportResult = {
  changes: Record<string, unknown>;
  reference_proposals?: unknown;
};
```

For Session inference, parseInferenceResponse returns SessionReportResult and InferenceResult uses that type. Retain the complete model-facing schema in buildInferencePayload. Runtime parsing uses a separate strict envelope schema that validates changes and treats the optional reference_proposals value as unknown. Validate exactly one record_card call, its root object, allowed top-level keys and the existing normalised Session changes before returning. A bad call, envelope, unknown top-level property or invalid Session change fails the whole response before saving.

Clone reference_proposals unchanged through this parser. Defer every error within that value, including null, a non-array value, malformed rows, invalid targets and unknown row fields. Do not call a reference validator or request constructor from parseInferenceResponse. Other inference kinds retain their current validation. Model output can be logged as parsed after envelope validation while its reference stage remains unvalidated.

Add `automaticReferenceRequest(context, rawProposals: unknown, operationId): ApplyRequest | undefined` in core.ts. An omitted value or empty array returns undefined. Otherwise validate the entire automatic proposal array, derive each exact resource key and call referenceRequest with automatic drafts. Any invalid proposal rejects this reference operation before a reference save.

runReport invokes this constructor only after processing the Session progress receipt, or after finding no progress changes. Use the original captured catalog. Catch reference-stage construction or application exceptions within inferAndApply, emit one warning through environment.notice and return false. Its caller returns savedAny. Preserve prior receipts, skip recapture and later Stream inference, and perform no retry. Receipt refusals keep the existing receipt-first stop path.

A proposal's exact target key determines whether it adds a reference or updates a captured one. Proposals contain no revision, actor or old selector. Known identities and saved relationships remain part of inference context. Omit unchanged proposals before constructing a reference save.

The Session reset catalog projection contains Stream fields id/repositories/references, Session fields id/host_id/stream_id/harness/harness_session_id/checkout, sessions=[], and the captured top-level checkouts and document_locations hints. Copy these values from the visible capture. Model input omits catalog_source and the old Stream collections. The Stream reporting model remains responsible for its existing automatic metadata fields.

### Identity and recognition

Name, purpose, role, state and document recovery hints are outside the key. Changing a URL target's kind while preserving its URL preserves the same resource key. Classification can improve without creating a second reference for that saved URL.

The reporting model reuses a known saved identity when it recognises an equivalent identifier, following the behavioural contract. Runtime compares exact identifiers and preserves their spelling. A missed equivalence can still produce a duplicate under a different key.

A Link supports relevant HTTP(S) material when conversation evidence establishes relevance and no more specific kind is known. Resource opening and validation use existing local machinery.

### Parent items and meaningful passages

Ordinary PR and issue references use the established parent-item URL.
A separately useful comment retains its full passage URL. For example,
a Deliverable can point to a PR while Background points to a particular
explanation in its review discussion. Their saved URLs distinguish the targets.

Reporting judgement recognises parent items and meaningful passages. Runtime
code compares the selected exact identifiers.

### Comparison and cleanup meanings

References carry no revision of their own. A user reference save compares and swaps on the captured Stream `revision`, so any Stream change since capture refuses the whole user request. An automatic reference row compares the complete captured reference value with the current one; an outside writer that restores the complete original value leaves no observable difference. Unrelated reference and Session progress changes remain outside that entry's comparison. A row whose desired value the Stream already holds is no change, so a retried save that already landed is recognised from the record. The Stream remains the single save target.

Drop retains an exact identity in Dropped state and suppresses automatic restoration. Delete forgets the relationship and permits rediscovery, including through a report already underway. A captured mutation still needs its original reference to match. Deletion never turns that mutation into an addition.

An addition can save when its key was absent at capture and is absent now. A temporary creation and permanent deletion between those observations leaves no per-resource history to compare. This is permitted rediscovery.

## Captured requests and discussion edits

StreamWire, SessionWire and Mode retain their existing meanings.

```ts
type SetReference<V> = {
  key: ResourceKey;
  desired: V;
};
type DeleteReference = {
  key: ResourceKey;
  desired: null;
};
type AutomaticReferenceChange = SetReference<AutomaticReferenceValue>;
type UserReferenceChange = SetReference<ReferenceValue> | DeleteReference;

type ReferenceEnvelope<C> = {
  version: 2;
  catalog_source: string;
  kind: "stream";
  before: StreamWire;
  changes: {reference_changes: C[]};
  title_default: Mode;
};
type AutomaticReferenceRequest = ReferenceEnvelope<AutomaticReferenceChange> & {
  actor: "automatic";
  inputs: [{kind: "session"; record: SessionWire}];
};
type UserReferenceRequest = ReferenceEnvelope<UserReferenceChange> & {
  actor: "user";
  inputs: [];
};
```

Require changes to contain only the reference_changes property, whose value is a nonempty array. Reject mixed ordinary field edits. A retried request keeps its original before value; record state decides its outcome.

A key present in before selects an existing reference. A key absent from before denotes an addition and must equal the desired target key. A deletion requires an existing captured reference. Automatic rows must keep the same old and desired key. User rows may correct a captured target.

For correction, key identifies the old resource and desired.target supplies the new one. Across the batch, each old or new key belongs to one proposal only. A row can use the same old and new key. Reject overlaps between different rows during request validation. An occupied destination belonging to an untouched reference is an apply-time conflict for that row.

### Discussion edit

Keep the existing stream and session discussion variants. Add this variant.

```ts
type ReferenceDraft<V> = {key: ResourceKey; desired: V};
type ReferenceDiscussionEdit =
  | {
      kind: "references";
      actor: "automatic";
      changes: ReferenceDraft<AutomaticReferenceValue>[];
    }
  | {
      kind: "references";
      actor: "user";
      changes: ReferenceDraft<ReferenceValue | null>[];
    };
type ReferenceDiscussionResult = {
  reply: string;
  edit?: ReferenceDiscussionEdit;
};
```

The full discussion edit schema allows one response variant from the two existing ordinary variants and ReferenceDiscussionEdit. A reference edit contains nonempty changes and uses the captured-request target/key checks. User-authority edits require explicit user direction, including an explicit restoration request for a Dropped reference.

The core builder is `referenceRequest(context, actor, drafts): ApplyRequest | undefined`. It validates and clones the capture, removes effective no-ops, and returns undefined when no rows remain. Compute automatic no-ops after applying the captured recovery-metadata preservation rules, so excluded provenance fields do not create spurious changes. It supplies exactly one Session input for automatic requests and none for user requests. It must preserve the original capture while building the separate progress and reference requests.

### Example request construction

These examples use an existing captured context, so before is the complete captured Stream object. Serialization produces the version-2 packets declared above.

```ts
declare const captured: {
  catalog_source: string;
  stream: StreamWire;
  session: SessionWire;
  title_defaults: {stream: Mode};
};
const automatic: AutomaticReferenceRequest = {
  version: 2,
  catalog_source: captured.catalog_source,
  kind: "stream",
  actor: "automatic",
  before: captured.stream,
  title_default: captured.title_defaults.stream,
  inputs: [{kind: "session", record: captured.session}],
  changes: {reference_changes: [{
    key: {kind: "url", url: "https://github.com/example/tools/pull/42"},
    desired: {
      name: "Roll out workspace configuration",
      purpose: "Merge and deploy the configuration to both servers",
      role: "deliverable",
      state: "active",
      target: {kind: "pr", url: "https://github.com/example/tools/pull/42"},
    },
  }]},
};

// These two keys must already exist in captured.stream.references.
const user: UserReferenceRequest = {
  version: 2,
  catalog_source: captured.catalog_source,
  kind: "stream",
  actor: "user",
  before: captured.stream,
  title_default: captured.title_defaults.stream,
  inputs: [],
  changes: {reference_changes: [{
    key: {kind: "url", url: "https://example.org/brief"},
    desired: {
      name: "Accepted rollout brief",
      purpose: "Define rollout acceptance criteria",
      role: "brief",
      state: "active",
      target: {kind: "link", url: "https://example.org/brief#accepted"},
    },
  }, {
    key: {kind: "url", url: "https://example.org/obsolete"},
    desired: null,
  }]},
};
```

## Pure apply and domain results

Add the following public Rust boundary to operations/references.rs. The signatures show the interface with method bodies omitted. Saved/user values and result types derive strict serde forms matching the wire declarations. ReferenceChanges owns validated, cloned capture and intent.

Decode reference intent in parse_record_update before any optional-field deserialization loses property presence. Keep each change as a JSON object while checking its exact required and allowed keys, then dispatch by the envelope actor. For automatic document targets, allow only kind, repository_url, path and optional ref. A present ref must be a nonempty string; commit and source_session_id are unknown properties even when null. User targets use the saved nullable recovery-field rules. Delete requires a present null desired; set requires a non-null desired.

Construct the following in-memory variants after those checks. ReferenceChange and automatic intent types have no Deserialize implementation. The parser constructs them from validated raw objects; direct Rust callers construct typed intent. `ReferenceChanges::new` requires Automatic variants for an automatic actor, UserSet/UserDelete for a user actor, and rejects native authority. This representation adds no wire tags.

```rust
pub enum ReferenceChange {
    Automatic {
        key: ResourceKey,
        desired: AutomaticReferenceValue,
    },
    UserSet {
        key: ResourceKey,
        desired: ReferenceValue,
    },
    UserDelete { key: ResourceKey },
}
```

```rust
pub struct ReferenceChanges { /* private validated fields */ }

impl ReferenceChanges {
    pub fn new(
        before: &Stream,
        actor: Actor,
        captured_session: Option<&Session>,
        changes: Vec<ReferenceChange>,
    ) -> Result<Self, ValidationError>;

    pub fn before(&self) -> &Stream;
    pub fn captured_session(&self) -> Option<&Session>;

    pub fn apply(
        &self,
        current: &Stream,
        session: Option<&Session>,
    ) -> Result<ReferenceReconciliation, ValidationError>;
}

pub struct ReferenceReconciliation {
    pub reconciliation: Reconciliation,
    pub reference_results: Vec<ReferenceResult>,
}

impl ReferenceReconciliation {
    pub fn outcome(&self) -> EditOutcome;
}
```

Constructor checks cover capture validity, actor/input shape, complete proposed values and batch independence. For automatic requests, require captured_session.stream_id=before.id and captured_session.host_id=before.owner_host_id, preserving the current addition operation's admission rule. Native actor is invalid. Leave semantic stale-state and Dropped checks to apply, so one refused proposal does not discard independent changes.

Apply checks current Stream identity and ownership first. User changes then require the current Stream `revision` to equal the captured one. Automatic changes compare the captured and current Session ID, native identity, stream_id and membership_epoch. Ordinary progress fields are outside that comparison. User changes require no Session. Pure apply performs no reads, generates no IDs and changes no external state.

### Refusal mapping

A row whose desired value already equals the current one is NoChange before any other check, so a landed retry writes nothing. Otherwise check the whole-operation guards first. If they pass, apply the first matching remaining condition independently to each proposal. The current reference comparison includes every captured field.

| Condition | Automatic result | User result | Effect |
| --- | --- | --- | --- |
| Current Stream identity or ownership differs | All rows Superseded | All rows Conflict | Preserve the entire current Stream |
| Current Stream revision differs from the user capture | Not applicable | All rows Conflict | Preserve the entire current Stream |
| Captured Session is missing or its native identity or membership period changed | All rows Superseded | Not applicable | Preserve the entire current Stream |
| A captured reference is now absent | That row Superseded | That row Conflict | Preserve deletion |
| A captured reference differs from its current value | That row Superseded | That row Conflict | Preserve the current reference |
| An addition's key is occupied now | That row Superseded | That row Conflict | Preserve the current reference |
| A captured Dropped reference is proposed in a visible state | That row Superseded | Continue normal user checks | Preserve Dropped on automatic reports |
| A correction's destination key is occupied by another current reference | Invalid automatic request | That row Conflict | Preserve both references |
| Preconditions match and desired is null | Invalid automatic request | Changes | Remove that reference |
| Preconditions match and desired changes a value or adds a reference | Changes | Changes | Install the desired value |

Use these concrete refusal reasons for the corresponding row conditions.

| Condition | Reason |
| --- | --- |
| Stream identity or ownership changed | captured Stream identity or ownership changed |
| User capture is stale | Stream changed since capture; capture fresh inputs |
| Session guard failed | captured Session identity or membership period changed |
| Reference absent | reference no longer exists |
| Reference changed | reference changed since capture |
| Addition occupied | reference already exists |
| Automatic restoration | reference is Dropped; explicit restoration is required |
| Correction occupied | replacement target already exists |

Validation errors for malformed payloads remain errors before apply. In particular, an automatic deletion is structurally invalid. Automatic restoration is a per-row semantic refusal because a well-formed proposal can encounter a protected saved relationship.

Derive action from before and desired. An absent captured entry uses add; null desired uses delete; a Dropped-to-visible transition uses restore; a visible-to-Dropped transition uses drop; all other mutations use update. Classify a refused automatic restoration as action=restore even though it is not accepted.

Apply independent rows in proposal order against their affected current references. Update existing slots, append additions and remove explicit deletions. Overlap validation makes the result independent of cross-row ordering. Validate the resulting Stream once before returning it.

### Domain encoding

```ts
type ReferenceResult = {
  key: ResourceKey;
  action: "add" | "update" | "drop" | "restore" | "delete";
  outcome: "changes" | "no-change" | "superseded" | "conflict";
  reason: string;
};
type ReferenceDomain = {
  outcome: "changes" | "no-change" | "superseded" | "conflict";
  conflicts: string[];
  reason: string;
  reference_results: ReferenceResult[];
};
```

Keep row results in proposal order. Accepted Changes and NoChange rows have reason="". Refused rows use the reasons above.

The aggregate uses Changes if any row changes the Stream. Otherwise use Conflict if any row conflicts, Superseded if any row is superseded, and NoChange for the remaining case. The existing Reconciliation enum remains unchanged.

For aggregate Changes, conflicts=[] and reason="", following the existing enum. Row refusals remain in reference_results. For aggregate Conflict or Superseded, conflicts=["references"] and reason is respectively "reference proposals conflict" or "reference proposals were superseded". For a whole-Stream or Session guard failure, use conflicts=["inputs"] and the corresponding guard reason. Aggregate NoChange uses conflicts=[] and reason="no effective reference changes".

For example, a stale automatic update plus an independent addition produces this domain result.

```json
{
  "outcome": "changes",
  "conflicts": [],
  "reason": "",
  "reference_results": [
    {
      "key": {"kind": "url", "url": "https://example.org/brief"},
      "action": "update",
      "outcome": "superseded",
      "reason": "reference changed since capture"
    },
    {
      "key": {"kind": "url", "url": "https://example.org/new"},
      "action": "add",
      "outcome": "changes",
      "reason": ""
    }
  ]
}
```

Use ReferenceReconciliation.outcome() for both real saves and preview. The application installs the reconciliation's replacement Record when present, retains the current Record otherwise, and stores this typed outcome in RecordChange.outcome. Preserve the existing projection for other operation kinds.

There is no stored result to replay. Every request, including a retry, compares the current collection with its captured preconditions.

### Application and preview integration

Add the following application entry point. It constructs user intent with no Session dependency and uses the shared RecordUpdate save path. Re-export it from src/application.rs.

```rust
pub fn save_user_references(
    store: &impl RecordStore,
    before: &Stream,
    changes: Vec<ReferenceChange>,
    defaults: TitleDefaults,
) -> crate::application::Result<RecordSave<EditOutcome>>;
```

Replace `CapturedEdit::Documents` with `CapturedEdit::References`. Automatic application reads the named captured Session; user application reads only its Stream target. The single-target store contract remains unchanged.

Preview in src/agent/preview.rs integrates the shared reference operation into simulated record updates. Preserve its exact captured-Stream/supplied-snapshot admission check. Automatic reference apply uses the supplied current Session and the membership-period comparison, allowing earlier simulated progress changes. User reference apply has no Session requirement.

For valid reference domain outcomes, return a simulated receipt containing the complete domain map. Install its simulated Stream only for Changes. Return receipts for whole reference Conflict and Superseded outcomes as well, so replay retains their diagnostic rows before its existing refusal check. Invalid capture admission still returns an error. Other operation variants keep their existing preview handling.

## Receipts, report stopping and notifications

Domain acceptance, local durability and publication remain separate receipt facts.

For a durable Changes result containing unapplied rows, finish_record sets accepted_save=true and output.code=2. Add one notice per unapplied row. The existing serve path produces SavedWithWarnings with the saved record and complete receipt. The save's local status remains persisted or unchanged on a receipt retry; publication is handled separately.

Add `receiptHasUnappliedReferences(receipt): boolean` in streams.ts. It checks every save's reference_results for conflict or superseded. A present reference_results list must pass the exact result schema above. Legacy domains for other operations may omit that list.

| Helper | Required treatment |
| --- | --- |
| receiptCanContinue | Retain the current domain-acceptance and local-persistence checks; partial aggregate Changes remains accepted |
| resultNeedsAttention | Retain existing checks and include unapplied reference rows |
| receiptHasWarning | Retain its existing composition and surface partial reference results |
| receiptText | Retain its save/local/publication summary and append one line per unapplied reference row |

Use `Reference not applied: KEY; REASON` for each diagnostic line. Format KEY as safe compact JSON for the exact ResourceKey, using the human-output encoding below. Encode any reference-derived text in REASON with referenceText. When receiptText appends these lines, omit a line already present verbatim in the receipt's warnings. Preserve other warning and save-summary text. The structured result remains authoritative; formatted text supplies inspection help.

In runReport, process each receipt in this order.

1. Call environment.receipt with the complete receipt.
2. If receiptCanContinue is true, set savedAny=true, retaining the existing meaning of an accepted save attempt.
3. Emit eligible accepted addition/drop notices.
4. Stop inferAndApply when receiptCanContinue is false or receiptHasUnappliedReferences is true.
5. At its caller, return savedAny on a stop, preserving earlier accepted progress. Stop before recapture and later Stream metadata inference. Keep existing identity and liveness checks and perform no retry.

The reporting boolean records whether at least one save was accepted. Warnings and receipts preserve unapplied proposal outcomes.

Extend ReportEnvironment.notice and Reporter.Options.notice to `notice(text: string, level?: "info" | "warning"): void`. Reporter.report forwards both arguments through its runReport adapter when fence.live(). In index.ts, supply a callback that sends text to the current UI with level defaulting to warning; keep the existing error-to-warning callback for error callers. Route info to the existing UI information notification. Replay retains notice text in its existing notices array and may ignore the level.

Live receipt warnings remain owned by index.ts receipt handling. In replay.ts, retain the complete simulated receipt first, then append receiptText(receipt) once to result.notices when receiptHasWarning is true. This includes partial Changes without requiring an accepted add/drop row. runReport emits only its existing notices and the specified accepted add/drop information notices; it emits no additional receipt warning. The existing replay refusal check still handles wholly refused receipts.

Only newly accepted automatic add/drop rows generate information notices. Use `Reference added: NAME` and `Reference dropped: NAME`, with NAME encoded by referenceText. Require local=persisted or local=committed and suppress notices on repeated receipts. Simulated accepted rows may generate the same text only in replay's collected notices. Updates, fulfillment and user actions keep ordinary receipt/card feedback.

Publication problems retain their existing separate warnings. A partial Changes receipt remains a completed replay sample with attention notices; a wholly refused reference receipt retains its rows and follows replay's existing refusal handling.

## Reference management and presentation

Use the existing `2: Stream` tab. Add a SelectItem with value `references`, label `References…` and description `Manage saved references, including Dropped entries`. Its fallback label is `Stream · References…`. Keep the other tabs unchanged.

The manager lists current saved references in saved order, followed by retained-only rows in referenceEdits insertion order. Use one row per exact selector tuple. A retained edit whose key is currently present belongs to that saved row; append its row only when that key is absent. Exclude the unfinished manual-add key new from this union. Add reference… resumes that form and shows an additional retained-input label when it exists. Opening the manager preserves every retained capture and input.

Each row has a visible ordinal and shows `N. NAME · KIND · ROLE · STATE`, plus its safe JSON ResourceKey. Display kinds as Document, PR, Issue and Link; roles as Deliverable, Dependency, Brief and Background; states as Active, Fulfilled and Dropped. Use these kind/role/state labels in normal cards and plain CLI output too. JSON retains lowercase wire values. A row with retained input also shows Retained edit, Retained addition or Retained deletion, derived from its captured presence and nullable desired input. A retained-only row adds Source absent. Its metadata comes from the captured source when present, otherwise from the non-null desired addition. Keep the original selector while displaying a corrected desired target in its input inspection.

Use the existing SelectList with each item's value equal to the exact JSON selector tuple. Its label contains the ordinal and relationship fields, and its description contains the safe JSON key and retained status. The fallback ctx.ui.select receives unique strings combining those same fields and maps the selected complete string back through the prepared row table. Never resolve by name or metadata, and never persist the ordinal as identity. Show the selected row's safe exact key in its action heading. Offer Add reference… and Close as separate action entries.

A saved row without retained input offers Edit, Drop, Restore when Dropped, Delete, and Back. Edit exposes name, purpose, role, kind, target and document recovery fields. Non-Dropped rows also offer Active/Fulfilled state selection. Keep a Dropped row's state Dropped during ordinary edits; its Restore action asks for Active or Fulfilled. Drop sets Dropped. Delete confirms `Forget NAME? Reporting may add it again, including from a report already underway.`

Add ReferenceEdit in core.ts and `CardState.referenceEdits?: Map<string, ReferenceEdit>` to the existing CardState type in panel.ts. Initialise the map lazily. A new form uses key `new`. Existing forms use JSON.stringify of `["url", url]` or `["document", repository_url, path]`. Retain the old selector key while correcting a target.

ReferenceEdit owns its original LocalContext, key, actor, desired input, dirty state and error. Desired input retains its automatic or user shape and may be null for a pending user deletion. It validates input and calls the shared referenceRequest builder. One selected-row save sends one change. Opening an existing selector resumes its retained instance. A new form resumes the new instance and refuses an entered key already present in its capture, directing the user to Edit that saved row. CardState retains its existing per-Session, catalog-source and Stream cache key.

Discussion reference edits require an empty referenceEdits map. index.ts inspects this map when the typed reporting model response arrives, before constructing or applying its reference request. If it contains retained input, apply none of the discussion batch and retain the composer with the error `Finish or discard retained reference edits before applying discussion reference changes.` Ordinary discussion replies and Session/Stream edits remain available. Background reporting continues independently of local forms. This local form rule lets each selector keep one draft without replacing earlier input.

For a discussion batch with no pending local edits, retain every conflict/superseded row before testing aggregate continuation. Clone its original context, actor and desired input into a separate ReferenceEdit. This includes refused deletions. Use the resource-key JSON tuple for all discussion rows, including additions; new is reserved for the manual-add form. Independent-key validation prevents collisions within the batch. Receipt recording and input retention happen before any refusal is thrown.

Partial Changes follows receiptCanContinue, commits the user and reporting model messages, clears the composer and leaves unapplied rows in the manager. A wholly refused receipt retains the composer and sets its existing error after retaining row intent. Local/publication failures keep their existing continuation rules and complete receipts. No unapplied row is retried automatically.

Remove a retained draft only after acceptance, explicit discard or a no-op against its captured value. Every retained row offers Inspect retained input, Inspect current row, Keep input and close, and Discard. Non-null input also offers Edit retained input. Inspect current row reports absence when the source is missing; offer Reapply against current row only while that source exists. Reapply explicitly displays current and retained values and replaces that row's capture before saving. A missing source keeps its independently selectable retained row; creation requires an explicit Add reference action.

Reapply prompts for Keep Dropped, Restore Active, Restore Fulfilled or Cancel when the fresh row is Dropped and retained non-null input requests a visible state. Default to Keep Dropped. Ordinary text reapplication must preserve Dropped unless that restoration choice is explicit. Reapplying a deletion displays the current row and requires the Delete confirmation again.

Retained automatic input keeps its original shape until the user acts. Reapply materialises it as a complete user value using the displayed current recovery metadata and the automatic preservation rules, then applies the restoration choice. Edit retained input materialises it against its original capture and opens the complete user form. Further edits retain user authority and the complete selected recovery values. Both paths require explicit user submission.

New forms require an explicit kind and role selection, start name, purpose and target fields empty, and default state to Active. Closing or reopening must leave dirty captures unchanged. Keep existing ordinary FieldEdit state and behaviour separate.

## Reporting evidence

Keep the existing trimmed projection unchanged. It retains conversation text and attempted file paths, including existing nested codemode path metadata. It excludes tool calls, tool-result text and thinking.

The reporting model proposes references when the retained conversation establishes their identity and relevance to the whole effort. Attempted file activity alone remains insufficient to establish that a file exists. A useful URL appearing only in discarded tool output may remain undiscovered. This accepted limit keeps the current evidence projection.

Full-context reporting remains available through the existing setting, with the whole-request size check unchanged. Reference inference uses the current evidence machinery and omits uncertain references.

## Document opening and normal-card rows

Document targets use their saved repository URL directly. Application
location hints resolve the distinct URLs needed by current document
references, using the existing checkout snapshot and explicit selection or
repository default.

A document reference remains saved through an unknown ref, missing checkout
selection or unavailable file. Opening uses an established selected checkout
or an explicit repository default. With neither usable location, it shows the
unavailable-location result while reporting preserves the relationship.
The user establishes checkout bindings and files through the existing commands.

Current-file opening uses the existing checkout resolver, containment checks
and native opening revalidation. Historical context guides explicit recovery
through the existing revision machinery. Normal opening resolves the current
worktree file.

### Location hints

Use these JSON shapes, retaining the current success/unavailable union.

```ts
type CheckoutHint = {
  repository_url: string;
  checkout_id: string;
  root: string;
};
type DocumentLocationHint = {
  repository_url: string;
  path: string;
  checkout_id: string | null;
  root: string | null;
  kind: "local" | null;
  file: string | null;
  error: string | null;
};
```

A successful location has kind=local, non-null checkout_id/root/file and error=null. An unavailable location has kind=null, file=null and a nonempty error; checkout_id and root are either both present or both null. Keep current absolute-path, canonicalisation and containment checks at their existing boundaries.

Rename CheckoutBinding.repository and DocumentLocation.repository to repository_url. Their Rust checkout selection and resolved-location payloads remain unchanged. Accept extra established checkout URL bindings as display hints, even when no document uses them; validate their text, roots and exact-URL uniqueness. Require one location for every non-Dropped document target and reject duplicate or unrelated location rows.

The resolution URL set is the exact union of visible document repository URLs, the captured Session's explicit checkout URL when present, and the registered repository URLs of the viewing Session's selected checkout. Read these established values from the existing checkout snapshot and directory binding. Enumerating the selected checkout's saved URL values requires no filesystem or catalog discovery. Resolve each distinct URL once with the existing selected-checkout/default policy. Emit successful CheckoutHint rows even before the Stream has saved a document.

Live observation and diagnostic capture use the same producer in cli/documents.rs. Extend `agent::capture_inputs` with a final `document_hints: impl Fn(&Stream, &Session) -> serde_json::Value` argument. For each typed sample context, call it once when both records exist and merge its checkouts and document_locations into that sample's serialised catalog. Otherwise emit both arrays empty. CLI capture constructs one registry and a DirectoryBindings from its clone, passes that binding object to capture, and supplies a closure calling `documents::context` with those prepared objects. Live observation calls the same producer. Keep hint errors in the existing unavailable-location union. Inventory-only capture has no samples and makes no hint calls.

TypeScript capture preserves those supplied arrays when freezing a sample. Reset projection copies them from that sample, and replay reads them without querying the live registry, directories or catalog. A Session without a portable checkout URL, aliases or saved documents still captures established URLs from its selected registered checkout.

Directory selection has three explicit outcomes. A missing selection, or a valid plain directory with no registered checkout, permits default resolution. Pass selected_id=None and selected_repository_url=None to the existing resolver for each established URL. The portable Session checkout URL still contributes to the URL set, and an explicit saved repository default supplies the location.

A broken explicit selection, unregistered VCS directory, or registry/binding read error produces unavailable locations. Preserve the current validation errors and stop resolution for those locations. A valid registered selection passes its ID and the Session checkout URL, falling back to the selected binding's URL when that portable URL is absent. Preserve the resolver's exact binding checks and selected-checkout precedence, including refusal when the asserted selected repository lacks that binding. Defaults for other repositories remain available through the existing resolver.

A missing binding or file emits an unavailable document location. Alias-free references remain saved.

DocumentTarget in tui/snapshot.rs carries scope, owner, the complete saved `reference: Reference`, and its ResolvedDocument. It exposes `document() -> Option<&RepositoryDocument>` by matching the target kind. Its existing duplicated top-level repository_url field becomes unnecessary because the saved target carries that value.

Native opening requires a document target. Look it up by its saved repository URL and path among current non-Dropped references and compare the freshly built complete DocumentTarget with the painted target. That comparison includes the relationship fields. Then preserve the existing fresh registry, canonical file, containment and native owner checks. Update ApplicationSource.resolve to read the document payload through document(); retain editor reuse identity and its checks unchanged.

### URL actions

Carry the complete saved Reference through painted URL actions. Change render output links to `(usize, Reference, Rect)` and `view::Target::Link` to `Link(Box<Reference>)`. Change both CardRecords.contains_link and CardSnapshot.contains_link to accept `&Reference`. They require a PR, Issue or Link target, a non-Dropped state, exact equality with a current saved reference, and the shared URL policy.

In src/cli/cards.rs activation, preserve the existing frame, fresh viewing scope, native owner and subscription checks. Require both painted and fresh records to contain that complete reference. Then obtain the browser URL through reference_url and keep the existing process/environment boundary. Parsing supplies the browser argument after exact saved-value comparison; it never supplies reference identity. Dropping or changing the painted row prevents activation even when another active URL parses to the same address.

### Rendering

Keep the existing card sections outside references. Replace the Stream reference region with Document, PR, Issue and Link groups in that order, preserving saved order within each group. Render each entry as a linked first line plus its wrapped purpose.

```text
[a] Document Rollout design · Brief · Active
    Define rollout acceptance criteria
[1] PR Workspace configuration · Deliverable · Active
    Merge and deploy to both servers
[2] Issue Platform rollout · Dependency · Fulfilled
    Supply the platform capability used by this work
[3] Link Service dashboard · Background · Active
    Explain the deployment's current operating context
```

The linked first line uses the current CYAN style. Purpose text uses the current default style and plain-text sanitisation, indented four cells. Use the saved name as the primary label. Stop deriving item numbers from URL paths; remove artifact_number once its last caller is removed. Numeric labels identify URL rows; activation uses the existing hyperlink hit regions.

For a document, append ` (REPOSITORY_URL)` after its name when the target URL differs from the viewing Session's explicit checkout URL or when visible documents span several repository URLs. Omit that qualifier only when the one visible document repository exactly matches the viewing Session's known repository. Keep the URL's spelling. Opening uses current checkout resolution; saved ref/commit hints supply no branch label in this row.

Allocate document letters in saved document-group order using DOCUMENT_KEYS. URL row numbers follow PR/Issue/Link order. Preserve the current horizontal clipping rules. Register a document or artifact shortcut when its complete bracketed key fits; a clipped descriptive label keeps that action. Register a URL hit region over its nonempty fitted header text. Vertically omitted entries have no action. Preserve unkeyed behaviour if DOCUMENT_KEYS is exhausted. Filter Dropped entries before layout, shortcut allocation and overflow accounting.

Lay out reference blocks followed by Session artifacts. Each reference block has a header and wrapped purpose with a minimum height of two rows. Purpose uses the existing wrap function at width minus four cells, with a minimum wrap width of one, and four-cell indentation clipped to the available width. At least one purpose row is required even when horizontal clipping hides its text. Artifacts retain their current name grid and remaining alphabetic keys.

Use the following allocation in src/tui/render.rs.

1. Build minimum blocks in group order, then artifact grid rows using existing column sizing. Each reference costs two rows; each artifact grid row costs one. For the existing progress-budget formula, replace reference row count with this total minimum height. Its reservation remains capped at two rows.
2. After progress layout, let S be the remaining slots. If all minimum reference blocks plus one description row per artifact fit in S, choose the existing one-column artifact description mode. Otherwise keep the name grid. Compute minimum height for that chosen sequence.
3. If the sequence exceeds S, reserve one slot for overflow when S is nonzero. Select the longest leading prefix of whole minimum blocks that fits the other slots. Otherwise select every block with no overflow reservation. A reference needs both minimum rows; an artifact grid row is admitted as one block.
4. Reserve minimum heights for every selected block. Give spare slots to wrapped purpose continuations in reference order. Earlier purposes use remaining spare rows while later selected blocks keep their reserved minima. Clip a truncated final purpose continuation with the existing ellipsis policy.
5. Append selected blocks, then the overflow row when entries remain and a reserved slot exists. Count each omitted reference and each artifact in omitted grid rows once. A selected reference with a truncated purpose is visible and contributes no overflow count.

For example, three reference blocks with seven remaining slots select all three minima and give the first purpose one extra row. With five slots, select the first two minima and use the last slot for overflow. A long first purpose cannot remove a later selected header.

## CLI entry points

Add a `streams reference` command family. STREAM uses the existing explicit Stream selection rules. Capture the selected record before constructing intent. List includes every state, including Dropped. Select an existing reference with exactly one of `--url URL` and `--repository-url URL --path PATH`. The document pair is required together.

| Command | Inputs and behaviour |
| --- | --- |
| `streams reference list STREAM` | Read and list all saved references; accept no selector or mutation flags |
| `streams reference add STREAM` | Require kind, target, name, purpose and role; refuse a key already present in the capture |
| `streams reference edit STREAM` | Require a selector and at least one supplied mutation flag; equal supplied values can produce a no-op |
| `streams reference drop STREAM` | Require a selector; set Dropped |
| `streams reference restore STREAM` | Require a selector and --state active or --state fulfilled |
| `streams reference delete STREAM` | Require a selector and --confirm; permanently remove the reference |

Add and edit use single-valued --name TEXT, --purpose TEXT, --role ROLE, --state STATE and --kind KIND. Allowed roles are deliverable, dependency, brief and background; states are active, fulfilled and dropped; kinds are document, pr, issue and link. These values are case-sensitive lowercase wire values, and parsing rejects repeated flags or unknown values. Text uses the model's existing validation and preserves exact strings. Text-valued flags accept leading-hyphen values; list selectors must round-trip those values too.

Add requires --name, --purpose, --role and --kind. Its optional --state defaults to active and accepts all three states. A document requires --repository-url and --path; a web target requires --url. Reject mixed target families and every --target-* flag on add.

Edit preserves unspecified fields, including kind and state. A Dropped row remains Dropped during ordinary edits; reject --state active or fulfilled for that row and direct the user to restore. Other rows accept all three states. Restore permits only active and fulfilled. Drop, restore and delete accept no metadata, recovery or correction flags.

Add and edit accept optional --ref TEXT, --commit TEXT and --source-session TEXT for document targets. Edit also accepts --clear-ref, --clear-commit and --clear-source-session. Add rejects clear flags. Setting and clearing one field together is invalid. Validate these flags against the resulting kind, and reject all recovery flags for web targets.

On edit, selector flags identify the old key. --target-url supplies a new web target. --target-repository-url and --target-path supply a new document target and are required together, including document-to-document corrections. Reject mixed correction families. Correction flags must match the resulting kind. Changing between document and web requires explicit --kind and the complete new target. Changing between pr, issue and link may retain the old URL. Clear document recovery context on a move to web; preserve it on document-to-document corrections unless explicitly changed. Reject invalid combinations before applying any request.

These invocations define representative accepted and rejected combinations. STREAM denotes an existing selected Stream.

```sh
# Accepted
streams reference add STREAM --kind document --repository-url https://example.org/repo --path docs/plan.md --name Plan --purpose 'Guide rollout' --role brief
streams reference add STREAM --kind link --url https://example.org/status --name Status --purpose 'Record outcome' --role background --state fulfilled
streams reference edit STREAM --repository-url https://example.org/repo --path docs/plan.md --target-repository-url https://example.org/repo --target-path docs/accepted.md --purpose 'Accepted rollout plan' --role deliverable --state fulfilled
streams reference edit STREAM --url https://example.org/status --kind issue

# Rejected: partial correction, mixed families, missing role, web recovery field
streams reference edit STREAM --repository-url https://example.org/repo --path docs/plan.md --target-path docs/accepted.md
streams reference add STREAM --kind document --repository-url https://example.org/repo --path docs/plan.md --url https://example.org/status --name Plan --purpose 'Guide rollout' --role brief
streams reference add STREAM --kind link --url https://example.org/status --name Status --purpose 'Record outcome'
streams reference edit STREAM --url https://example.org/status --ref main
streams reference edit STREAM --url https://example.org/status --kind issue --clear-ref
```

### Human output and exact selectors

Keep the saved-text validation unchanged. Define unsafe display characters as Unicode categories Cc, Cf, Zl and Zp. This includes terminal escapes, newlines, invisible formatting and line separators. Escaping changes presentation only; preserve exact captured values, retained input, operation payloads and machine JSON.

Add safeReferenceJson and referenceText in streams.ts. Add matching safe_reference_json and reference_text helpers in cli/references.rs, visible to sibling CLI modules. Safe JSON first uses the ordinary compact JSON serializer, then replaces any remaining unsafe characters with lowercase JSON \uXXXX escapes. Encode supplementary characters as a UTF-16 surrogate pair. Preserve the ResourceKey field order declared above. The output is valid JSON that decodes to the original value. referenceText returns an ordinary string unchanged when it contains no unsafe character; otherwise it returns that string's safe JSON literal, including its quotes.

Use referenceText for plain list names and purposes, reference notices, manager labels and confirmation text, and reference-derived error messages. Use safe JSON for structured retained-input and current-value previews. Apply these encoders to reference-derived scalar values before composing trusted layout text. Keep structured results exact. Diagnostics use safe JSON ResourceKeys. Existing normal-card plain-text sanitisation remains in place. Input values and selected keys retain their original strings.

Reference list JSON is an object with owner_host_id, stream_id and references, including every state. It retains the ordinary machine representation and exact string values. Plain output uses this deterministic block in saved order. Empty output says No references.

```text
NAME
  KIND · ROLE · STATE
  PURPOSE
  --url='EXACT_URL'
```

For a document with safe selector strings, the last line is --repository-url='EXACT_REPOSITORY_URL' --path='EXACT_PATH'. A private shell-quoting helper surrounds each value with single quotes and replaces an embedded apostrophe with the shell sequence `'\''`. Use the flag=value form so literal option markers remain values. Test that POSIX-shell execution and CLI parsing preserve exact values, including apostrophes, leading hyphens, literal `--` and equals signs.

When either document selector string contains an unsafe display character, replace the whole selector line with `selector-json: OBJECT`. OBJECT is safe compact JSON for that exact ResourceKey. This line is data. Decode it with a JSON parser and pass its values as separate process arguments. Keep the existing selector flags; introduce no JSON-input flag. To drop that reference explicitly, save the object after the prefix in selector.json and run the following command.

```sh
python3 -c 'import json,subprocess,sys; k=json.load(sys.stdin); s=["--url="+k["url"]] if k["kind"]=="url" else ["--repository-url="+k["repository_url"],"--path="+k["path"]]; subprocess.run(["streams","reference","drop",sys.argv[1],*s],check=True)' STREAM < selector.json
```

The documented command changes state only when the user runs it; Streams adds no runtime decoder or automatic action. A path containing a newline appears in selector-json with an escaped newline and remains the same exact path when decoded. URL targets already reject unsafe characters through their model policy. List reads the selected Stream and writes no catalog state.

Remove the old document, issue and PR collection flags from StreamFields. Repository configuration flags remain. Initial reference creation uses an explicit reference command after Stream creation. Ordinary Stream field changes and reference mutations use separate commands. Discussion uses the dedicated variant defined above.

## Detailed implementation plan

### Buildable boundaries

Implement the following phases. Every phase ends buildable and testable. The cutover is broad because one saved representation changes its consumers together.

#### Phase 1. Standalone types and validators

Modify only the following files for this phase.

| File | Scope |
| --- | --- |
| src/model.rs | Add Reference, ReferenceValue, RepositoryDocument, target, key, role and state types and validation; introduce shared URL checking with the existing native browser credential restriction |
| src/tui/render.rs | Delegate web_url validation to the shared function; suppress credential-bearing URL controls while leaving current collection rendering intact |
| src/tui/tests.rs | Add username-only and username/password painter rejection cases using current record shapes |
| extensions/stream-checkpoint/wire.ts | Add standalone matching reference types and validators; leave the existing Stream shape intact until phase 2 |
| tests/codec.rs | Add standalone reference/target serde and validation cases |
| tests/enum_values.rs | Add role/state values |
| integrations/herdr-work-cards/tests/record-context.test.mjs | Add standalone TypeScript reference validation cases |

Keep the current Stream collections and operation wiring unchanged in phase 1. The new standalone types have no persisted runtime collection yet. This is preparation for one cutover; it introduces no runtime reader for two formats.

Exit gates are `test:rust:unit`, `test:rust:fast`, `test:herdr`, `check:fmt` and `check:clippy` using the current mise tasks and isolated environments.

#### Phase 2. Atomic schema and consumer cutover

Modify the following production files together. Entries marked Create or Delete are the only new or removed source paths; all remaining entries are Modify.

| Action | Files |
| --- | --- |
| Create | src/operations/references.rs; src/cli/references.rs |
| Delete | src/operations/documents.rs |
| Modify | src/model.rs; src/operations.rs; src/operations/edits.rs |
| Modify | src/agent/record_update.rs; src/agent/preview.rs; src/agent/capture.rs |
| Modify | src/application.rs; src/application/record_update.rs |
| Modify | extensions/stream-checkpoint/wire.ts |
| Modify | extensions/stream-checkpoint/core.ts; extensions/stream-checkpoint/agent.ts; extensions/stream-checkpoint/report-workflow.ts; extensions/stream-checkpoint/reporter.ts; extensions/stream-checkpoint/replay.ts; extensions/stream-checkpoint/streams.ts; extensions/stream-checkpoint/panel.ts; extensions/stream-checkpoint/index.ts |
| Modify | src/cli/args.rs; src/cli/fields.rs; src/cli/mod.rs; src/cli/catalog.rs; src/cli/documents.rs; src/cli/cards.rs; src/cli/cards/source.rs |
| Modify | src/tui/snapshot.rs; src/tui/render.rs; src/tui/view.rs; src/runtime/native_cards/documents.rs |

The paired tests and fixtures are the following closed set.

| Action | Files |
| --- | --- |
| Create | src/cli/references/tests.rs |
| Modify | tests/codec.rs; tests/enum_values.rs; tests/record_updates.rs; tests/record_local.rs; src/operations/edits/tests.rs; tests/agent_preview.rs; tests/agent_capture.rs |
| Modify | src/cli/catalog/tests.rs; src/cli/catalog/tests/apply.rs; tests/cli.rs |
| Modify | tests/stream_checkpoint_test.mjs; tests/stream_outcomes_test.mjs; integrations/herdr-work-cards/tests/record-context.test.mjs |
| Modify | tests/stream_rust_replay_test.mjs; tests/stream_prompt_primitives_test.mjs; tests/stream_prompt_replay_test.mjs; tests/stream_prompt_lab_test.mjs |
| Modify | tests/stream_sync_test.mjs; tests/stream_naming_test.mjs; tests/stream_native_test.mjs |
| Modify | src/cli/cards/source/tests.rs; src/tui/tests.rs; src/tui/fixtures/active-renderer.json; src/tui/fixtures/card-text-renderer.json; src/tui/application.rs |
| Modify | src/runtime/native_cards/tests.rs; src/runtime/native_cards/tests/documents.rs; tests/tui_pty.rs; tests/verification.rs |

This phase wires the pure operation, required Stream field, strict packets, application/preview projections, CLI, UI, inference/reset, receipts and opening/rendering before its exit gate. The file-by-file tables below define each change's scope and reason.

Exit gates are `test:rust:unit`, `test:rust:fast`, `test:pi`, `test:herdr`, `test:replay`, `test:native:rust`, `test:verify`, `check:fmt` and `check:clippy`. Use fresh fixture catalogs throughout. At this boundary the new runtime accepts only the new reference format.

#### Phase 3. Documentation and full acceptance

Modify the existing planned documentation set.

```text
README.md
skills/streams/SKILL.md
skills/streams/references/reporting.md
docs/durable-work-catalog.md
docs/pi-work-card.md
docs/native-herdr-work-cards.md
docs/session-document-additions.md
docs/stream-references.md
docs/stream-reference-implementation.md
integrations/herdr-work-cards/README.md
src/tui/fixtures/README.md
```

Run `mise run -j 1 test`, `test:verify`, `check:fmt` and `check:clippy` with `MISE_AUTO_INSTALL=false` and one workspace-local `CARGO_TARGET_DIR`. Keep the existing disposable-home, fake-native and fixture-model test controls. The record-local IO budget measures apply/save after captured intent exists; CLI selection and capture remain separate costs.

Keep offline activation as an explicit later action requiring user approval. Stop old clients, preserve their catalog offline, initialise a fresh root and compatible sharing destination, and restart clients against it. The implementation and automated tests use existing initialization and test machinery.

### Source changes and reasons

| Action | File | Change and reason |
| --- | --- | --- |
| Modify | `src/model.rs` | Replace the three saved collections with required `references`; define reference, role, state, target and exact resource-key types. Validate nonempty name and purpose, targets and unique keys across URL kinds. Keep document recovery metadata separate from identity and own the shared URL validation policy. |
| Create | `src/operations/references.rs` | Define captured `ReferenceChanges` and its pure result with per-reference outcomes. Compare user requests with the captured Stream revision; apply independent proposals while preserving newer data and automatic Dropped protection. |
| Delete | `src/operations/documents.rs` | Remove the superseded addition-only operation after its callers use the shared operation. |
| Modify | `src/operations.rs` | Export the shared operation and remove the old reference field policies. Keep references outside ordinary Stream field edits; remove the additive-merge function once its last reference caller is removed. |
| Modify | `src/operations/edits.rs` | Remove the three collection-merge branches. Preserve title, goal, lifecycle and Session field reconciliation. |
| Modify | `src/agent/record_update.rs` | Parse a singleton `reference_changes` operation using actor-dispatched raw-property validation before constructing typed intent. Preserve null/omission authority rules and reject old additions or ordinary list replacement. Require the Session input for automatic reference requests only. |
| Modify | `src/agent/capture.rs` | Add the typed per-sample hint callback and merge its result into the frozen catalog input; keep empty contexts and inventory-only capture explicit. |
| Modify | `src/agent/preview.rs` | Apply the shared pure operation and domain projection. Preserve strict capture admission, simulate partial saves and retain per-reference results for refused proposals. |
| Modify | `src/application.rs` | Re-export save_user_references from the private record_update module so CLI callers can reach the shared user save path. |
| Modify | `src/application/record_update.rs` | Save the shared operation through the existing target callback and map its result into the stored outcome object. Add a user-reference save entry point alongside the ordinary user-edit entry point. |
| Modify | `extensions/stream-checkpoint/wire.ts` | Replace the old reference wire types and validate the new collection, required fields, target shapes and exact-key uniqueness. Export and reuse isReferenceText with the specified Rust-equivalent whitespace/scalar contract. Require the new shape without adapting old records. |
| Modify | `extensions/stream-checkpoint/core.ts` | Replace document-addition construction with automaticReferenceRequest for raw deferred proposals and referenceRequest for validated drafts. Use supplied operation IDs, preserve dirty input and keep reference changes separate from generic field edits. |
| Modify | `extensions/stream-checkpoint/agent.ts` | Add model-facing reference proposals, a SessionReportResult runtime type with deferred raw proposal validation, and a dedicated reference discussion-edit variant. Supply known identities, roles, states and purposes in guidance; replace the Session-reset projection with the specified reference fields and newly included top-level checkout/location hints. Preserve the current evidence projection, manual title and goal rules, and the Stream reporting model's metadata-only scope. |
| Modify | `extensions/stream-checkpoint/report-workflow.ts` | Save valid Session progress before validating raw reference proposals through automaticReferenceRequest. Catch later reference-stage errors, surface one warning and return prior savedAny. Preserve original capture and receipts; stop recapture, later inference and retries after errors or unapplied rows. |
| Modify | `extensions/stream-checkpoint/reporter.ts` | Forward the optional notice level through Options and the runReport adapter under the live fence. |
| Modify | `extensions/stream-checkpoint/replay.ts` | Retain complete simulated receipts and add receipt warning text to sample notices, including partial Changes without an accepted add/drop action. Preserve the existing whole-refusal status check. |
| Modify | `extensions/stream-checkpoint/streams.ts` | Own safeReferenceJson and referenceText for human output. Type and validate per-reference outcome rows. Treat unapplied proposals as warnings even when the overall domain outcome is Changes; preserve successful local-save information. |
| Modify | `extensions/stream-checkpoint/panel.ts` | Replace the three reference editors with one References manager showing all states. Extend CardState with lazy retained reference edits. Add explicit Drop, Restore and confirmed Delete controls, labelled target fields and retained unapplied input. |
| Modify | `extensions/stream-checkpoint/index.ts` | Route info/warning notices to the UI. Gate discussion reference changes on an empty retained-input map, retain every unapplied row before continuation checks, and follow the specified partial/whole discussion completion rules. Preserve capture fences and complete receipts. |
| Modify | `src/cli/args.rs` | Add reference command and selector types, remove old collection flags and test argument requirements and mutually exclusive selectors. |
| Modify | `src/cli/fields.rs` | Remove old collection and document-provenance flag translation, together with functions made unused by this change. Preserve ordinary Stream and repository field translation. |
| Create | `src/cli/references.rs` | Implement explicit reference inspection and user commands, shared safe human-output helpers and conditional escaped-JSON selectors. Capture the named Stream, build guarded changes and use the shared application save entry point. |
| Modify | `src/cli/mod.rs` | Register and dispatch the reference command family. Supply capture's typed hint callback using the same `documents::context` producer as live observation; retain the existing read path. |
| Modify | `src/cli/catalog.rs` | Format per-reference diagnostics with safe JSON keys and escaped reference text while preserving accepted-save receipts. A partial conflict must not discard the saved Stream or suppress its normal completion handling. |
| Modify | `src/cli/documents.rs` | Supply established URL/root hints even with empty Stream aliases. Resolve direct document URLs with the specified absent, valid or broken selection rules. Permit explicit defaults with no selected ID, retain broken-selection errors and preserve existing resolver binding checks. |
| Modify | `src/cli/cards.rs` | Validate complete painted and fresh URL references before parsing for the browser. Preserve existing scope, owner, subscription and process guards. |
| Modify | `src/cli/cards/source.rs` | Decode URL-keyed checkout and document hints into the card snapshot and resolve opening against the target's saved URL. Preserve fresh filesystem and checkout revalidation. |
| Modify | `src/tui/snapshot.rs` | Read the unified collection and URL-keyed document locations. Change both contains_link methods to validate complete non-Dropped URL References with exact equality. Keep viewing scope and native owner checks intact. |
| Modify | `src/tui/view.rs` | Carry boxed complete References in Link actions while preserving captured frame and geometry checks. |
| Modify | `src/tui/render.rs` | Present the four resource kinds with relationship fields using the minimum-block and spare-purpose allocation. Filter Dropped entries before overflow accounting and retain existing painted shortcut and hyperlink rules. |
| Modify | `src/runtime/native_cards/documents.rs` | Revalidate documents using their direct repository URL and current visible target. Preserve file containment, editor reuse and current-file opening checks. |

The checkout registry, record-store interface, YAML atomic-save adapter, file transport and child protocol retain their existing contracts. Use existing dependencies and build tasks. Keep document shortcut-map names where they describe navigation.

Use current test harnesses and fakes. Register the new CLI test module inside its source module and retain existing verification models. Generated fixtures must be deterministic.

### Tests and fixtures

Use the existing UI harness and native fakes. Move the six unsafe URL snapshot cases from active-renderer.json and card-text-renderer.json to codec rejection tests. Defensive painter/action tests construct a valid snapshot before injecting an invalid target in memory; adapt the existing file-URL case through this test-only boundary. Username-only and username/password URLs must be rejected consistently by Rust, TypeScript, painting and opening.

| Action | File | Change and reason |
| --- | --- | --- |
| Modify | `tests/codec.rs` | Cover new target shapes, required fields, unknown-field refusal, exact-key uniqueness, document recovery round trips and the unsafe URL cases moved out of renderer snapshots. For names, purposes, revisions, present recovery fields and document locator/path text, accept U+FEFF and valid supplementary characters, reject whitespace-only U+0085 and any NUL, and preserve accepted strings unchanged. Reject malformed-surrogate JSON through decoding. Reject the old Stream reference fields. |
| Modify | `tests/enum_values.rs` | Add exhaustive role and reference-state wire-value checks. |
| Modify | `tests/record_updates.rs` | Replace addition-only tests with shared operation cases for partial acceptance, all roles and states, target correction, ownership, membership revisits, stale edits and exact read/write budgets. |
| Modify | `tests/record_local.rs` | Exercise partial outcomes, landed retries and retries after later edits with reference changes. Keep current record data intact on retry. |
| Modify | `src/operations/edits/tests.rs` | Replace old collection-merge coverage with refusal of generic reference-list edits. Retain unrelated ordinary-edit coverage. |
| Modify | `tests/agent_capture.rs` | Assert one hint callback per typed Stream/Session sample, empty arrays with missing records, no calls for inventory-only capture and retained successful/unavailable hint payloads. |
| Modify | `tests/agent_preview.rs` | Exercise reference proposals and partial outcomes through the real pure preview adapter without live persistence. Preserve exact captured-Stream admission; use an occupied correction target to demonstrate a partial result. |
| Create | `src/cli/references/tests.rs` | Test every CLI flag/value/default, complete document correction, invalid family combinations, repeated flags, explicit add state, Dropped edit restrictions and user editing without a Session. Cover every accepted/rejected invocation above. Test safe JSON and shell selector round trips, apostrophes, ESC, newline, Cf and supplementary-format characters without changing saved text. |
| Modify | `src/cli/catalog/tests.rs` | Retire old collection-flag tests while retaining repository and ordinary field coverage. |
| Modify | `src/cli/catalog/tests/apply.rs` | Assert that partial conflicts return the accepted record and per-reference diagnostics. |
| Modify | `tests/cli.rs` | Add fresh-process reference commands, Dropped inspection/restoration, confirmed deletion, recovery flags and exact selectors in plain/JSON formats. Compare live and diagnostic-capture hints from the same fake-native selection with no portable Session URL, documents or aliases; include unavailable and empty contexts. Assert reference-derived output has no raw unsafe characters while controlled layout newlines remain and JSON preserves exact values. |
| Modify | `tests/stream_checkpoint_test.mjs` | Test inference/reset schemas, URL parity and shared capture. Valid Session changes with malformed reference_proposals or invalid reference values must save progress and retain its receipt/savedAny, warn once and skip later inference/retry. Also test savedAny on receipt stops, retry notice suppression, notice levels through Reporter/UI and escaped names/keys/errors with exact structured receipts. Manager cases cover exact labels, role/state controls, retained keys and reopened drafts. Exercise custom and fallback selection of two equal-metadata rows with distinct keys and prove each edit/drop reaches its selected key. Delete several captured sources before discussion receipts, including a pending deletion, then reopen and inspect/discard every retained-only row and edit each non-null draft. Keep pending additions and the unfinished manual-add form reachable without changing captures or creating references. A dirty A blocks a discussion batch for A/B without saving B or overwriting input. Retain multiple refused additions and a refused deletion; distinguish partial/whole completion. Preserve trimmed/full projection and explicit restoration. |
| Modify | `tests/stream_outcomes_test.mjs` | Cover Changes receipts containing unapplied proposals and local or publication failures after accepted reference changes, and warning/continuation predicates for partial Changes. |
| Modify | `integrations/herdr-work-cards/tests/record-context.test.mjs` | Validate new wire records and refusal of duplicate keys or old collection shapes. Match the Rust text cases for names, purposes, revisions, recovery fields and document locator/path text, including U+FEFF, U+0085, empty/whitespace text, NUL and valid surrogate pairs. Reject lone high/low surrogates, including those embedded in ordinary text. Keep unrelated validators unchanged. |
| Modify | `tests/stream_rust_replay_test.mjs` | Update frozen Stream fixtures and add deterministic reference replay through the real Rust preview, with sandbox-only writes. A refused restoration plus accepted purpose update stays completed and records attention text with no add/drop notice. |
| Modify | `tests/stream_prompt_primitives_test.mjs`, `tests/stream_prompt_replay_test.mjs`, `tests/stream_prompt_lab_test.mjs` | Update frozen record fixtures to `references`; retain existing capture immutability, model-request and no-live-write assertions. Freeze supplied successful/unavailable/empty hint arrays and assert reset and replay preserve them without consulting live location state. |
| Modify | `tests/stream_sync_test.mjs`, `tests/stream_naming_test.mjs`, `tests/stream_native_test.mjs` | Update literal Stream fixtures while preserving the scope of their existing assertions. |
| Modify | `src/cli/cards/source/tests.rs` | Test direct-URL hints with no aliases; an explicit default with no selected directory or a valid plain directory; missing default; broken explicit selection despite a valid default; portable Session URL without an ID; missing files and selected-checkout precedence. |
| Modify | `src/tui/tests.rs` | Cover resource kinds/states, labels, horizontal clipping, minimum-row and purpose allocation, exact overflow, repository qualifiers, absent Session repository and artifact placement. Paint an uppercase-host URL, drop it while a distinct lowercase-host URL stays active, and refuse the original action. Also refuse changed revision, kind or relationship fields. |
| Modify | `src/tui/fixtures/active-renderer.json`, `src/tui/fixtures/card-text-renderer.json` | Update valid record shapes and painted output using the fixture boundary above. |
| Modify | `src/tui/application.rs` | Update its in-module record fixtures and action tests for the shared collection; preserve the production input loop. |
| Modify | `src/runtime/native_cards/tests.rs`, `src/runtime/native_cards/tests/documents.rs` | Update document fixtures and inject target changes or dropping between paint and open, proving no stale native effect occurs. |
| Modify | `tests/tui_pty.rs` | Adapt exact document locations and exercise real terminal navigation with the new reference rows in an isolated home. |
| Modify | `tests/verification.rs` | Replace the old issue-list fixture in existing preservation checks; retain the existing formal models and coverage. |

### Documentation changes

| Action | File | Change and reason |
| --- | --- | --- |
| Modify | `README.md` | Replace old reference flag examples with the new command family and explain roles, states, direct document targets and cleanup meanings. |
| Modify | `skills/streams/SKILL.md` | Teach the shared reference model, exact identifiers, conservative evidence and user-only restoration and deletion paths. |
| Modify | `skills/streams/references/reporting.md` | Describe reference proposals and captured requests separately from Session progress, including partial outcomes and unchanged evidence trimming. |
| Modify | `docs/durable-work-catalog.md` | Update the Stream example and reference identity description to the new collection after implementation; preserve unrelated catalog decisions. |
| Modify | `docs/pi-work-card.md` | Update reference editing and discussion behaviour while retaining the current reporting context and settings policy. |
| Modify | `docs/native-herdr-work-cards.md` | Describe the new visible reference rows and URL-based document identity while preserving native lifecycle and opening boundaries. |
| Modify | `docs/session-document-additions.md` | Mark the addition-only decision superseded when the replacement is implemented, retaining its historical rationale. |
| Modify | `docs/stream-references.md` | Record the implementation status and link the final contract and verification evidence after implementation passes its gates. |
| Modify | `docs/stream-reference-implementation.md` | Update the current-implementation inventory after cutover; retain the approved contract and plan, activation instructions and verification evidence. |
| Modify | `integrations/herdr-work-cards/README.md` | Align the package's reference presentation description with the unified model. |
| Modify | `src/tui/fixtures/README.md` | Explain the new reference fixture coverage and Dropped action exclusion. |

### Offline activation

Stop old clients and obtain approval for a fresh catalog root and, when sharing, a fresh compatible transport destination before using the new binary with a live catalog. Keep the old catalog offline as a backup. Initialise the new catalog through existing commands, restart clients and enrol the wanted work. Activation remains an explicit operator step.

New runtime code reads only the new reference representation.

## Verification plan

Implementation must pass the following checks.

### Behaviour and outside changes

| Area | Required cases |
| --- | --- |
| Shared model | Every role with Active, Fulfilled and Dropped; required name and purpose; all four target kinds; unknown or extra fields rejected |
| Identity | Duplicate exact URL rejected across URL kinds; document ref or commit does not create a new identity; equivalent spellings remain distinct unless the model reuses a supplied key |
| Evidence | Current trimmed and full projection fixtures remain unchanged; attempted file paths alone establish no success; uncertain or tool-only references can be omitted |
| Capture | One Session inference can produce progress and references; the reference request retains the original Stream and Session after progress saves; reset context keeps references and established checkout/location hints |
| Concurrency | A newer A is preserved while independent B saves; unrelated reference and Session progress edits do not reject A; a user capture taken before any Stream change conflicts; outside edits are detected when their current value differs |
| Deletion | A captured mutation cannot update a deleted or recreated reference; an addition captured while absent may rediscover after permanent deletion; no deletion ledger is consulted |
| Authority | Foreign-host writes, changed native identity, moved membership and move-away-and-back are refused; explicit user edits work without any Session |
| Dropped | Automatic restoration is refused per row while an independent change saves; automatic deletion is structurally invalid; metadata edits and stale text reapplication preserve Dropped; explicit restoration can choose Active or Fulfilled |
| Retargeting | An old proposal cannot reach a corrected target; occupied destination keys conflict; overlapping changes are rejected before apply |
| Receipts | Partial results survive local save; partial Changes sets accepted_save and warning output; a landed retry is no change; later current data survives a retry |
| Failure | Session progress remains saved after reference refusal; savedAny records earlier accepted saves before stopping; later Stream inference is skipped; no retry occurs; uncertain persistence or failed publication remains distinct from domain acceptance |
| Management | The current Stream tab offers References; all states are inspectable; Delete requires confirmation; dirty captures and unapplied input survive reopening; reapply requires explicit refresh and restoration choices |
| Presentation | Fulfilled stays eligible for display; Dropped has no normal-card action or overflow count; saved names and relationship text use the specified layout; clipped headers retain actions under the existing painted-prefix/text rules; vertically omitted entries have none; purpose continuation preserves selected minima and exact entry overflow counts; artifacts keep their separate source |
| Opening | Established URLs link without aliases or branches; selected-checkout hints exist before the first document; unavailable locations preserve references; fresh full-target, checkout and canonical-path comparisons prevent stale effects |
| Preview and replay | Strict captured-Stream admission remains; an occupied correction target demonstrates partial preview; whole refused reference results retain diagnostics; the same frozen input and reporting model response yield identical revisions and outcomes; no live catalog or native writes occur |

### Record-local cost assertions

Use existing record-store observation utilities with zero and many unrelated records,
including malformed unrelated records. After opening the store, an automatic
reference save reads its Stream target and named Session plus the fixed
format metadata. It performs no directory inventory and replaces at most
one target record. A user reference save needs no Session dependency read.
A retry that already landed reads the same records and writes nothing.

Count addressed reads, transferred bytes and installed replacements under the
existing [adapter budget](record-local-operations.md#inventory-and-operation-budgets).
### Local commands

Use the current tasks in [`mise.toml`](../mise.toml). Run with
`MISE_AUTO_INSTALL=false` and a consistent workspace-local `CARGO_TARGET_DIR`.
Focused Rust contract checks use the compiler and isolated Git
environment from those tasks.

```sh
mise run test:rust:unit
mise run test:rust:fast
mise run test:pi
mise run test:herdr
mise run test:replay
mise run test:native:rust
mise run test:verify
mise run check:fmt
mise run check:clippy
```

Run the narrowest relevant check after each implementation boundary. Before
integration, run `mise run -j 1 test` and the verification, formatting and Clippy
gates above. Automated checks must use disposable homes and catalogs, fake
native protocols and fixture model responses. A focused pass establishes only
its boundary; live activation still needs the offline step and user approval.
