# Testing policy

Choose the smallest gate that exercises the changed boundary while iterating.
Run its contracts before review and full acceptance before integration. A focused
pass is not evidence that the whole application works. A flaky test is a bug:
fix the cause or report it; a pass on rerun does not close it.

## Choose a gate

| Changed boundary | Iteration check | Additional evidence |
| --- | --- | --- |
| Rust lib/bin logic or CLI grammar | `mise run test:rust:unit` | Relevant `contracts` modules; `test:rust:fast` for the development gate |
| Record saves and captured edits | Filter `operations` or `yaml_store::records` | `record_local`, `record_updates` and `application_records` contracts |
| File transport | Filter `sync::records` | Real-Git CLI and Pi sync acceptance |
| Native effects and cards | `mise run test:rust:fast` | `test:native:rust`, scripted outside changes and PTY contracts |
| Pi helpers and replay | `mise run test:pi` or `mise run test:replay` | Real Rust preview in isolated homes |
| Herdr package support | `mise run test:herdr` | Real Rust native routing where relevant |

Rust task commands set absolute `RUSTC` and `RUSTDOC`; use the same setup for
focused Cargo commands:

```sh
mise exec -- sh -c 'RUSTC=$(rustup which rustc) RUSTDOC=$(rustup which rustdoc) cargo test --locked --test contracts record_updates'
```

Keep fixture Git configuration isolated, as the tasks in `mise.toml` do. Use a
workspace-local `CARGO_TARGET_DIR` consistently for builds and CLI tests.
[Contributing](../CONTRIBUTING.md) lists setup and full gates:

```sh
MISE_AUTO_INSTALL=false mise run -j 1 test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

There is no Python differential suite or separate Temper verification target.
Retired coordinator, import, native-fence and model tests do not establish the
current record-local runtime's correctness.

## Match evidence to the boundary

- Pure operation tests check captured targets, manual fields, membership revisits,
  same-host writes and rejected requests' lack of mutation.
- Memory and YAML record stores exercise the same interface. Disk tests separately
  establish atomic replacement, embedded receipt retries after later edits,
  changed-intent refusal and post-replacement uncertainty.
- Real Git tests establish disjoint-file publication, same-file conflict refusal,
  two-host visibility and offline save/explicit sync behavior. Domain replay and
  per-operation commits are not part of that transport.
- CLI and Pi tests retain exact wire expectations and fresh-process reads.
  Independent expected receipts prevent two implementations agreeing on a mistake.
- Native fakes change actual resources between observation and action, including
  subscription loss, move, close, rename, crash and lost replies. Assert which
  resource received the effect, not only the requested numeric address.
- Replay uses the real Rust preview, poisoned configuration and file snapshots to
  detect writes outside its sandbox.
- Codec, version, unknown-field, symlink, special-file and path-containment checks
  remain boundary evidence, not redundant examples to remove for test count.

Prefer explicit finite cases for known modes. Add generated sequences only when
they supply evidence beyond those cases; do not recreate a disconnected model
or shrinking framework to replace tests of production behavior.

The [record-local adapter budgets](record-local-operations.md#inventory-and-operation-budgets)
count addressed content reads, listings, replacements and bytes. They are not
process-wide card/native budgets: configuration, filesystem metadata, native
inputs, process starts, lock time and failed partial I/O remain outside them.
Exercise real callers before claiming those costs are bounded. Receipt maps and
membership history can grow even when the number of addressed records stays fixed.

Count work, not elapsed time, for semantic budgets. Test an unchanged projection's
lack of effects and an unrelated event's lack of reads. Measure compilation
separately from execution; warm samples and historical test counts are not latency
guarantees. No live credentials, user catalogs or inference belong in automated
acceptance.

## Isolated terminal acceptance

`cargo test --locked --test contracts tui_pty` runs the production
Ratatui/Crossterm application loop in child processes with fresh controlling
PTYs. It is included in the normal gates. Each child uses an empty environment
and disposable home; no real Herdr, editor, browser or user catalog is contacted.
The native `PaneHost` is deterministic. Native targeting, launch and document-file
checks remain separate.

Tests send real keystrokes and resize signals, interpret painted cells with
`vt100`, verify exact document dispatch and clipped/busy-input rejection, and
exercise refresh, close, signal cancellation, startup errors and worker panic.
They compare kernel termios before and after the application, including an
initially raw terminal, and check cleanup escapes. A blocked close checks
restoration before acknowledgement. Child exit is acknowledged only after
inspection because Darwin revokes a terminal on controlling-session exit.
The harness kills its private child process group on assertion failure.

These checks do not prove live-Herdr integration, browser/editor navigation or
restoration after SIGKILL or process abort. `vt100` 0.15 stays test-only because
newer releases conflict with Ratatui 0.29's pinned Unicode-width version.

## Keep path and process tests physical

On macOS, canonical path spelling uses `getattrlist(ATTR_CMN_NAME)` rather than
listing siblings at every component. A reported spelling that differs from the
request must identify the same non-symlink object before replacing that component.
This handles mounted-volume names and case aliases. Unsupported filesystems and
other Unix platforms retain the scan fallback. No filesystem observations are
cached. A Darwin-only test checks the kernel response directly so fallback cannot
make the ABI test pass.

Keep timeout and descendant-termination tests real. A TERM-ignoring descendant
acknowledges that its FIFO is open, then blocks while holding it. Production
termination runs after that handshake. A nonblocking writer open returning
ENXIO observes that the reader is gone; `waitpid` returning ECHILD checks that
the leader was reaped. A positive live-reader control checks the probe itself.
Timeout, partial-output and inherited-pipe contracts remain separate.

Bounded probes check descriptor release instead of waiting for future sentinel
files. A mocked clock cannot prove a descendant is gone. For concurrency tests,
prefer barriers and observable lock/process state over sleeps that guess another
worker has started. OS locks must be tested across real processes.
