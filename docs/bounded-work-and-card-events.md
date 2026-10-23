# ADR: Bounded, event-driven work in every Streams process

Status: proposed. The poll interval stopgap is applied locally; nothing else here is implemented. This builds on the [native Herdr ADR](native-herdr-work-cards.md) and the [native reconciliation ADR](native-context-reconciliation.md).

## Problem

Simple Streams operations take tens of seconds and keep the machine busy while nothing changes. One `streams native check` takes 26–32 seconds. It starts the `herdr` CLI 27 times, including 14 calls to `status --json` and 5 to `pane current`. On this machine, starting any process costs 0.25–0.5 seconds; `streams --help` takes 0.26 seconds. In contrast, reading all 102 catalog files in one process takes 4 ms. The cost comes from starting processes and from each layer observing the same state again, not from the amount of data.

Work cards make this continuous. A card refreshes on a timer (`pane_poll_interval_ms`, measured from the end of the previous refresh). Each refresh runs the full attested observation above, and the card marks itself "Refreshing…" before every read. A 30-second read on a 1–5 second timer leaves the card nearly always refreshing, while every open card keeps starting Herdr processes.

The Pi extension has the same shape. It starts a new Streams CLI process for each operation. When a native update fails, it retries on a 15-second timer, up to four times.

Some reads also scale with data that has nothing to do with them. Finding one Session loads the whole catalog. Building a card's document list checks every document's file in its checkout.

## Decision

### Fixed cost per operation

Every operation should have a declared budget of process starts, Herdr requests, files read and fsyncs. A single-Session budget must not grow with unrelated Streams, Sessions or panes. Capture reads Sessions named by the workspace's Pi panes; open/resume read the saved arrangement's Sessions and skip changed membership. Stream-member listing and title resolution are explicit user discovery, like `list`; they never run on an event or report path. `check` and `sync` may also enumerate records. Tests should enforce budgets by counting calls through the existing fake Herdr and process runner, not by timing; host load makes timings unreliable. A new loop that performs I/O for each item then needs a deliberate budget change, rather than appearing as a slowdown later.

Proposed budgets:

| Operation | Process starts | Herdr requests | Files read |
| --- | --- | --- | --- |
| Card update after a record change | 0 | 0 | the changed files only |
| Card update after a native event | 0 | at most 1 snapshot | 0 |
| Pi lifecycle event, through the Pi child | 0 | at most 1 | the card's keyed files |
| One-off CLI `check` or `observe` | the CLI itself | 1 snapshot | the card's keyed files |
| Card closes itself | 0 | 0 | 0 |
| `open`, `close`, `focus` | the CLI itself | 1 snapshot, the effect and its confirmation | keyed files and the evidence file |

### Every process is event-driven

These rules apply to every Streams process: the Pi extension, the Rust child it starts, cards and one-off CLI commands. A process does work only in response to an event: a Pi lifecycle event, a user command, a record file change, or a Herdr event. No process polls for changes or retries local work on a timer. A failed local operation is retried when an event shows that its inputs may have changed. For example, an unready enrollment is retried when its Session file is written.

Timers remain allowed for three things:

- one-shot interface expiry, such as clearing a status message
- backoff between attempts at a remote model call
- deadlines that bound a single operation

The Pi extension's own handlers should do no I/O beyond one message to its Rust child. Today's 15-second native retry timer should be removed.

### Event subscriptions are required and filtered early

A process that depends on events must have its subscriptions. If it cannot establish them, it should fail visibly rather than fall back to polling. This applies when Herdr lacks `events.subscribe`, its protocol version is unknown, or the file watcher cannot start. When a subscription drops, the process should resubscribe once and then do one full read. If resubscribing fails, it should report an error and stop updating, not degrade to polling.

Irrelevant events should cost almost nothing:

- Subscribe only to the event types the process uses.
- Watch only the directories holding its own files.
- Make the first step for each event a comparison against a small fixed set of IDs or file names. Decode only the type, pane, workspace or file name for that check; decode the rest of an event only after it matches.
- Read and re-render only after the check matches.

Herdr filters structural events by type only, so the ID comparison happens in the process. `pane.agent_status_changed` can be filtered by pane in Herdr, and should be if a process ever needs it.

To avoid missing a change between a process's first read and its subscription, it should subscribe before that first read.

### Observe once and pass the result down

Each process should keep one connection to its active native frontend, behind that frontend's interface. For Herdr work cards, each operation should take one snapshot and derive the caller, workspace, agent and owner from it. Lower layers should receive that observation rather than reconnecting or observing again. Streams should use Herdr's socket API directly instead of the CLI. It should check the protocol version when it connects, and refuse an unknown version rather than guessing.

### Separate display from effects

Displaying state and acting on it need different freshness. A card displays state kept current by events. An effect — open, close, focus or capture — takes one fresh snapshot under the native workspace lock, compares it with what it expects, then acts. This keeps every existing fence and its uncertainty handling. A card snapshot already grants no save or effect authority, and that remains true.

### Look records up by key

A card knows its conversation identity from its launch context. Its directly addressed Session supplies the current Stream ID; launch-time membership is not fixed identity. It should read only its own records:

- the Stream file
- the Session file
- the host's checkout registry
- the host's directory binding for its Session

Each of these is a direct lookup. The rule is a fixed set of keyed files, not a file count. These four should stay separate because they have different owners and change at different rates:

- The Session and Stream records are portable catalog records.
- The checkout registry is host-wide and rarely changes.
- The directory binding is host-local and per Session.

Moving the binding into the Session record would put one host's paths into a portable record. Merging bindings into the registry would make every directory selection rewrite a host-wide file under a host-wide lock, and wake every card on the host. Either merge would trade one cheap keyed read for a coupling cost.

The [record-local operations ADR](record-local-operations.md) replaces the earlier lookup-only proposal. The records themselves should supply each operation's inputs, with exactly one writing host per Stream and its Sessions. Ordinary reads and saves should read their named records, apply their input as a pure function, and write the result. They must not scan the catalog, replay history or consult a derived cache, including at Pi child startup. Git sync alone consults history for commit/pull/push. It does not merge domain records; same-file concurrent changes fail.

That ADR inventories current operation paths and defines host-partitioned records, deterministic Session location, host-local relationships and explicit deploy migration. It supersedes full-catalog save validation, authority replay, native-ID indexes and scan fallback. Session.stream_id alone records membership. Moves atomically replace one Session file with its embedded retry receipt; there is no roster or multi-file transaction. Captured native panes and saved arrangements supply bounded inventory, not ordinary single-Session context.

### Event-driven cards

A card should update only when something it displays may have changed. Events say what to re-read; they are not data to apply. When an event arrives, the card should drain any further queued events, then do one read. That read is the changed files, or one Herdr snapshot for a native event. A burst of changes therefore produces one update.

| What the card shows or depends on | What changes it | Signal |
| --- | --- | --- |
| Session title, summary, status and Stream membership | Pi reports, edits, sync, membership moves | Session file |
| Stream title, summary, status and documents | Stream reports, edits, sync | Stream file |
| Checkout and directory for each repository | Checkout registration, directory selection | Registry and directory binding files |
| Owner's pane, tab and workspace | Pane or tab moved | Herdr `pane.moved`, `tab.moved` |
| Whether the owner still exists | Pane closed or exited, tab or workspace closed | `pane.closed`, `pane.exited`, `tab.closed`, `workspace.closed`; the card closes itself |
| Which conversation the owner pane runs | Fork or new session in the same pane | `pane.updated`, `pane.agent_detected`; compare the agent session |
| The card's own placement | User moves the card | `pane.moved` for the card's pane |
| Terminal size and keys | User | Terminal input, already event-driven |

The card should ignore focus, output, agent status, layout, and tab or workspace renames. It does not display them.

Saves replace files by atomic rename, so the card should watch the containing directories and filter by file name. An unrelated Session's save in the same directory costs a name comparison, not a read. If the Session's membership changes, the card should switch to watching the new Stream's file. After the explicit record migration, a Session has one identity-based path that does not change with membership. New readers require the completed format, with no legacy path watches.

The card should keep showing the last good state during a read. It should show a notice only when a read fails, or takes long enough for a user to notice.

Apart from closing itself, the card only observes. It never changes membership or other catalog records. The Pi child handles membership, as described below.

No channel from the CLI or Pi child to the card is needed. File watching sees every writer — the CLI, the Pi child, sync and manual edits — without a list of which pane shows which record.

The card should check document availability when it opens, when its records change, and when a document is opened. It should not watch document files, so availability may be stale between those points. Opening a document checks the file again, so a stale display cannot open a missing file.

### Closing a card

When the user presses `q`, or the owner pane closes, the card should close itself:

1. Cancel any in-flight read. Reads have no effects.
2. Exit without writing evidence or owner markers.

Herdr removes a plugin pane when its process exits. The server log shows this for card panes today. The Pi child/native executor is the sole evidence and marker writer. It handles the card's pane.closed/pane.exited event with one positive-absence snapshot, or checks on next start if it was absent. Uncertainty remains until positively resolved. Self-close itself needs no Herdr requests, evidence reads or writes.

Today, `q` first restores the terminal, which leaves the blank pane with a cursor. It then waits for any in-flight read to finish, which can take 30 seconds. Then it runs the external close path from inside the card: an attestation, two marked observations, two source reads, a Herdr snapshot and a `plugin pane close` request. At current costs, that can keep the blank pane visible for minutes. None of that work protects anything a self-close leaves exposed.

Automatic opening should depend only on the auto-open setting, not on why a card last closed. A card opens automatically when Pi starts or resumes the conversation and the setting is on. Within a running Pi session, a closed card stays closed until the user opens it. Today, any existing evidence record for the owner suppresses automatic opening in every later session; that should stop. The trigger is Pi's session start event, not the Rust child starting, so restarting a crashed child does not reopen a card the user closed. Automatic opening keeps its positive-absence and duplicate checks.

Closing a card from outside — the CLI, or Pi on the user's request — remains an effect. It takes one fresh snapshot under the lock, marks the record closed, requests the close, and confirms through the `pane.closed` event or one snapshot.

### One Streams process per Pi session

The Pi extension should start one long-lived Rust child per Pi session and talk to it over stdio. The child holds the Herdr connection and the resolved Session, and receives lifecycle events such as reports, names and card requests. It exits when Pi does, so no separate daemon lifecycle is needed. It writes records through the existing catalog lock; cards learn of the writes through their file watches. Native effects still follow the effect rule above.

The child should also reconcile moves of its own conversation. It should subscribe to `pane.moved` and `tab.moved` for its owner pane, and to `workspace.metadata_updated` for the owner's workspace. When the owner lands in a workspace associated with another Stream, the child should apply the membership change from the native reconciliation ADR. The card then sees the Session file change. This replaces "on the next Streams interaction" for moves while Pi is running. The owner pane is the Pi process, so a move cannot happen without a running child, except after a child crash.

If the child exits unexpectedly, the extension should warn the user and restart it once. The new child reads current topology, so moves made while it was down are reconciled. If the restarted child also exits, the extension should warn again and stop Streams for that Pi session until reload.

The stdio protocol becomes a long-lived interface between TypeScript and Rust, so it needs a version number checked at startup.

### Stopgap

Until cards are event-driven, the local `~/.pi/agent/work-cards.json` sets `pane_poll_interval_ms` to 60,000. Each card then runs one costly read a minute rather than nearly continuously. Both running cards were reopened to pick up the setting.

## Order of work

Counting tests come first, so that today's 27 Herdr calls fail a test before any change. Then:

1. Remove the repeated Herdr observations and switch to the socket.
2. Make keyed lookups.
3. Make cards event-driven.
4. Add the Pi child, and move the extension's CLI calls and retry timer onto it.

Cards come after the read path is cheap, because event-driven updates still perform a read.

## Appendix: measurements

Measured on 2026-10-22 on the live workstation with two open cards:

- `streams native check`: 26.05 s, 32.18 s and 28.67 s on three runs.
- Herdr calls in one `check`, traced through a wrapper: 14 `status --json`, 5 `pane current`, 4 `workspace get`, 2 `api snapshot` and 2 `agent get`.
- Process start: `streams --help` 0.25–0.26 s; `herdr --help` 0.25–0.26 s.
- `streams --json check` on the full catalog: 0.9–1.1 s wall time, 0.08 s CPU.
- Reading all 102 catalog files in one process: 4 ms.
