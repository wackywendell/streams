# References owned by Sessions

Author: Wendell Smith
Status: Approved
Date: 2026-07-09

---

## Overview

A reference points to something that matters to a piece of work. It can be a pull request, an issue, a web page, or a document in the Session's directory. This design moves references from the Stream to the Session, saves where each Session runs in its own record, and makes the Session and Stream records follow one set of rules.

After the change, each Session holds one map of references, keyed by URL or by a path relative to the Session's directory. The Session's own automatic report and its person write that map, through the same edit request as every other field. Streams then knows which file a document means, and finding it reads only the filesystem.

The design builds on the edit model of the CLI redesign, `docs/cli-by-entity.md`. An edit names the units it changes, and can give the saved value it expects for every one of them. This design defines the records for that model, and the save rules inside each unit.

## Terms

- Stream. The record of one continuing effort.
- Session. The record of one agent conversation's work on one Stream. A Session belongs to one host, and only that host writes it. Its Stream is on the same host. Working on that Stream from another host makes a new Session.
- Host. One machine with its own Streams identity, `host_id`.
- Harness. The coding agent that runs a conversation, such as Pi, Claude Code or Codex.
- Session ID. The harness's own ID for its conversation, saved as given. It is unique among that harness's conversations on a host.
- Key. The full name of a record. A Stream's key is `HOST/ID`. A Session's key is `HOST/HARNESS/ID`.
- Enrolled. A conversation is enrolled when a Session record exists for it.
- Catalog. The directory of Stream and Session records on a host. Sync copies it between hosts through a Git repository, which this document calls Git transport, and refuses a commit in which one host changes another host's records.
- Automatic report. After each agent turn, a model reads the conversation and proposes changes to its Session's title, state, recap, remaining work, references and branch, and to its Stream's title and goal. The Session's changes are saved as one edit, and the Stream's as another.
- Pi. The coding agent that Streams supports most fully.
- Pi extension. The Streams package inside Pi. It runs the automatic report, and shows a Session panel where a person edits the Session's references.
- Discussion edit. A change a person asks for in a conversation with the Pi extension's report model, which the next report saves.
- herdr-core. The daemon, `hc`, that keeps herdr-app's terminals running.
- Classic Herdr. The terminal multiplexer, with workspaces, panes and plugins, that came before herdr-app. Streams maps each Stream to one of its workspaces. `card watch` is the process that keeps each pane's card up to date, and notices when a Session's pane moves to another workspace.
- Card. The per-terminal work card in classic Herdr that shows one Session's record beside its conversation, and opens its documents and links.
- herdr-app. The terminal workspace app built on herdr-core. The mod is the Streams plugin inside herdr-app, and shows a Stream as a tab with a sidebar.
- Refresh. A card or the mod reading the records it shows again, after a save or a change of focus. The CLI redesign defines it.
- Artifact. A file saved into the catalog under a Session. Its entry has a name and a description of what the file holds.
- Unit. The smallest piece an edit writes or refuses on its own. Each top-level field is one unit, and each entry of the `references` map is one unit.
- Manual mark. The `manual` member of a title, a goal or a reference. `true` means a person chose the value. Automatic reports leave a manual title or goal alone, and leave a manual reference's name, purpose and role alone.
- Kind. What a reference points to. It is `pr`, `issue`, `link` (a web page) or `document` (a file in the Session's directory).
- Role. Why a reference matters. It is `deliverable` (what the work produces), `dependency` (what it waits on), `brief` (what it is asked to do) or `background` (useful context).
- Reference status. Where a reference stands. It is `active`, `fulfilled` (a deliverable is done or a dependency is met) or `dropped` (a person removed it).
- Purpose. Why a reference matters to the work, in a sentence.

## The Problem

Today the Stream holds one shared list, `Stream.references`, and two things go wrong with it.

A document reference names a repository and a path, and no clone. A host often has several clones of one repository, one per branch worktree. To open a document, Streams picks the reading Session's clone, an explicit default or the only clone it knows. It can open the wrong worktree's copy. Each lookup also checks the clone by running Git or jj.

The list has many writers. Every member Session's report writes it, and so does the person. Its save carries the Session as the report read it, checks membership with a counter called `membership_epoch`, and compares each reference with the value the report read. The prompt also has to explain that one Session finishing does not mean the effort's deliverable is done.

A Session already runs in one directory, which answers which clone, and it has one automatic writer, its own report. Moving the references onto the Session removes the guess and most of the save rules.

## Goals

- Each Session holds one map of references. Streams hold none and show none.
- A document reference is a path relative to the Session's directory. A Session in a plain directory, outside any clone, can hold documents.
- Finding a document's file reads only the filesystem and starts no Git or jj process.
- Each reference is one unit, keyed by its URL or path, so writers who change different references never overwrite each other.
- When a person drops a reference, an automatic report cannot add that key back. When a person edits a reference, reports leave its name, purpose and role alone.
- References are part of the Session record, and stay with it when it moves to another Stream.
- The Session record saves its directory, its repository URL, its path within the clone and the branch last reported. A person can find the space when it exists, and recreate it when it is gone.
- An automatic report writes one save per Session, covering progress and references together.
- Stream and Session fields follow one set of rules for names, empty values and manual marks.
- Code that served only Stream references, repository-addressed documents or host-local directory files is removed.

## Non-Goals

- Reading a document from another host, another Session's directory, a deleted directory or a past commit. Such a document is reported as not available here.
- Storing a commit, ref or repository with a document.
- Copying files into Streams. A file outside the Session's directory can be saved as an artifact or linked by URL.
- A merged view of all references in a Stream. The app shows the focused Session's references.
- Searching for a moved or deleted worktree, or recreating one automatically. A person does this from the saved repository fields, following a written step.
- A Session on one host belonging to a Stream on another.
- Migration code in Streams. A script kept in `.tmp/`, outside the repository, converts an existing catalog once.
- Command names, flags, output formats and which programs open files. These belong to the CLI redesign, and the section "Changes for the CLI redesign" lists what it takes from here.

## Operating Assumptions

- One person works across a few hosts, with tens of Sessions per Stream at most.
- No catalog is shared through Git transport today.
- People edit references while watching. Automatic reports run after each agent turn. A lost or partly refused report is acceptable, because the next turn reports again.
- Moving a Session to another Stream is rare.
- Cards run on the Session's own host.
- Streams is alpha. Records are generated automatically, and runtime code never reads old formats.

## Failure Boundaries

An edit is a request with `changes`, the new values, and optionally `before`, the saved values the writer expects. "Editing a Session" gives the full rules.

- An edit whose document key is an absolute path, or has a `..` segment, fails as a whole. The CLI and the Pi extension turn an absolute path inside the Session's directory into a relative key before they send the edit, and refuse any other path with an error that suggests an artifact or a link.
- A document whose Session is on another host, or has no directory, is reported as not available here. A file that is missing, is not a regular file, or resolves through a symlink to somewhere outside the Session's directory is reported as unavailable, with the reason. These checks run when `locate` runs, and a card opens the path its last refresh was given. Streams tries no other clone and no commit.
- When the request gives `before`, a unit whose saved value differs from its `before` value is not written. The result lists it with its current value, unless that value is already the one requested.
- A request that gives `null` as a reference's new value fails as a whole.
- A request without `before` that gives an object with members missing fails as a whole.

## Critical User Journeys

### CUJ 1: The agent opens a pull request

1. An agent in Session S opens `https://github.com/o/r/pull/43`.
2. After the turn, S's report proposes a new deliverable reference for it.
3. The report saves that entry together with S's progress in one edit.
4. S's card shows the pull request. The app's sidebar shows it while S is in focus.

### CUJ 2: A person drops a wrong reference

1. S's report added `https://github.com/o/r/issues/7`, an issue the conversation mentioned in passing.
2. The person drops it from Pi's Session panel or the CLI. The entry stays in S's map with status `dropped`, and the card hides it.
3. The next report sees the dropped entry and proposes nothing for it. If it proposes adding the issue anyway, that one entry is not written, and the rest of the report is saved.

### CUJ 3: A person and a report edit at the same time

1. The person renames S's pull request reference while S's report is running. The entry is now marked manual.
2. The report, working from what it read before the rename, adds an issue and marks the pull request fulfilled.
3. The issue is added. The pull request keeps the person's name and stays active, and the report's result lists it with its current value. The next report sees the manual mark, leaves the name alone, and can mark it fulfilled then.

### CUJ 4: Opening a document

1. S runs in `~/src/streams/feature-x`, which S's record saves as its directory. S has the document `docs/plan.md`.
2. The person opens it from S's card or from the app.
3. Streams joins the directory and the path, and checks that the file is inside the directory. The file opens. No Git or jj process runs.
4. Another Session T, in a second clone of the same repository, can also have `docs/plan.md`. Each opens its own clone's file.

### CUJ 5: A path outside the Session's directory

1. S's agent writes `/tmp/notes.md`, and also `~/src/streams/feature-x/docs/plan.md` by absolute path.
2. The report proposes both as documents. The Pi extension leaves out `/tmp/notes.md`, because the edit would refuse it and fail the whole report. It logs that the file can be saved as an artifact. It sends the second as `docs/plan.md`.
3. A person who adds `/tmp/notes.md` from the CLI gets an error that names S's directory and suggests an artifact or a link.

### CUJ 6: A Session moves to another Stream

1. S belongs to Stream A and holds two references. The person decides S's work belongs to Stream B, on the same host.
2. The person changes S's `stream_id` to B. In classic Herdr, moving S's pane to B's workspace makes the same edit.
3. S keeps both references and its artifacts. B's view lists S and its artifacts, and A's no longer does.

### CUJ 7: The worktree is gone

1. The person deletes the worktree `~/src/streams/feature-x`.
2. S's documents are reported as not available here, because the directory no longer exists.
3. `session show` gives S's repository URL, its path within the clone and the branch last reported. The person clones the repository, checks out the branch, checks that it holds the work, and sets S's directory to the new path. S's documents open again.

## Technical Plan

### Record rules

These rules apply to every field of the Stream and Session records.

- A field or member with no value is `null`. It is never `""`, and never left out. Every text is nonempty.
- A pair of fields that are always written together is one field holding an object. A text and its short form are `{text, short}`. A value and its manual mark share one object.
- One name means one thing. `status` is always an enum, and `state` is always the Session's progress text. `X_id` is the ID of record X. `host_id` names the owning host on every record.
- Each top-level field is one unit. In the `references` map, each entry is one unit.
- A field is editable, read-only or computed, as the CLI redesign lists them. An edit that names a read-only or computed field fails as a whole.
- A Session's `status` is `active`, `waiting`, `blocked`, `parked`, `done` or `closed`, as today. A Stream has no stored `status`; its status is derived from its Sessions.

### The Session record

```json
{
  "host_id": "retriever",
  "harness": "pi",
  "id": "0199ab12-7c3e-4f00-9a1b-2d5e8f6c0a11",
  "stream_id": "7f3c2a90-…",
  "title": {"text": "Session references", "manual": false},
  "status": "active",
  "state": {"text": "Drafting the design", "short": "drafting"},
  "recap": {"text": "…", "short": null},
  "remaining": null,
  "directory": {
    "path": "/Users/w/src/streams/feature-x",
    "repository": {
      "url": "git@github.com:o/streams.git",
      "within": null,
      "branch": "feature-x"
    }
  },
  "references": {
    "https://github.com/o/r/pull/43": {
      "kind": "pr",
      "name": "Add session references",
      "purpose": "Implements this design",
      "role": "deliverable",
      "status": "active",
      "manual": false
    },
    "docs/session-references.md": {
      "kind": "document",
      "name": "Design",
      "purpose": "The agreed design for this work",
      "role": "brief",
      "status": "active",
      "manual": true
    }
  },
  "artifacts": {
    "notes.md": {"description": "Interview notes for the design"}
  },
  "revision": 12,
  "updated_at_ms": 1783600000000
}
```

The Session's identity is its host, its harness and its Session ID, so its key is `HOST/HARNESS/ID`. Each Session has its own directory, `hosts/HOST/sessions/HARNESS/ID/`, which holds its record, `session.json`, and its `artifacts/`. No Session's files can then share a path with another's, as `foo.json` and `foo` would. `id` is the harness's own ID, saved as given. It must be a safe token, made of ASCII letters, digits, `.`, `_` and `-`, and starting and ending with a letter or digit. Pi requires the same of its IDs, and Codex and Claude Code use UUIDs. A harness ID that breaks the rule fails `session create` with an error that names it. `host_id`, `harness` and a Stream's ID name directories too, and follow the same rule. `Session::id_for`, the UUIDv5 derived from these three values, and `harness_session_id` go.

`title` is `{text, manual}`. `state`, `recap` and `remaining` are each `{text, short}` or `null`, and `short` is nonempty text or `null`. `checkout` and `membership_epoch` are removed.

`artifacts` maps each artifact's name to its entry, `{description}`, as `references` maps keys to entries. It is a read-only field that `artifact write` keeps. An artifact belongs to its Session alone. Its key is the Session's key followed by `/NAME`, and its file is `artifacts/NAME` in the Session's directory. A Stream's `artifacts` is a computed field, listing its member Sessions' artifacts, which a Stream read already loads. This replaces the grouping by producing Stream in `docs/session-artifacts.md`, and `ArtifactKey` loses `stream`.

`stream_id` names a Stream on the Session's own host. It is an editable field. An edit that changes it reads `hosts/HOST/streams/ID` on the Session's own host, and fails as a whole if that Stream does not exist there. A Stream key from another host is refused before the edit.

### The Stream record

```json
{
  "host_id": "retriever",
  "id": "7f3c2a90-…",
  "title": {"text": "Streams references", "manual": true},
  "goal": {"text": "References live on Sessions", "short": "session refs", "manual": false},
  "revision": 4,
  "updated_at_ms": 1783600000000
}
```

The Stream loses `references` and `repositories`. `owner: {host_id}` becomes `host_id`. `title` and the title mode become `title: {text, manual}`, and `goal`, `goal_short` and the goal mode become `goal: {text, short, manual}` or `null`. A Stream's key stays `HOST/ID`, with a generated ID.

### The Session's directory

`directory` is one field, and so one unit. It is `null` when no directory is set.

- `path` is the absolute path where the Session runs. It is required.
- `repository` describes the clone that holds `path`. It is `null` when `path` is not in a clone, or the clone has no remote. When it is an object, all three members are present.
- `repository.url` is the clone's remote URL, and is required. It is the remote of a jj workspace's backing repository, or else `origin`.
- `repository.within` is the path from the clone's root to `path`, following the path key rule, or `null` at the root.
- `repository.branch` is the branch the work is on, as last reported, or `null`.

The edit checks the shape only. It reads no files.

Setting the directory from the CLI resolves the path with `fs::canonicalize`, then inspects the clone once to fill in `repository`. Finding a document starts no Git or jj process.

- In a Git clone, `branch` is the branch `HEAD` names, or `null` when `HEAD` is detached.
- In a jj workspace, `branch` is the bookmark on the nearest ancestor of `@` that has local bookmarks, when it has exactly one, or else `null`.
- A path in no clone, or in a clone with no remote, gives `repository: null`.
- Any other Git or jj failure fails the command, with the error.

Pi sets the directory this way when it creates its Session, and a person refreshes the branch by setting the directory again. A report may update the branch with a merge onto `before`, sending `{"directory": {"repository": {"branch": "B"}}}` with the directory it read. If the path or repository changed since then, `before` no longer matches and the branch is not written. A report sends no branch when the saved `repository` is `null`, because the merged value would be incomplete.

`branch` is a report, so it can be stale. The written step for recreating a worktree says to check the branch before using it.

Moving a worktree means setting each of its Sessions' directories to the new path. Recreating a deleted one means cloning `url`, checking out `branch` and setting the directory to the clone joined with `within`. Both are written steps in the README.

`session resume` runs the harness in `directory.path` with the Session ID, such as `pi --session ID`. What the harness then does is its own concern. After a worktree has moved, Pi forks the conversation into the new directory, and the fork has a new ID, so it becomes a new Session. No transcript path is saved. `SessionCommand` passes the ID, and stops resolving the transcript file that it passes to Pi today.

### Reference keys and entries

A key has one of two forms, and no string has both.

- A URL key is an `http` or `https` URL that `model::reference_url` accepts. That function refuses whitespace, control and format characters, backslashes, broken percent escapes, a missing host, and a user name or password. The key is saved exactly as given, so two spellings of one address are two keys.
- A path key follows the rule in today's `runtime::checkouts::relative_path`. Segments are separated by `/`, and no segment is empty, `.` or `..`. There is no backslash, no leading `/` and no `:` in the first segment. It has at least one character other than whitespace, and no ASCII control character or DEL. The rule moves into `model.rs` and replaces the looser `check_relative_path`.

A URL always has `:` in its first segment, so it never passes the path rule. An entry's kind must agree with its key. `document` takes a path key, and `pr`, `issue` and `link` take a URL key. An entry has no generated ID. Two writers who add the same pull request with the same URL write the same key, and the second add finds the first. Pointing an entry at a different URL or path means dropping one key and adding another.

An entry is `{kind, name, purpose, role, status, manual}`, and every member is required. Name and purpose are nonempty text from whoever adds the entry. Streams fills in none of them.

The manual mark works as a title's does. A person's edit sets `manual: true`, and a report sets `false` on entries it adds. A report leaves the name, purpose and role of a manual entry alone, and may still change its status. The Pi extension follows this rule, and the edit does not enforce it, because the mark is not an access control.

Cards and the app list entries by role, then name, then key. The map's key order carries no meaning.

### Editing a Session

An edit is one request, `{"changes": {...}, "before": {...}}`. Both halves name units.

```json
{
  "changes": {
    "state": {"text": "PR open", "short": "review"},
    "references": {
      "https://github.com/o/r/pull/43": {"status": "fulfilled"},
      "https://github.com/o/r/issues/9": {"kind": "issue", "name": "Track rollout", "purpose": "Rollout waits on this", "role": "dependency", "status": "active", "manual": false}
    }
  },
  "before": {
    "state": {"text": "Drafting the design", "short": "drafting"},
    "references": {
      "https://github.com/o/r/pull/43": {"kind": "pr", "name": "Add session references", "purpose": "Implements this design", "role": "deliverable", "status": "active", "manual": false},
      "https://github.com/o/r/issues/9": null
    }
  }
}
```

These rules apply to every unit of the Stream and Session records.

- `before` is optional. When it is absent or `null`, every change is written.
- When `before` is given, its keys must be exactly the keys of `changes`, at the top level and inside `references`. Otherwise the whole request fails. A request is either all overwrites or all checked writes.
- A `before` value is the saved value the writer expects. `null` means the unit has no value, and for a reference it means the entry is absent.
- With `before`, each value in `changes` is merged onto its unit's `before` value before anything is saved. The merge is JSON Merge Patch (RFC 7386), changed so that a member given as `null` is set to `null`. RFC 7386 would remove that member, and these records never leave a member out. Objects merge member by member, and an absent member keeps its `before` value. A value that is not an object, or a `before` value of `null`, is replaced by the change.
- Because `before` must equal the saved value, merging onto it gives the same result as merging onto the saved value, and the merge needs no read.
- Without `before`, each value in `changes` must be complete. An object with a missing member fails the whole request.
- A merged value must be valid. `null` for a required member, such as `directory.path`, `title.text` or a reference's `name`, fails the whole request.
- A unit whose saved value differs from its `before` value is not written. Every other unit is.
- The whole edit is one save of the record. The `before` comparison, the merge, the check of a new `stream_id` and the result all run on the current record inside the store's save callback (`RecordStore::save`), so a change saved after the writer's read is compared and never lost.

In the example, the pull request's change names only its status, and the merge keeps its saved name, purpose, role and mark. The issue has `before: null`, so its entry is new and must be complete. These changes, each with its `before`, clear values:

- `{"state": {"short": null}}` clears the short form and keeps the text.
- `{"directory": {"repository": {"branch": null}}}` clears the branch.
- `{"directory": {"repository": null}}` clears the repository.
- `{"remaining": null}` clears the whole field.

The result names the record and lists each requested unit whose saved value, after the save, differs from the merged value requested, with its current value. An absent reference shows as `null`. A unit missing from `not_written` holds the requested value. That is true whether this edit wrote it, or another writer saved the same value first.

```json
{
  "key": "retriever/pi/0199ab12-7c3e-4f00-9a1b-2d5e8f6c0a11",
  "not_written": {
    "references": {
      "https://github.com/o/r/pull/43": {"kind": "pr", "name": "Session references", "purpose": "Implements this design", "role": "deliverable", "status": "active", "manual": true}
    }
  }
}
```

Entries are never deleted. `null` as a value in `changes.references` fails the whole request. To remove a reference, a writer sets its status to `dropped`, which with `before` is `{"status": "dropped"}`. The entry stays, so a report that proposes the same key with `before: null` is refused. Setting the status back to `active` with `before` restores the entry, and the merge keeps its saved name, purpose and role. Pi's Session panel restores this way. A CLI add writes the complete entry, so it restores with the name, purpose and role given.

A request that breaks a rule above, or has a malformed key or value, fails as a whole and writes nothing. The error names the key.

### Document paths from callers

The edit accepts only relative path keys. The CLI's document flag and the Pi extension accept an absolute path, because both know the Session's directory, and turn it into a key:

1. A path with a `..` component in the raw input is refused. `Path::components` finds it, and Node's `path.relative` runs only after this check.
2. `Path::strip_prefix` removes `directory.path`. It compares whole components, so `/a/bc` is not inside `/a/b`.
3. If that fails, the CLI resolves the longest existing ancestor of the path's parent directory with `fs::canonicalize` (`fs.realpathSync` in Node), joins the rest of the path on with the final name unchanged, and runs step 2 again. A file that does not exist yet is accepted, and a symlinked file keeps its own name. This accepts another spelling of the same directory, such as `/tmp` for `/private/tmp` on macOS.
4. The remainder must pass the path key rule.

Any other path is refused, with an error that names the directory and suggests an artifact or a link.

For example, with `directory.path` at `/private/tmp/work`, the input `/tmp/work/link.md`, where `link.md` is a symlink to `real.md`, becomes `link.md`. The parent `/tmp/work` resolves to `/private/tmp/work`, and the name `link.md` is kept. `locate` then follows the link when it opens the file.

Two inputs can give one key, such as `docs/a.md` and `/tmp/work/docs/a.md`. The CLI adds one reference per command. The Pi extension converts every proposal first, keeps the first proposal for each key, and logs the rest.

### Finding a document's file

One function, `documents::locate(session, path)`, finds the file for a document. Every caller uses it. These are `session show` for `document_files`, the document commands, the card and the app. `document_files` maps each document key to `{"path": FULL_PATH}`, or to `{"reason": TEXT}` when it has no file here.

1. Check `path` against the path key rule.
2. If the Session belongs to another host, or `directory` is `null`, the result is "no directory here".
3. Resolve `directory.path`, and `directory.path` joined with `path`, with `fs::canonicalize`. A missing directory gives "no directory here", and a missing file gives "missing".
4. The resolved file must be inside the resolved directory (`Path::starts_with`), or the result is "outside the Session's directory". It must be a regular file, or the result is "not a file".
5. Otherwise the result is the full resolved path.

Any other I/O error, such as a permission error, is returned as an error, and the CLI reports it with the path and the cause. No Git or jj process runs, and records are not checked when they load.

The current `directory` defines every document path, including those in reports already running.

### The automatic report

The Pi extension owns the report's rules. Rust applies the edit and knows nothing about reports.

1. The extension reads `session show --json` and `stream show --json` for the Session's Stream. The first gives the Session record, including dropped references, its directory and `document_files`. The second gives the Stream's title and goal.
2. The model sees the conversation, the Session's own references and its directory. It sees no other Session's references. The prompt says references describe this Session's work, and drops the rule about whole-effort fulfilment.
3. The extension filters the model's proposals.
   - It leaves out any proposal for a dropped key, and never sets a dropped entry back to `active`.
   - It leaves out changes to the name, purpose or role of a manual entry.
   - It turns absolute document paths into keys as above, keeps the first proposal for each key, and leaves out paths outside the directory, logging that they can be saved as artifacts.
   - It may update `directory.repository.branch` when `repository` is set.
   - It leaves out the Session's title when the Session's `title.manual` is `true`, and the Stream's title or goal when that field's `manual` is `true`.
4. It adds the person's discussion edits on top, with `manual: true` on each reference they change. These are a person's edits, so they may change a manual entry or restore a dropped one. Where a discussion edit and a proposal name the same unit, the discussion edit wins.
5. It sends the Session's changes with `session edit SESSION --input -`, and the Stream's changes, if any, with `stream edit STREAM --input -`. Each edit has `before` for every change, taken from its own read. That is `null` for a new entry and the saved value for anything else.
6. It compares what it sent with each result on its own. Each unit missing from `not_written` holds the requested value. The extension shows the resulting reference status ("#43 is fulfilled", "added #9"), and logs each unit in `not_written` with its current value, with no retry. The next turn reports again.

A report that starts over from the record, after the person resets the conversation's report state, still shows the model every reference, including dropped ones, so it proposes none of them again. "References…" moves from the Stream panel to the Session panel.

### People editing references

People edit references with `session edit` and with Pi's Session panel. Both send the edit request above, and both set `manual: true` on the entries they change.

- CLI flags send no `before`, so they always write. Adding a reference from the CLI takes its key, kind, name, purpose and role, and writes the complete entry with status `active`, which also restores a dropped entry.
- Pi's Session panel sends `before` with the entry as it showed it. If the result lists the entry, the panel says it changed since it was shown and shows the current value.

A person adds briefs, background and dependencies to a Session, their own or a named one. A Stream has no references of its own.

### Cards and the app

A card shows its own Session's references and hides dropped ones. It keeps the full path that its last refresh was given for each document, and opens that path when the person picks it. A link opens its URL. The card checks nothing and reads nothing again when the action fires, because the person is watching and the card refreshes whenever its Session saves. The same holds for the mod's sidebar. The card does not edit references.

A document is identified by catalog, Session key and path. This replaces the checkout IDs and viewing Session in today's document identity.

The Stream view lists Sessions and no references. The app's sidebar shows the focused Session's references. The CLI redesign's refresh supplies them with `session show` for the focused Session, beside `stream show` for the Stream's title, goal and Sessions. When the focused terminal has no enrolled Session, the sidebar shows no references.

### Moving a Session

A move is an edit of `stream_id`, with the old value as `before`. References, directory and artifacts stay on the Session, and their keys do not change. The new Stream lists the Session's artifacts, and the old one no longer does. In classic Herdr, `card watch` makes the same edit, through the same function as `session edit`, when a Session's pane moves to another Stream's workspace. It creates the Stream first when the workspace has none, as it does today. A move that races another move is refused by its `before`, and the card shows the Session's current Stream.

### Removals

These go, with their tests:

- `Stream.references` and `Stream.repositories`, which held the shared list and repository aliases, with their CLI and app code.
- `Session.checkout`, the Session's repository and revision, and `membership_epoch`, the counter that let a report detect a move. This includes the checks in `operations/edits.rs` and `operations/references.rs`.
- The Temper model of report admission. That is `specs/report_admission.ioa.toml`, `tests/verification.rs`, its `[[test]]` target in `Cargo.toml`, the `temper-jit` and `temper-verify` dev-dependencies, their `[profile.test.package.*]` entries and those of their dependencies such as `stateright`, the `Cargo.lock` entries they bring, and `just test-verify` with its place in `just test`.
- `operations/membership.rs`, `MoveIntent`, `apply_move` and `move_session_observed`. The `stream_id` edit replaces them.
- The grouping of a Session's artifacts by Stream (`artifacts::groups`), and the Stream in `ArtifactKey` and in artifact paths.
- `Session::id_for`, `harness_session_id`, the Stream's `owner` object, `TitleMode` with `inherit` and its `resolve`, and the goal mode.
- `RepositoryDocument`, and the document target's `repository_url`, `ref`, `commit` and `source_session_id`. `ResourceKey` becomes the reference key, with only its URL and path forms.
- `AutomaticReferenceValue`, `AutomaticDocumentTarget`, `materialize`, the TypeScript `materialiseReference` and `CapturedEdit::References`, which turned report proposals into Stream references.
- `src/runtime/revisions.rs` and `checkout document`, which read files at a past commit.
- Directory bindings (`src/runtime/locations.rs`), the host-local files that saved each Session's and Stream's directory and transcript path.
- The checkout registry (`src/runtime/checkouts.rs`, `checkouts/state.rs`), the host-local list of clones with their repository URLs and defaults, and the `checkout` commands. Inspecting a clone stays, as one function that fills in `repository`.
- `catalog.checkouts` and `document_locations` in the report context, and `documents::context`.
- The card's re-read and compare before acting, in `src/runtime/native_cards/documents.rs` and `src/cli/cards.rs`.
- In the Pi extension, `ReferenceDiscussionEdit` and `CardState.referenceEdits`.
- In the shared TypeScript wire parser, `extensions/stream-checkpoint/wire.ts`, the Stream's `references` and `repositories`, the reference array, the string progress fields and `Session.checkout`. That file stays the one source of the record schema in TypeScript, and the extension modules keep importing it.

### Format change and conversion

The catalog format goes from 4 to 5. Streams reads only format 5, and points older catalogs at the reset steps in the README.

A script in `.tmp/session-references/`, kept out of the repository, converts one host's catalog once. It runs with Streams stopped, after a backup of the catalog and of the host state it reads.

Its inputs are these:

- the catalog's format marker, `.catalog-local/records.json`;
- Stream records in `hosts/HOST/streams/`, and Session records in `hosts/HOST/conversations/`;
- artifact files in `hosts/HOST/streams/STREAM/artifacts/SESSION/`;
- the catalog's directory bindings, in `$XDG_STATE_HOME/streams/catalogs/DIGEST/`, where `DIGEST` is the SHA-256 of the catalog root;
- the host's checkout registry, in `$XDG_STATE_HOME/streams/checkouts/`.

For each Stream, the script does the following:

- It removes `references` and `repositories`, and replaces `owner` with `host_id`.
- It joins `title` and `title_mode` into `{text, manual}`, and `goal`, `goal_short` and `goal_mode` into `{text, short, manual}`, or `null` when there is no goal. A mode of `manual` gives `true`, and `automatic` gives `false`. A mode of `inherit` takes the host's old default, `naming.stream`, from the config file, so a title that was protected stays protected.

For each Session, the script does the following:

- It moves the record to `hosts/HOST/sessions/HARNESS/ID/session.json`, with `id` set to the old `harness_session_id`, and refuses to run if any key part breaks the token rule.
- It moves each artifact file from `hosts/HOST/streams/STREAM/artifacts/SESSION/NAME` to `hosts/HOST/sessions/HARNESS/ID/artifacts/NAME`, and turns the Session's `artifacts` into one map from name to `{description}`. If a Session has two artifacts with one name under different Streams, the script refuses to run and lists them, and the person renames one.
- It removes `checkout` and `membership_epoch`, adds an empty `references` map, and joins `title` and its mode as for a Stream, with `naming.session` as the default for `inherit`.
- It joins the three text pairs into `{text, short}`, giving `null` for an empty text.
- It sets `directory` as follows:
  - A Session with no binding gets `directory: null`.
  - Otherwise `path` is the bound directory, or the checkout's saved directory joined with the path within it.
  - When that directory exists, `repository` comes from inspecting the clone, as the CLI does.
  - When it is gone, `repository` comes from the old `checkout.repository_url`, the binding's path within the checkout and `branch: null`, and is `null` when the old record has no URL. Format 4 saves the checkout's root as `"."`, which becomes `within: null`.

Last, the script deletes this catalog's directory bindings and sets format 5. It leaves the checkout registry in place, because other catalogs on the host may still use it and format 5 never reads it. The README's reset steps say to delete it once every catalog on the host is converted or reset.

Existing references are lost. Streams, Sessions and artifacts are kept. A catalog shared through Git transport follows the README's reset steps, because sync refuses one host rewriting another host's records. None is in use today.

### Implementation and verification

This change and the CLI redesign land in one cutover. The order, the files and the checks for both are in the "Detailed implementation plan" of `docs/cli-by-entity.md`. Code that the CLI redesign deletes is deleted, and not ported first.

These are the test cases this design adds. The plan cites them.

- Codec. Each key form, kind and key agreement, nonempty name and purpose, the path rule's control character and whitespace cases, the safe token rule for Session IDs, the `directory` shape (a `repository` object with a `null` URL fails, and `within: null` passes), and `null` in place of empty text.
- Edit, with a fake store that changes records between read and save:
  - mixed reference saves;
  - exact `before` keys;
  - the merge onto `before`, including clearing `short`, `branch` and `repository`, and refusing `null` for `path` and `name`;
  - incomplete values without `before`;
  - `null` references;
  - dropped suppression and restore;
  - a branch change refused after a directory change;
  - a title marked manual during a report;
  - an unrelated edit, and an edit of the same unit, saved between the writer's read and its save;
  - a unit refused by `before` that already holds the requested value, which is missing from `not_written`;
  - a `stream_id` change to a missing Stream, or one on another host.
- `documents::locate`:
  - a symlink that leaves the Session's directory but stays inside the clone;
  - a symlink inside the directory;
  - a missing file and a missing directory;
  - a Session on another host;
  - a plain directory;
  - a fake process runner that fails the test if `locate` starts a process.
- Path conversion. Another spelling of the directory with an existing symlinked file, a dangling symlink, a missing file, a `..` segment, and a path outside the directory.
- Clone inspection. A Git clone on a branch and detached, a jj workspace with one bookmark and with two, a clone with no remote, and a plain directory.
- Pi extension, with a fake `streams` program:
  - a proposal for a dropped key is left out, and a discussion edit restores one;
  - a proposal does not change a manual entry's name, and a discussion edit does;
  - a manual goal is left out of the Stream edit, and the two results are handled separately;
  - two spellings of one document give one change and a log line;
  - a notice when another writer saved the same value first;
  - a refused unit is logged with its current value.
- `card watch`. A fake pane move to another Stream's workspace changes `stream_id`, and a move that races another is refused.
- Session layout. Sessions with the IDs `foo` and `foo.json` on one harness, each with an artifact, write, list and read back.
- Resume. A fake launcher sees the harness run in `directory.path` with the Session ID.
- Conversion. A fixture catalog with a plain directory, a checkout with a subdirectory, a present and a deleted checkout bound at its root, a checkout whose directory is gone, a Session with no binding, Stream references, every title and goal mode with each `naming` default, empty text fields, and artifacts under two Streams for one Session. A second catalog on the same host keeps the registry it reads.

## Alternatives Considered

### Keep a Stream list for briefs

A person-only list on the Stream would hold briefs and background that belong to the whole effort. It would keep a second owner of references, a second edit path, and the question of which clone a Stream's document is in. Putting a brief on a Session costs one choice of Session.

### Paths relative to the clone root

A path from the clone root reads naturally for repository files. A Session in a plain directory has no clone root, and every Session has a directory.

### Documents addressed by repository, ref or commit

Storing a repository URL, and a ref or commit, would let any clone or a remote read the file. That is today's model, and it is what makes Streams guess the clone. Opening a document from the Session's own directory covers every workflow in use.

### References as a list

A list keeps insertion order. Editing one entry would then need list operators or an index in the edit model, and two writers who add the same pull request would add it twice. A map keyed by URL or path makes each entry a unit, and the same URL string always lands on the same key.

### Generated IDs as keys

An ID per entry would let an entry change its URL or path in place. Two writers adding one pull request would create two entries, and every caller would need a lookup from URL to ID. Dropping one key and adding another covers a changed target.

### Normalising URL keys

Streams could save each URL in a normal form, so two spellings of one address give one key. Both halves of a request would then need normalising before any check, with an error when two keys collapse into one, and the Pi extension would need the same normal form. The parser still keeps some equal addresses apart, such as `/%7Euser` and `/~user`. Saving the URL as given needs none of this. A second spelling becomes a second entry, which a person can drop.

### Deleting entries

Letting a person delete a reference is simpler to explain. The same conversation would then lead the next report to add it back, because nothing records that the person removed it. A dropped entry is that record, and it costs one small entry per removal.

### Letting `before` name some of the changed units

`before` could guard some units and leave others to be written whatever their saved value. A request then mixes checked writes and overwrites. A writer that means to check every unit, and leaves one out by mistake, overwrites that unit unseen. Requiring the same keys makes a request one or the other, and a mix fails as the likely mistake it is.

### Replacing whole values only

Every value in `changes` could be complete, even with `before`. Changing one branch or dropping one reference would then repeat every other member. With `before` the full current value is already in the request, so merging onto it is exact and adds no reads.

### Plain RFC 7386 merge

The merge could follow RFC 7386 exactly, so `null` removes a member. A cleared branch or short form would then be missing from the merged value, and the record would need a rule that a missing member means `null`. Setting the member to `null` keeps every object complete, and the decoder needs no default.

### A mode beside each value

A title and its mode could stay two fields. A person who marks the title manual during a report would then change only the mode, and the report's title write, checked against the unchanged title, would still be saved. One `{text, manual}` field makes the switch to manual change the unit the report checks, so the report's write is refused.

### A derived Session ID

Today a Session's ID is a UUIDv5 of its host, harness and harness ID. That ID carries nothing the three values lack, so every caller that knows its conversation must look the Session up by those values. Using the harness ID in the key lets the Pi extension name its own Session, and leaves one ID for each Session. The cost is the safe token rule on harness IDs.

### Sessions on another host's Stream

A Session could join a Stream on another host, with `stream_id` holding a full key. The Session's host would then write records that point into another host's partition, and a move would cross hosts. Work on a Stream from another host makes a new Session on that host's own Stream.

### Artifacts grouped by producing Stream

A Session's artifacts could stay under the Stream that produced them, so the old Stream keeps listing a moved Session's outputs. Artifact keys and paths would then name a Stream the Session may have left, and artifacts would follow a different rule from references. Moves are rare and a person's choice, so artifacts belong to the Session alone and move with it.

### A separate move operation

Moving a Session could stay its own operation, as `move_session_observed` is today. That keeps a second code path that writes `stream_id`, with its own checks. An edit of `stream_id` with `before` uses the same request, save and result as every other field.

### Rewriting absolute paths in the edit

The edit could read the Session's directory and rewrite absolute keys. That puts a filesystem read inside a catalog edit, and two keys in one request can collapse into one after the rewrite. The CLI and the Pi extension already know the directory, and the Pi extension already filters paths so that one bad path does not fail a report.

### Refusing one entry with a reason

A malformed or outside path could be refused alone, with the reason in the result, while the rest of the edit is saved. That adds a reason field to the result for a case the Pi extension already prevents. A path that slips through costs one report, and the next turn reports again.

### Copying outside files

Streams could copy a file outside the Session's directory, such as one in `/tmp`, into an artifact. It would then own a copy that goes stale unseen. The person or agent can save an artifact on purpose, and the error points them to it.

### A checkout registry and host-local bindings

Today a host-local file binds each Session to a directory, or to a registered clone and a path inside it, and `checkout move` updates every Session in a moved clone. With documents relative to the Session's directory, the registry's only remaining use is that move. Moving a worktree is rare and done by a person, so a written step covers it. A Session belongs to one host, so its directory can live in its own record.

### A separate branch field

`branch` could be its own top-level field. A report could then change it while a person points the Session at a different repository, and save a branch that belongs to neither. Inside `directory`, the `before` check refuses a branch change once the path or repository has changed.

### Checking the clone on every read

Today each directory lookup runs Git or jj to confirm the directory is still a clone root. Finding a document then depends on a process starting. Checking the saved directory and containment covers the failures seen in use, and setting a directory still inspects the clone.

### Refusing a directory change while documents exist

Streams could refuse to change a Session's directory while it has documents. That ties the directory to the references, misses reports already running, and blocks moving work to a new worktree. Streams uses the current directory for every path.

### Re-reading before a card action

A card could read its Session again when a person picks a reference, and act only if the entry is unchanged. It could also check again that the file is inside the directory. The card already refreshes on every save, and the person is watching. The worst case is one stale document or link opening, or a link replaced after the refresh being followed.

### Including other Sessions' references in the report

Showing a report its siblings' references would need a read of the Stream's Sessions on every turn, and invites copying another Session's links. A report needs only its own conversation's evidence.

### Migration code

Streams could read format 4 and convert it on load. The repository's rule is to reset old data without migrating it, and runtime code never reads old formats. A one-time script outside the repository keeps the Sessions and artifacts the person cares about.

## Changes for the CLI redesign

`docs/cli-by-entity.md` follows this design. It has these parts:

- references on `session edit` only;
- one reference per add command, with name, purpose and role required;
- `session show` with every reference and `document_files`;
- `stream show` without references;
- the exact `before` key rule, and the merge onto `before`;
- `directory` as an editable field set by `--directory`, with no branch flag and no bindings or registry;
- `stream_id` as an editable field, with `card watch` moving a Session through the same edit;
- `title` as `{text, manual}` and `goal` as `{text, short, manual}`;
- the key `HOST/HARNESS/ID`, with partial keys and unique prefixes for commands that act on an existing record;
- relative paths only in `--input -`, and absolute paths converted by the document flag;
- the mod's refresh when focus moves within a tab;
- one cutover, with one plan;
- reference entries with `status` and `manual`, and the CLI's reference flags setting `manual: true`;
- `recap`, `state` and `remaining` as `{text, short}` or `null`, with a clear flag;
- `host_id` in place of the Stream's `owner`;
- artifact keys of the form `HOST/HARNESS/ID/NAME`, and a Stream's `artifacts` computed from its Sessions;
- the document flag's fallback, which resolves the parent directory only;
- the plan citing this design's test cases by area;
- each Session's own directory, `hosts/HOST/sessions/HARNESS/ID/`, with `session.json` and `artifacts/`;
- Pi's automatic report as a Session edit and a Stream edit, each with its own `before` and result;
- cards and the sidebar opening the path they were given at refresh;
- `session resume` running the harness with the Session ID, with no transcript lookup;
- the edit's checks running inside the store's save callback.

No changes remain.

## Open Questions

None.
