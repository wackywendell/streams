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
The dedicated `git_catalog` target retains 5 physical boundary tests:

- two-clone history roundtrips and lost-ack recovery across disk-backed reopen
- partial checkout adoption that retains both histories and fences restart
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

## Check model mutations at the asserted boundary

Positive specifications still run all 4 Temper levels. Negative mutations call
Temper's public model builder and checker directly, with the cascade's unchanged
counter bound of 2. Each mutation must parse and produce an actual counterexample;
a dead transition or parse failure does not satisfy the test.

The resource-effect model's 4 mutations are separate, named tests rather than a
serial loop. Each can run and fail independently. The focused group took 17.77
seconds with the serial direct-checker loop and 7.36 seconds after splitting it.
The full state exploration still runs; these samples do not establish a fixed
speedup. The model remains limited to the proposal described in its source.

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
- process tests wait beyond descendant sentinel delays to detect surviving children

One instrumented two-clone recovery trace recorded 382 Git adapter subprocess
calls. Those calls took 8.41 of the trace's 9.77 seconds. That supports keeping a physical
trace; it does not prove 382 commands or that duration is a lower bound. Further
speedups can target repeated Git observations or a compatible read adapter without
weakening the tests. Re-running policy matrices through Git is not justified.

## Spend time on independent failure evidence

Keep tests for different boundaries even when their inputs look similar:

- real Git crash/recovery traces exercise durable receipts, histories and interrupted adoption that an in-memory fake cannot establish
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
files. The separate local retry property still generates up to 24 saves. Real-Git
traces retain lost replies, intervening saves and interrupted checkout. These checks
complement the matrix; they do not claim exhaustive Git history coverage.

Do not remove cheap constructor or codec examples merely to reduce test count.
Replace selected examples with a stronger matrix when it checks accepted results
and rejected operations' lack of mutation. The native attempt matrix now covers
all 12 phase/operation pairs, including a different replacement resource tuple.
It checks the whole attempt after success or rejection. It replaces selected
transition assertions, not wire roundtrips, schema tests or native ownership tests.
Existing wire, schema, native ownership, uncertainty and replay tests stay intact.

Add independent expectations to fakes instead of treating call-count assertions
as end-to-end evidence.

Keep OS-level timeout and descendant-termination tests real. Their delayed
sentinels and bounded waits test behavior that a mocked clock cannot prove.
For new concurrency tests, prefer explicit barriers and observable lock/process
state over sleeps used only to guess that another worker has started.
