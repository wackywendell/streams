# ADR: Rust runtime with a TypeScript Pi boundary

Status: implemented and activated. The filename preserves existing links; embedded
Temper execution and the disconnected old protocol model are retired.
[Record-local operations](record-local-operations.md) owns storage and transport;
[observe and act](native-observe-and-act.md) owns native effects.
The [Temper Cloud backend](catalog-temper-cloud-sync.md) remains a proposal.

## One runtime, one in-process boundary

Rust owns the CLI, typed records, pure operations, YAML persistence, Git file
transport, checkout/workspace runtime, Herdr and cmux adapters and terminal card.
TypeScript remains in `extensions/stream-checkpoint/` because only code inside
Pi can observe the live conversation tree, register tools and commands, render
the inline editor and call Pi's session-name setter.

The split removes the Python CLI and Node-to-Python advisory-lock helper without
keeping a fallback runtime. Reporting and native requests share one executable:
an explicit absolute `STREAMS_BIN_PATH`, or this checkout's debug Cargo target.
A packaged extension needs an explicit matching executable. Missing or invalid
builds fail rather than selecting an installed command from `PATH`.

A Rust child cannot observe a branch switch while inference is running inside Pi.
TypeScript therefore retains conversation lifetime, cancellation and active-branch
prefix admission. An appended conversation can accept an older captured prefix;
rewind, replacement and branch changes cannot. Versioned requests carry the
original target, operation ID, catalog source and captured fields. Rust checks
record-local intent and manual protection; neither side substitutes a fresh base
after inference.

## Interfaces must be implementable

The domain depends on no filesystem, Git or frontend. Pure operations receive
their typed inputs and produce one target decision. Application code uses the
`RecordStore`, `RecordReader` and `RecordWriter` interfaces; YAML and memory
implementations exercise the same contracts. Receipts and the current record share
one target envelope and one atomic replacement. A possible write is reported as
uncertain, not disguised as a definite refusal.

Sync transports host-partitioned files and has its own outcome. It does not control
local save admission or replay domain operations. Domain, local and sharing
results stay distinct, so an accepted save and a failed publication remain
representable together. None and Git need no background service. A future service
backend must fit the current record contract rather than revive the retired
catalog-wide replay interface.

Frontend selection belongs at the application boundary. Explicit `--frontend`
selection wins; `auto` uses caller context and refuses ambiguity rather than trying
adapters until one succeeds. Herdr, cmux and future frontends implement native
capabilities behind interfaces, not storage or membership policy. A native
transport executes actions and returns checked replies; it does not write records.

Herdr supplies workspace markers, pane topology and events. CMUX does not fabricate
those capabilities: exact record inspection can work without them, and unsupported
workspace-only selection or multi-Session opening refuses explicitly. Lack of a
native observation is unknown state, not absence.

## Keep native decisions separate from effects

The driver reads concerned records, takes at most one current snapshot and passes
typed values to pure planning. Executors perform only their own action types.
Catalog saves and native effects are not one transaction. Checked action replies
can supply handles to later actions in the same pass; they do not create a new
snapshot or durable effect permit.

Reopening discovers active same-host Sessions only for the explicit user command.
It uses saved exact transcript locators, not transcript-directory searches.
Already-observed running Sessions stay where they are. There is no saved layout,
placement record, native launch journal or proof-state machine. Failures warn and
end the affected pass; duplicate avoidance is best effort. The subscription-ended
check and accepted within-pass restart race are documented in
[observe and act](native-observe-and-act.md#bound-targeting-to-the-current-pass).

Enrollment passes admitted records and parsed intent to a record-only helper.
It can save a Stream before its Session and retain that first save if later work
fails. Directory selection and marker projection are completion steps, not proof
that records were rolled back. See the [enrollment boundary](native-enrollment-boundary.md).

## Paths, identity and inspection

Session IDs derive from immutable host, harness and native conversation identity.
Membership moves write only that Session, preserving work, portable checkout URL
and host-local directory intent. No alias, old-layout decoder or catalog search
stands in for an exact-key read. The explicit alpha reset replaces migration.

Capture reads durable local records without a save, network request or native
effect. Frozen preview takes only supplied records and intent and runs the same
pure operations as production. It cannot open a backend or mint an effect permit.
The [frozen-request decision](pi-prompt-replay.md) owns private artifact handling.

Checkout selection is explicit and host-local. Canonical path and regular-file
checks prevent traversal and escaping symlinks; macOS path spelling support does
not become a legacy catalog fallback. The revision-extraction library uses a
private user-owned cache, rejects links and nonregular entries, resolves named
revisions again on request and never switches a checkout's branch. The current
native card resolves working files only; document provenance does not imply an
active source/revision chooser.

## Test the production boundaries

The Rust migration used differential traces and extracted fixtures before
retiring Python and the duplicate TypeScript coordinator. Those historical gates
are not current build dependencies or compatibility requirements.

Current tests exercise record stores, captured reports, receipts, file transport,
path checks, native fakes with outside changes, real OS locks, process termination
and isolated PTYs. Pi/replay acceptance runs the real Rust CLI in disposable homes.
The removed formal model described old save/publication markers and had no
production caller; its verification was not evidence for the current runtime.
No replacement model is introduced.

An independent memory store demonstrates record-interface substitutability, not
database crash durability. Native fakes and PTYs do not establish live-platform
behavior. Each frontend needs its own transport and lifecycle acceptance.
See the [testing policy](testing.md).

### TUI Unicode choice

Use ratatui's standard Unicode layout rather than reproduce every JavaScript
width rule. Five trailing U+0D4E Malayalam stress cases differ: Rust treats the
final grapheme as zero-width and JavaScript counts one column. The original
JavaScript fixtures remain historical evidence and supply input snapshots;
`card-text-renderer.json` records the current Rust layout. Tests assert every
input's expected lines, visible shortcuts, document targets and links. This avoids
a custom compatibility patch without waiving rendering or navigation checks.
The [fixture notes](../src/tui/fixtures/README.md) record the cases.
