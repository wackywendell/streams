# ADR: Bounded, event-driven work in every Streams process

Status: event-driven design implemented. The historical measurements below explain
why repeated process starts and observations were removed. The budgets remain
requirements to test at each real caller, not verified process-wide cost claims.
[Record-local operations](record-local-operations.md) defines catalog inputs and
[observe and act](native-observe-and-act.md) defines native effects.

## Decision

### Fixed cost per operation

Every operation should have a declared budget of process starts, Herdr requests, files read and fsyncs. A single-Session budget must not grow with unrelated Streams, Sessions or panes. Diagnostic capture reads Sessions named by the workspace's Pi panes; explicit reopening discovers active Sessions as defined by [record-local operations](record-local-operations.md#reopen-active-sessions). No saved arrangement remains. Stream-member listing and title resolution are explicit user discovery, like `list`; they never run on an event or report path. `check` and `sync` may also enumerate records. Tests should enforce budgets by counting calls through the existing fake Herdr and process runner, not by timing; host load makes timings unreliable. A new loop that performs I/O for each item then needs a deliberate budget change, rather than appearing as a slowdown later.

Intended whole-path budgets (separate from the measured record-adapter counters):

| Operation | Process starts | Herdr requests | Files read |
| --- | --- | --- | --- |
| Card update after a record change | 0 | 0 | the changed files only |
| Card update after a native event | 0 | at most 1 snapshot | 0 |
| Pi lifecycle event, through the Pi child | 0 | at most 1 | the card's keyed files |
| Foreground native action | the CLI itself | at most 1 snapshot | concerned keyed files |
| Card closes itself | 0 | 0 | 0 |
| `open`, `close`, `focus` | the CLI itself | at most 1 snapshot, then effects and their replies | concerned keyed records; no launch-evidence file |

### Every process is event-driven

These rules apply to every Streams process: the Pi extension, the Rust child it starts, cards and one-off CLI commands. A process does work only in response to an event: a Pi lifecycle event, a user command, a record file change, or a Herdr event. No process polls for changes or retries local work on a timer. A failed local operation is retried when an event shows that its inputs may have changed. For example, an unready enrollment is retried when its Session file is written.

Timers remain allowed for three things:

- one-shot interface expiry, such as clearing a status message
- backoff between attempts at a remote model call
- deadlines that bound a single operation

Native Pi handlers send one message to their Rust child rather than starting another CLI or retry timer.

### Event subscriptions are required and filtered early

A process that depends on events must have its subscriptions. If it cannot establish them, it should fail visibly rather than fall back to polling. This applies when Herdr lacks `events.subscribe`, its protocol version is unknown, or the file watcher cannot start. When a subscription drops, the process should resubscribe once and then do one full read. If resubscribing fails, it should report an error and stop updating, not degrade to polling.

Irrelevant events should cost almost nothing:

- Subscribe only to the event types the process uses.
- Watch only the directories holding its own files.
- Make the first step for each event a comparison against a small fixed set of IDs or file names. Decode only the type, pane, workspace or file name for that check; decode the rest of an event only after it matches.
- Read and re-render only after the check matches.

Herdr filters structural events by type only, so the ID comparison happens in the process. `pane.agent_status_changed` can be filtered by pane in Herdr, and should be if a process ever needs it.

To avoid missing a change between a process's first read and its subscription, it should subscribe before that first read.

### Keep keyed records separate

The Session supplies its current Stream key. A card reads those records and,
when documents need them, the host-wide checkout registry and per-Session local
directory binding. A fixed set of keyed files is the rule, not one combined file.

Putting a directory binding in the Session would synchronize host-local paths.
Putting it in the registry would make every directory selection rewrite and lock
a host-wide file and wake unrelated cards. Their separate owners and rates of
change justify the cheap keyed reads. [Record-local operations](record-local-operations.md)
owns point reads, revision guards and the separate discovery budget for explicit listing,
capture and reopening. No automatic path scans for a Session or roster.

### Separate record inputs from display inputs

Native name projection needs the Stream, Session and owner identity. It must not resolve checkouts or document files merely to construct a card-shaped input. Validated record inputs stand on their own; a displayed card adds a complete set of document locations. This keeps the card's completeness checks without inventing unavailable-document results for a pass that never requested them.

A display read uses one operation-local registry snapshot and shares checked checkout roots across repository aliases. The snapshot retains no state between operations and grants no effect authority. Opening a document still resolves current display inputs and rechecks the registry, canonical paths, containment and regular-file identity before acting.

The card builds its record input from the bytes its file watcher has already observed, rather than reopening the same Session and Stream. A membership change selects the new Stream's exact path. An explicit full refresh rereads watched files even without a notification; subscription recovery also rereads them. Neither needs a second cache or an invalidation protocol.

### Event-driven cards

A card should update only when something it displays may have changed. Events say what to re-read; they are not data to apply. When an event arrives, the card should drain any further queued events, then do one read. That read is the changed files, or one Herdr snapshot for a native event. A burst of changes therefore produces one update.

| What the card shows or depends on | What changes it | Signal |
| --- | --- | --- |
| Session title, summary, status and Stream membership | Pi reports, edits, sync, membership moves | Session file |
| Stream title, summary and documents | Stream reports, edits, sync | Stream file |
| Checkout and directory for each repository | Checkout registration, directory selection | Registry and directory binding files |
| Owner's pane, tab and workspace | Pane or tab moved | Herdr `pane.moved`, `tab.moved` |
| Whether the owner still exists | Pane closed or exited, tab or workspace closed | `pane.closed`, `pane.exited`, `tab.closed`, `workspace.closed`; the card closes itself |
| Which conversation the owner pane runs | Fork or new session in the same pane | `pane.updated`, `pane.agent_detected`; compare the agent session |
| The card's own placement | User moves the card | `pane.moved` for the card's pane |
| Terminal size and keys | User | Terminal input, already event-driven |

The card should ignore focus, output, agent status, layout, and tab or workspace renames. It does not display them.

Saves replace files by atomic rename, so the card should watch the containing directories and filter by file name. An unrelated Session's save in the same directory costs a name comparison, not a read. If the Session's membership changes, the card should switch to watching the new Stream's file. A Session has one identity-based path that does not change with membership. Readers accept only the current format, with no legacy path watches.

The card should keep showing the last good state during a read. It should show a notice only when a read fails, or takes long enough for a user to notice.

Apart from closing itself, the card only observes. It never changes membership or other catalog records. The Pi child handles membership, as described below.

No channel from the CLI or Pi child to the card is needed. File watching sees every writer — the CLI, the Pi child, sync and manual edits — without a list of which pane shows which record.

The card should check document availability when it opens, when its records change, and when a document is opened. It should not watch document files, so availability may be stale between those points. Opening a document checks the file again, so a stale display cannot open a missing file.

### Card lifetime

A card exits on `q` or observed owner loss, without a native request or closure
record. Within a Pi lifetime, a closed card stays closed until explicit open.
Pi start/resume, not child startup, is the automatic-open trigger. Opening uses
observed reuse and a per-Session OS process lock for self-exiting duplicates.
External close remains a safely targeted effect. The full lifetime and restart
risk belong to [observe and act](native-observe-and-act.md).

### One Streams process per Pi session

The Pi extension should start one long-lived Rust child per Pi session and talk to it over stdio. The child holds the Herdr connection and the resolved Session, and receives lifecycle events such as reports, names and card requests. It exits when Pi does, so no separate daemon lifecycle is needed. It writes records through the record-store boundary; cards learn of writes through their file watches. Native effects still follow the effect rule above.

The child also follows moves of its own conversation. It subscribes to `pane.moved` and `tab.moved` for its owner pane, and to `workspace.metadata_updated` for the owner's workspace. A move re-projects the card into Pi's new tab and workspace; it never changes the Session's membership, as the native reconciliation ADR describes. An unmarked destination workspace is associated with the Session's Stream; a workspace that names another Stream produces a warning.

If the child exits unexpectedly, the extension should warn the user and restart it once. The new child observes current topology and moves an existing separated card into Pi's tab. It keeps no owner-move baseline and does not reopen a closed card. If the restarted child also exits, the extension should warn again and stop Streams for that Pi session until reload.

The stdio protocol becomes a long-lived interface between TypeScript and Rust, so it needs a version number checked at startup.

### Bounded transcript identity reads

Pi and Codex identify a saved conversation in the first transcript header. Verification reads through that line with a small buffer, retaining the existing 65,537-byte bound for malformed or oversized headers. It never scans neighboring transcript directories. Claude retains its multi-record identity check: stopping at its first identity would miss conflicting records.

## Appendix: measurements

Measured on 2026-10-22 on the live workstation with two open cards:

- `streams native check`: 26.05 s, 32.18 s and 28.67 s on three runs.
- Herdr calls in one `check`, traced through a wrapper: 14 `status --json`, 5 `pane current`, 4 `workspace get`, 2 `api snapshot` and 2 `agent get`.
- Process start: `streams --help` 0.25–0.26 s; `herdr --help` 0.25–0.26 s.
- `streams --json check` on the full catalog: 0.9–1.1 s wall time, 0.08 s CPU.
- Reading all 102 catalog files in one process: 4 ms.
