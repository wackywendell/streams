# Stream status

Status: implemented.

## Purpose

A Stream's status is derived from its Sessions. A Stream is a durable work record owned by one host, and a Session is a conversation record belonging to one Stream on that same host. Only Sessions store a `status` field on disk. Stream records carry no `status` field, and every view computes a Stream's status from its member Sessions under one total order from highest to lowest precedence.

```
active > blocked > waiting > parked > done > closed
```

Open statuses are active, blocked, waiting and parked. Settled statuses are done and closed. Because Stream status is derived from Sessions, removing the stored field from `Stream` eliminates duplicate state, multi-record write cascades, reopen logic and out-of-sync disk states.

Two related topics stay outside this document. How age decay or staleness appears in listings is handled separately, and the Temper verification check is unchanged.

## Assumptions

The guarantees rest on three properties of the catalog.

1. Every Session shares its fixed writing host with its Stream, so every member of a Stream lives in one host partition.
2. Each Session record is replaced atomically under `.catalog.lock`, so concurrent Session updates on the same Stream never corrupt one another.
3. Catalog format version 4 drops `Stream.status`. Existing format-3 catalogs are rejected with the reset hint so no runtime path decodes old Stream records.

## Guarantees

Every view that reports a Stream's status satisfies three rules observable from the saved JSON files on disk.

1. Status equals the Session maximum. When a Stream has at least one readable Session, its derived status equals the highest `Session.status` among those Sessions under `active > blocked > waiting > parked > done > closed`. When every Session of a Stream is done (or done and closed), the Stream's derived status is done. When every Session is closed, the Stream's derived status is closed.
2. Empty Streams default to active. When a Stream has zero Sessions (for example right after `streams create`), its derived status is active until its first Session arrives or read-time age decay marks it closed.
3. Single writer per stored status. Each Session owns its own `status` field. Writing one Session touches only that Session file, with zero sibling scans and zero Stream writes.

## Implementation

### Components

Three components carry out the model.

1. Record model and format version (`src/model.rs`, `src/file_store/records.rs`). `Status::precedence` defines `active (5) > blocked (4) > waiting (3) > parked (2) > done (1) > closed (0)`. `Stream` has no `status` field, and `FORMAT_VERSION` is 4.
2. Pure view derivation (`src/operations.rs`). `operations::derived_stream_status(stream, members, cutoff_ms)` returns the maximum status by `Status::precedence`, or `Status::Active` when `members` is empty. When that status is open and the Stream's latest activity precedes `cutoff_ms`, it returns `Status::Done` if any member is done, otherwise `Status::Closed`, matching what `streams tidy` persists. Every view, including `streams list`, both `streams show` paths, `HostListing`, `StreamView` and `RecordContext`, derives Stream status through this one function.
3. Scoped write surfaces (`src/operations/patches.rs`, `src/operations/edits.rs`, `src/application/lifecycle.rs`, `src/cli/`, `extensions/stream-checkpoint/`).
   - `StreamPatch`, `streams create`, `streams start`, `streams update`, and the Pi `/streams` Stream tab expose no Stream status field; `streams close` and `streams restore` are removed.
   - Automatic Session reports (`Actor::Automatic`) may set active, blocked, waiting, parked or done (`Status::REPORT_VARIANTS`). Rust rejects `closed` from automatic reports in both `PatchSchemas` and `SessionEdit::new`.
   - Direct user edits (`streams session update --status`, the Pi `/streams` Session tab) may set any of the six Session statuses.
   - Harness lifecycle transitions (`resume_session`, `park_session`) and `streams tidy` (`decay_host_records`) touch only Session records. `resume_session` activates parked or closed Sessions and skips done Sessions so inspecting a finished conversation never reopens it.

```
  Session files on disk (hosts/<host>/conversations/*.json)
     |          |          |
     v          v          v
   A: done    B: done    C: closed
     \          |          /
      +---------+---------+
                |
                v
     operations::derived_stream_status
       max(done, done, closed) = done
                |
                v
     Read views: streams list, streams show,
                 HostListing, StreamView, RecordContext
```

Every Stream status shown to a user or UI comes directly from `operations::derived_stream_status` over the Stream's saved Sessions.

### Walkthrough

Finishing a two-Session Stream. Stream S has Session A (active) and Session B (active). Session A finishes and saves as done; `max(done, active) = active`. Later, Session B finishes and saves as done (via an automatic report or a user edit); `max(done, done) = done`. Every listing and sidebar immediately shows S as done with zero Stream writes.

Coming back to a done Stream. Stream S has Session A (done) and Session B (done). A user enrolls a new Session C (starts active), edits B to active, or sends a new turn in B whose automatic report sets B to active. Because `active > done`, S immediately shows as active. Opening B's transcript without new work leaves B done and S done because `resume_session` skips done Sessions.

Parking, resuming and tidying. When Pi exits cleanly, `park_session` writes only that Session as parked. If all open Sessions of S are now parked, `max` is parked; if a sibling is waiting or blocked, `max` is waiting or blocked. When Pi starts again, `resume_session` writes only that Session as active, and `max` returns to active. When `streams tidy` runs, it decays stale open Sessions to closed using revision checks; once all open Sessions of S have decayed, S's derived status becomes closed (or done if a finished Session remains).

## Failures

Each failure case names the guarantee it tests and shows why it holds.

- Two Sessions save done concurrently (tests Rules 1 and 3). Each save acquires `.catalog.lock` and replaces only its own Session file. Neither save touches the Stream file or scans siblings. Once both saves finish, every reader observes `{A: done, B: done}` and derives done.
- Session A saves done while Session B enrolls or resumes active concurrently (tests Rules 1 and 3). Each save writes only its own Session file. Regardless of lock order, the catalog ends with `{A: done, B: active}` and every reader derives active.
- Process crash or I/O error during a Session save (tests Rule 3). Atomic temp-file replacement leaves the Session file at either its previous valid JSON or its new valid JSON. Because no second Stream record needs a paired update, no cross-file drift can occur.
- Unreadable Session file on disk (tests Rule 1). Listings and Stream views name each unreadable record with its repair command and derive a Stream's status from its readable Sessions only, matching `docs/record-local-operations.md`.
