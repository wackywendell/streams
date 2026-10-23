# Testing policy

Choose the smallest gate that exercises the changed boundary while iterating.
Run its contracts before review and full acceptance before a trial or release.
A fast focused run is not evidence that the whole application works.

## Choose a gate

| Changed boundary | Iteration check | Additional evidence before review |
| --- | --- | --- |
| Rust lib/bin logic, CLI grammar or native card state | `mise run test:rust:unit` | Relevant `contracts` modules; use `test:rust:fast` for the complete development gate |
| Rust storage, runtime or adapter contracts | `cargo test --locked --test contracts <module>` through `mise exec`, with the compiler environment from CONTRIBUTING | `mise run test:rust:fast` |
| Save, publication or recovery decisions | Filter `sync::coordinator` in the library/binary gate | Real-Git recovery and CLI contracts |
| Git command construction and parsing | Filter `sync::git_io` in the library/binary gate | Real-Git adapter and recovery checks |
| Catalog protocol model | `cargo test --locked --test verification` through `mise exec` | Implementation recovery tests; model success alone does not establish adapter correctness |
| Pi helpers or replay | `mise run test:pi` or `mise run test:replay` | Replay through the real Rust preview in isolated homes |

Rust commands must use the absolute `RUSTC` and `RUSTDOC` setup in
[CONTRIBUTING](../CONTRIBUTING.md). Keep fixture Git configuration isolated too.
Use a workspace-local `CARGO_TARGET_DIR`, consistently for builds and CLI tests.
For example, filter coordinator tests with:

```sh
mise exec -- sh -c 'RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --lib --bins sync::coordinator'
```

Python/reference measurements below are historical. This Rust-only branch has no
Python implementation or differential gate. Pi, Herdr and replay acceptance remain
separate from the Rust-only measurements.

## Isolated terminal acceptance

`cargo test --locked --test contracts tui_pty` runs the production
Ratatui/Crossterm application loop in child processes with fresh controlling
PTYs. It is included in the normal Rust gates, not ignored or opt-in. Each child
uses an empty environment and disposable home; no real Herdr, editor, browser or
user catalog is contacted. The native `PaneHost` is deterministic. CLI/native
ownership, launch and document-file proofs remain separate boundary tests.

The tests send real keystrokes and resize signals, interpret painted cells with
`vt100`, verify exact document dispatch and clipped/busy-input rejection, and
exercise refresh, explicit close, external signal cancellation, startup errors
and worker panic. They compare kernel termios before and after the application,
including an initially raw terminal, and check cleanup escapes. A blocked close
checks restoration before its acknowledgement. Child exit is acknowledged only
after inspection because Darwin revokes a terminal on controlling-session exit.
The harness kills its private child process group on assertion failure.

These checks do not prove live-Herdr integration, browser/editor navigation or
restoration after SIGKILL or process abort. They verify the terminal loop and its
host boundary rather than repeat catalog/enrollment setup. `vt100` 0.15 stays a
test-only dependency because newer releases conflict with Ratatui 0.29's pinned
Unicode-width version.

## Test-value audit after PTY and membership integration

The audit from `803ea54f` assessed each test family's owned claim, not only the
slowest cases. The follow-up separates these boundaries:

- config values, runtime record schemas and directory-binding schemas use production parsers without files; physical readers and writers still reject malformed evidence without changing its bytes
- protocol permits use the existing memory journal; disk-backed marker, partial-write and reopen proofs remain
- descendant termination runs after a readiness handshake and checks real descriptor closure and leader reaping; timeout/output and inherited-pipe tests remain separate
- history validation owns the relocation matrix; seeded Git tests retain actual local movement, replay, publication, provenance, an independent receiver and duplicate-copy rejection before adoption
- invalid revision requests use a registered checkout without commits and assert specific validation errors; historical reads and cross-process cache tests keep actual commits, dirty files and blob removal
- document and checkout constructors directly test portable-path rules, which differ from runtime path lookup
- accepted-save completion failures keep both CLI paths and their distinct error payloads; fresh YAML stores verify persistence instead of extra CLI show processes

The import preparation failure test still retries the actual import. A fresh
successful import does not establish that the failed catalog remains reusable.
No test is ignored, feature-gated or removed from the default gate.

Before the Rust-only child, socket client and subscription integrations, the
original audit tip `d0b985fb` passed all 594 tests with a warm default
`cargo test --locked`:

| Target | Tests | Harness time |
| --- | --- | --- |
| Library | 193 | 3.15 seconds |
| Binary | 31 | 0.08 seconds |
| Contracts | 360 | 10.48 seconds |
| Dedicated Git boundaries | 3 | 1.70 seconds |
| Protocol verification | 4 | 0.06 seconds |
| Compile-checked documentation | 3 | 0.24 seconds |

Command wall time was 20.35 seconds. Cargo reported 0.46 seconds finishing the
profile; Rustdoc separately spent 0.99 seconds compiling snippets. The one-minute
load average was 12.43. This sample is not a controlled comparison with the
147.73-second run on the previous day, a measurement of current `main`, or a
combined Pi/Herdr/native acceptance run. The few-seconds target remains open.
Further Git observation
work must preserve fresh validation at effect boundaries, not cache mutable
repository state merely to shorten tests. The newer child protocol, socket
transport, request-budget and subscription tests remain in the normal gates
alongside these audited cases.

## Earlier Rust follow-up

The post-`7a0eda62` follow-up separates more production decisions and effects:

- journal schema and completed-import retry decisions run in memory; typed disk-state reopening remains covered
- imported-baseline authority and retired retries use the production coordinator, without moving opaque revision tokens between stores
- existing Git fixtures seed real bootstrap objects and an index instead of repeating unrelated initialization workflows
- offline-create still crosses real CLI init/create dispatch, an unavailable Git remote, fresh YAML readback and committed-file byte comparison
- fenced directory installation has real YAML/directory tests for pre-write failure, a persisted write with a lost acknowledgement, blocked rename and successful movement; none needs Git objects
- signing-profile checks vary raw Git replies and identity timestamps; one real import proves that a `gitdir`-conditional policy change leaves the installed graph fenced
- the three formal engines compile at test-only `opt-level = 1`, preserving assertions, overflow checks and verification bounds
- the positive borrowing doctest is compile-only; both storage implementations run its runtime assertions, and both lifetime-escape compile-fail examples remain

The following sample is from the test-value branch before integration with the
PTY and membership work. It is not a measurement of the combined migration gate.
A warm default `cargo test --locked` passed all 551 tests:

| Target | Tests | Harness time |
| --- | --- | --- |
| Library | 176 | 2.95 seconds |
| Binary | 31 | 0.13 seconds |
| Contracts | 334 | 8.99 seconds |
| Dedicated Git boundaries | 3 | 1.82 seconds |
| Protocol verification | 4 | 0.05 seconds |
| Compile-checked documentation | 3 | 0.45 seconds |

Command wall time was 21.20 seconds after a separate 10.06-second no-run build.
Wall time includes Cargo, executable and Rustdoc startup; Rustdoc also spent
1.05 seconds compiling snippets. The one-minute load average was 15.56. This is
a single sample, not a controlled percentage improvement or a lower bound.
The full default gate still misses the few-seconds target. No test was ignored
or moved behind a feature or a smaller default gate.

The positive four-level reconciliation cascade still explores 7,248 states,
with 5 simulation seeds, 233 transitions and 100 property cases. Optimizing the
formal engines reduced one warm cascade sample from 4.28 to 1.52 seconds without
reducing that coverage. Memory tests do not establish disk-crash durability.

## Measure compilation separately

Targeted measurements on macOS at `0dbcae45`, using this effort's own target
directory, isolated fixture Git configuration and 2 Cargo build jobs:

| Sample | Observed time |
| --- | --- |
| Cold lib/bin/contracts build, `--no-run` | 201.6 seconds wall; Cargo reported 199 seconds compiling |
| Warm `test:rust:fast` | 56.6 seconds wall; 94 lib/bin tests ran in 2.3 seconds and 318 contracts in 46.9 seconds |
| Warm `test:rust:unit` | 5.4 seconds wall; the same 94 lib/bin tests ran in 1.8 seconds |
| Old Rust lost-ack property, 16 generated cases | 173.4 seconds test execution; 178.6 seconds wall |
| Intermediate real-Git lost-ack matrix, 8 modes, serial | 69.2 seconds test execution; 74.0 seconds wall |
| Memory-only lost-ack matrix, 8 modes, serial | 0.08 seconds test execution |
| All 25 memory-only coordinator tests, including 32 generated sequences | About 0.6 to 0.8 seconds test execution |
| Python lost-push-reply restart trace | 5.0 seconds |
| Python separate-process Git serialization | 3.1 seconds |
| Python signing-failure safety check | 1.6 seconds |
| Python standalone-jj isolation check | 5.2 seconds |
| 13 Python model examples | Each below the sample's 1-millisecond reporting precision |

These are samples, not latency budgets or a new full-acceptance benchmark.
Host contention can change them. The unit-only task shortens focused iterations;
it does not itself reduce full-acceptance coverage or runtime. The Git matrix
reduced its targeted execution sample by about 60%; full acceptance was not rerun.
A cold unit build
still compiles dev dependencies. Do not confuse that cost with test execution.
Global Git isolation and batched blob observation were already present in
`1ae29d36`; this effort does not count them as new savings.

## Save and publication tests without I/O

`src/sync/coordinator.rs` owns save, fetch, scheduling, confirmation and adoption
decisions. Its `Repository` interface describes catalog effects, not Git command
strings. Production implements it inside the existing locked YAML transaction.
Tests supply immutable histories, observed remote tips, scripted failures and an
in-memory journal. They exercise the production coordinator, publication permits,
replay and history validation rather than a second coordination model.

The initial split replaced 23 real-Git cases with 25 coordinator tests. Later
passes moved the remaining recovery matrices and common backend laws there too.
Each request constructs fresh coordinator and state wrappers over the same memory
journal. This checks decisions across reopen, not filesystem crash durability.
Real-Git tests retain that separate evidence.

The `git_io` tests intercept every command without starting Git. They check push
arguments, unknown outcomes, branch-specific fetches, target validation, signing
failure without unsigned fallback, ancestry and batched blob observations.
The dedicated `git_catalog` target retains 3 physical boundary tests:

- rejection of actual dirty, ignored and unsupported Git histories
- initialization identity, branch selection and no-clobber checks
- partial local saves at add, commit creation and branch update, with durable YAML/index evidence and restart fencing

Publication-fence persistence is tested directly through `State::fence_adoption`
and the real YAML journal, without Git. The test physically unlinks the marker,
restores it byte-for-byte, checks that identical fences are not rewritten, and
reopens a fresh store to verify tips, operations and confirmation time. Coordinator
memory tests separately establish that adoption/clear failures invoke this recovery
and block later operations. One warm file-boundary sample took 0.040 seconds,
replacing a two-catalog publication trace that took 13.9 seconds in a concurrent
profile. These are not controlled benchmarks or evidence of hardware-crash survival.

The former two-catalog lost-reply and partial-adoption traces are split by claim.
Coordinator tests cover projection without adoption, later saves after an uncertain
push, original provenance, retained histories and fencing after checkout errors.
The coordinator treats checkout as an opaque effect; its fake need not simulate
partial filesystem changes. The library adapter tests in `src/sync/git/tests.rs`
and its `membership` module seed histories with `fast-import`. One uses a real ref
lock to cause partial checkout; another checks retained tips and successful
checkout. Membership tests exercise local save, the production isolated-index
candidate builder, actual publication and a fresh receiving catalog. They verify
original intent/provenance, canonical file movement and the publisher's unchanged
index, worktree and tip. A published duplicate copy is rejected before adoption.
Other invalid relocation shapes run through production history validation without
Git. The receiver must fetch pushed objects rather than inspect objects already
present in the publisher. These tests avoid catalog workflows for incidental setup.
These replace the earlier 382-subprocess application trace, not real Git itself.

Real Git also remains in storage, import, inspection, revision and CLI contracts.
The dedicated target's test count is not the total number of tests that start Git.

We retain the Git CLI behind this typed boundary. Replacing it with `git2` or `gix`
would still require this separation for memory-only tests. It would also introduce
compatibility work for existing signing, credential and SSH behavior.

This does not make the whole Rust suite memory-only. After the initial split,
318 contract tests took 49 seconds and 19 real-Git tests took 55 seconds with
4 test threads. Compilation took another 15 seconds. These historical samples
are not a controlled full-acceptance comparison. Filesystem durability, process
termination, Temper verification and Pi/Herdr acceptance remain separate checks.

## Further removal of incidental I/O

Retry and bookkeeping-failure laws use the production `LocalCatalog` with shared
`MemoryStore` handles instead of repeated YAML writes. Common backend laws also
run against the production coordinator's memory repository. A physical YAML and
Git reopen trace still checks idempotency, authority and changed-intent rejection.
The storage suites retain adapter conformance and disk-backed journal recovery.

`src/cli/catalog.rs` accepts `CatalogBackend` directly. Selection, filtering,
updates, archive/restore and receipt handling no longer require CLI processes or
disk catalogs in their semantic tests. Publication-failure tests script the
backend outcome, not the receipt decisions. Real CLI tests retain argument,
environment, input and dispatcher checks, including archive/restore wiring.
An offline-create CLI smoke test retains the real configuration, accepted save,
publication-warning, subsequent read and explicit-sync boundary.

Captured updates also use the backend-facing command handler. Memory tests check
original operation identity, unchanged retry receipts, source rejection and stale
bases against both the catalog snapshot and complete journal contents. The CLI
smoke retains unenrolled context, stdin, wrong-source record-byte preservation and
persisted read-back checks. It no longer repeats retry and stale-base policy.

Conversation discovery separates counting and identity policy from traversal.
The 10,000-entry limit is tested with synthetic observations, not 10,001 files.
An exact-locator filesystem test still checks the discovery bypass.
Registry schema and selection policy likewise run without filesystem fixtures;
registration and resolution still validate live physical directories.

On macOS, canonical path spelling uses `getattrlist(ATTR_CMN_NAME)` rather than
reading every sibling at every component. A reported name that differs from the
request must identify the same non-symlink object before replacing that component.
This handles mounted-volume names as well as case aliases. Unsupported filesystems
and other Unix platforms retain the scan fallback. No filesystem observations are
cached. A Darwin-only test checks the kernel response directly, so silent fallback
cannot make the native ABI test pass. The 29 registry tests took about 0.6 to
0.8 seconds in focused samples.

Import completion separates journal sequencing from physical graph installation.
Memory tests cover config, strategy-marker and fence-clear failures, plus a
changed configuration and successful completion. Production still reads the
current configuration after writing it. Physical tests retain candidate ownership,
rename failure, final-path signing, byte preservation and fence reassertion after
an actual unlink. A memory journal cannot establish those filesystem facts.

Import preparation now captures a private `Source`: decoded snapshot, exact record
bytes and bookkeeping presence/contents. It depends on the root and locked storage
transaction, not a Git-bearing catalog. The prepared import carries that coherent
value through installation. Direct YAML-backed tests check raw-byte drift with an
unchanged decoded catalog; content, addition and removal of each bookkeeping file;
and corrupt evidence. Only one preparation interleaving needs a Git runner: mutate
a record during `commit-tree`, require the precise drift error, then check that no
candidate, Git installation or save fence remains.

The CLI import smoke now uses 3 commands: initialize, reject a strategy change
without `--adopt`, then accept explicit adoption and return a valid baseline ID.
It checks that rejection preserves the local marker and that adoption installs the
Git marker without changing the configured host. Authority, retired retries and
completed-import identity remain in the direct adoption tests rather than being
repeated through CLI updates and readbacks.

## Check model mutations at the asserted boundary

Positive specifications still run all 4 Temper levels, including exhaustive L1
checking and dead-transition analysis. Negative mutations build the same Temper
model with the unchanged counter bound of 2, then use Stateright's BFS checker to
stop on a reachable `Invariants` counterexample. They do not need the remainder of
the graph or a second traversal for dead transitions. No timeout, depth or state
limit substitutes for a witness. Parse failures still fail the test, and an
unmodified protocol is a negative control that must produce no counterexample.
Stateright 0.31 is an explicit test dependency using the version already in the
lockfile through Temper; no dependency versions changed.

The resource-effect model's 4 mutations remain separate, named tests. They once
ran full L1 checks in a serial loop (17.77 seconds in one sample), then as separate
tests (7.36 seconds for the group in another). They now search only for the claimed
invariant witness. Positive full-cascade verification remains unchanged, and the
model is still limited to the proposal described in its source.

## Focused boundary follow-up sample

At `f5ca168f`, a separate no-run build took 20.36 seconds. The subsequent warm
default `cargo test --locked` passed all 542 tests:

| Target | Tests | Harness time |
| --- | --- | --- |
| Library | 165 | 3.92 seconds |
| Binary | 31 | 0.15 seconds |
| Contracts | 336 | 13.75 seconds |
| Dedicated Git boundaries | 3 | 1.71 seconds |
| Protocol verification | 4 | 0.06 seconds |
| Documentation | 3 | 0.03 + 0.22 seconds |

Command wall time was 28.19 seconds. Cargo startup, harness startup and Rustdoc
snippet compilation are outside the individual harness figures. Host load was not
controlled; these figures do not establish a fixed percentage improvement.

In focused samples, all 4 source-observer tests took 0.05 seconds after removing
durable writes from their setup. The 4 resource-effect mutant checks took about
0.62–0.71 seconds each, while their unchanged positive full cascade took 3.16
seconds. The earlier concurrent profile put those mutants at 6–10 seconds each.
They still use real generated models, not canned witness traces.

Argon reviews covered the fence, source, native adapter and witness boundaries.
The adapter review exposed missing production candidate/retention/checkout and
fresh-receiver coverage; focused native tests now cover those paths. The final
review found no material issues. Strict Clippy, warning-denied docs and formatting
passed. Pi (279), Herdr boundaries (43), replay (3) and Rust native routing (18)
also passed, with no skips.

The default Rust gate still exceeds the few-seconds goal. Contracts remain the
largest group; passing this sample is not evidence that every remaining test has
the right boundary or justified cost. No tests were ignored or moved out of the
default gate.

## Rust-only follow-up execution sample

The follow-up branches from the accepted Rust migration bookmark. A warm default
`cargo test --locked` passed 531 tests after a separate no-run build:

| Target | Tests | Harness time |
| --- | --- | --- |
| Library and binary | 183 | 2.26 seconds |
| Contracts | 336 | 18.36 seconds |
| Dedicated Git boundaries | 6 | 13.34 seconds |
| Protocol verification | 3 | 0.05 seconds |
| Documentation | 3 | 0.17 seconds |

Command wall time was 40.84 seconds, including Cargo and Rustdoc startup and
snippet compilation. The 333-contract baseline took 27.53 seconds before the
counterexample split. Both samples used default test concurrency, but host load
was not controlled. No test was ignored or moved out of the default gate.

With `PATH` pointing to a nonexistent directory, all 4 catalog-command unit tests
passed in 0.01 seconds and all 43 coordinator tests passed in 0.83 seconds. These
checks need no external programs on `PATH`. They do not prove disk crash
durability. The physical Git, import and storage tests remain unchanged,
apart from narrowing the CLI smoke described above.

## Pre-integration warm execution samples

These samples come from the separate test-value workspace before integration,
not this branch's combined native-reconciliation and Pi acceptance gates.
Two full default `cargo test --locked` runs passed without recompiling project
sources. Run A used `66d7a461`; run B used `90b40a19`, which adds one direct native
macOS lookup assertion but changes no production behavior.

| Target | Tests in run B | Run A | Run B |
| --- | --- | --- | --- |
| Library and binary | 162 | 1.73 seconds | 2.77 seconds |
| Contracts | 314 | 13.25 seconds | 18.74 seconds |
| Dedicated Git boundaries | 4 | 11.86 seconds | 23.38 seconds |
| Protocol verification | 3 | 0.07 seconds | 0.09 seconds |
| Documentation | 3 | 0.17 seconds | 0.21 seconds |
| Command wall time | 486 total | 33.29 seconds | 50.72 seconds |

Harness times sum to 27.08 and 45.19 seconds respectively. Run A's preceding build
took 5.30 seconds according to Cargo and is excluded. Cargo, harness and Rustdoc
startup are part of command wall time; doctests also compiler-check their snippets.
Host load varied sharply, including an interrupted build during a load average
above 300 on 10 logical CPUs. These are samples, not a performance guarantee or a
controlled percentage improvement.

The few-seconds target is met for lib/bin execution, not for default `cargo test`.
No tests were hidden behind features, `#[ignore]` or a smaller default gate.
Remaining slow tests must justify their cost with physical boundary evidence:

- Git histories and transport require real Git objects, refs, fetches and pushes
- import installation requires actual directory movement, file bytes and recovery fences
- storage and runtime contracts exercise locks, symlinks, special files and durable replacement
- process tests observe FIFO descriptor teardown after a real timeout to detect surviving descendants

One instrumented two-clone recovery trace recorded 382 Git adapter subprocess
calls. Those calls took 8.41 of the trace's 9.77 seconds. This identified adapter
work to test directly, not a reason to retain the compound application trace.
Neither 382 commands nor that duration was a lower bound. The focused replacements
exercise real Git construction, transport and partial checkout; the surrounding
recovery policy belongs in coordinator tests.

## Spend time on independent failure evidence

Keep tests for different boundaries even when their inputs look similar:

- real Git adapter tests establish object, ref, index and checkout behavior; physical journal/storage tests establish byte preservation and fresh-store reopening
- real CLI and Pi boundary tests check shared files and exact wire bytes; independent expected receipts prevent agreement on the same mistake
- native fakes verify that durable pending evidence precedes launch, closed evidence precedes close, and uncertainty never authorizes another effect
- exact ownership and unrelated-pane checks prevent destructive actions against foreign resources
- replay tests use real Rust preview, poisoned configuration and file snapshots to detect writes outside the sandbox
- Temper positive checks and deliberately broken models show that the model checker can find relevant counterexamples

Prefer a complete transition matrix for a small finite state machine. Keep
property tests for generated histories, retries and operation identities. Generated
examples should add sequence diversity, not substitute for enumerating known modes.
The memory-only lost-ack tests enumerate all 8 combinations of remote acceptance,
a competing remote save and interrupted adoption. Each mode reopens for the
sequence `[0, 1, 0, 2]`: fresh, fresh, retry an earlier save, then fresh again.
Every mode checks changed-intent rejection without changes to records, HEAD,
receipts, publication evidence or retained histories. Interrupted adoption remains
fenced while the other producer sees every accepted save.

The coordinator also runs 32 generated retry sequences with shrinking. Filesystem
failure persistence is disabled so these tests do not read or write regression
files. The separate local retry property still generates up to 24 saves. Lost
replies and intervening saves run through production coordination in memory;
physical tests check Git and journal effects separately. These checks do not
establish hardware-crash durability or exhaustive Git history coverage.

Do not remove cheap constructor or codec examples merely to reduce test count.
Replace selected examples with a stronger matrix when it checks accepted results
and rejected operations' lack of mutation. The native attempt matrix now covers
all 12 phase/operation pairs, including a different replacement resource tuple.
It checks the whole attempt after success or rejection. It replaces selected
transition assertions, not wire roundtrips, schema tests or native ownership tests.
Existing wire, schema, native ownership, uncertainty and replay tests stay intact.

Add independent expectations to fakes instead of treating call-count assertions
as end-to-end evidence.

Keep OS-level timeout and descendant-termination tests real. A TERM-ignoring
descendant acknowledges that its FIFO is open, then blocks while holding it.
After that handshake, `src/process/tests.rs` invokes production termination.
A nonblocking writer open returning ENXIO observes that the reader is gone;
`waitpid` returning ECHILD checks that the leader was reaped. A positive
live-reader control checks the probe itself. Timeout, partial-output and
inherited-pipe contracts remain separate. Bounded probes check descriptor
release instead of future-sentinel sleeps. A mocked clock alone cannot prove
a descendant is gone. For new concurrency tests, prefer explicit barriers and
observable lock/process state over sleeps that guess another worker has started.
