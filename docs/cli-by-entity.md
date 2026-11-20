# A Streams CLI organised by entity

- Author: Wendell Smith
- Status: Approved
- Date: 2026-11-19

---

## Overview

Streams records durable work. A Stream is a continuing effort, and a Session records one agent conversation's contribution to a Stream. The `streams` command line is how people, agents, the Pi extension and the herdr-app mod read and change those records.

This design reorganises the command line around the things it manages. Each entity gets one namespace:

- Stream;
- Session;
- Artifact;
- Config;
- the Catalog as a whole.

Every entity has one set of field names, used for reading, choosing output and editing. Field changes go through one `edit` command per entity, with one rule for concurrent writers. The herdr-app mod becomes the herdr-app frontend. It reads records through the public commands and opens the spaces, tabs and terminals, so `streams open` and the Rust code that opened work in hc, Herdr and cmux are removed. The classic Herdr work cards stay as they are, changed only where an interface they use changes. Machine and developer commands are hidden from the everyday help.

The work lands after the `record-format` branch, which gives every record its own revision and update time. It lands together with the design in `docs/session-references.md` on the `session-references` branch, which moves references from Streams to Sessions. The References and Documents sections and the Session's `directory` follow that design.

## Terms

- Stream: a durable record of one effort, owned by one host.
- Session: the record of one agent conversation's contribution. It belongs to one Stream.
- Artifact: a named file a Session saved into the catalog.
- Catalog: the directory of Stream and Session records, optionally synced through Git.
- Full key: the key that names exactly one record. A Stream's is `HOST/ID`, the owning host's ID and the Stream's ID. A Session's is `HOST/HARNESS/ID`, where `ID` is the harness's own ID for the conversation, saved as given. `docs/session-references.md` defines both.
- Partial key: a full key with leading parts left out, such as `HARNESS/ID` or `ID`, or a unique prefix of a bare ID.
- Field: a named value of a record. A stored field is saved in the record, and a computed field is worked out when it is read.
- Revision: a counter in each record that rises with every save that changes it.
- Edit result: the requested fields whose saved value differs from the requested one after an edit, each with its current value. An empty result means every requested field holds the requested value.
- Status: one of `active`, `waiting`, `blocked`, `parked`, `done` and `closed`, saved on every Stream and Session. `done` and `closed` mean the work is finished.
- Effective status: a Stream's status as lists show it. A Stream saved as `done`, `closed`, `blocked` or `waiting` shows that status. A Stream saved as `active` or `parked` shows `active` if any member Session is active, then `blocked`, then `waiting`, and otherwise `parked`. An unfinished Stream on this host with no activity for `lifecycle.decay_days` shows `closed`.
- Manual mark: the `manual` member of a title or goal. `true` means a person chose the text, and automatic reports leave it alone. A record created with only `--title` or `--goal` starts with `true`, and one created with `--title-mode automatic` or `--goal-mode automatic` starts with `false`. The settings `naming.stream` and `naming.session` say whether reports propose titles at all.
- Recap, state and remaining: a Session's account of what it did, where it stands and what is left. Each holds a text and an optional short form for narrow displays.
- Enrolled: a conversation is enrolled when a Session record exists for it.
- Automatic report: the edit an agent sends after each turn to update its Session and Stream.
- Harness: the agent program behind a Session, such as Pi or Claude Code.
- Pi extension: the Streams package inside Pi. It runs the public commands, and starts `streams card watch` beside each conversation for classic Herdr.
- herdr-app: the terminal workspace app built on hc. The mod is the Streams plugin inside herdr-app, in `integrations/herdr-app/`.
- hc: herdr-core, which keeps herdr-app's terminals running. A terminal can carry labels, which are `key=value` pairs that no window shows.
- Classic Herdr: the terminal multiplexer with workspaces, panes and plugins that came before herdr-app. Streams talks to it over its socket from Rust.
- Work card: the classic Herdr pane that shows one Session's progress and references beside its agent. `card pane` runs it, from the plugin in `integrations/herdr-work-cards/`, and `card watch` opens it beside each Pi.
- cmux: another terminal workspace app, which Streams could also open work in.
- Prompt lab: the developer tool for replaying and testing report prompts.

## The problem

A person reading `streams --help` meets machine interfaces first. The list opens with `integration`, `serve` and `debug`, and most everyday commands carry no description. There are 24 top-level commands, and several of them do the same job:

- `start` and `create --open` create a Stream.
- `close`, `archive`, `restore` and `update --status` change a Stream's status.
- `config` and `config list` print the same settings, and `--overrides` is accepted in two places.
- `--session` and `--session-key` select the same Session with different lookup rules.

Some flag names change meaning between commands. `--session-id` is a harness conversation ID in `session attach` and a catalog Session ID in `artifact write`. `--repository` is a URL in `checkout register` and a Stream's repository alias in `session directory`. Both produce commands that look right and act on the wrong thing.

Two read commands can write. `list --refresh` and `show --refresh` run a full sync first, and a sync can commit and push local changes.

Agents edit records through a separate `agent` command set. Its requests carry a captured copy of the record, an actor, a catalog source and a membership check, and its outcomes differ from a person's edit. Two edit paths have two sets of rules to learn and to keep correct.

The herdr-app mod reaches Streams through `integration describe`, `integration request`, `integration view-document` and `integration bind-space`. The mod asks for the same things the public CLI offers, through a request vocabulary of its own, and the two drift apart. Streams also drives herdr-app from Rust when it opens work, while the mod drives the same window from JavaScript. Two programs then decide how a space, tab or terminal looks.

## Goals

- A person can tell from `streams --help` what each command is for. Everyday commands come first, each with a one-line description, and machine and developer commands are hidden from that list.
- Each entity has one namespace, and a verb used in more than one namespace means the same thing in all of them.
- Each job has one command. The duplicates in the problem statement are removed.
- Each flag name has one meaning across the whole CLI.
- Each entity has one set of field names. Reading, choosing output and editing all use them.
- A person edits several fields of a record in one command. Agents use the same `edit` command, and one rule decides what concurrent writers save.
- A record argument names exactly one record. A key that doesn't exist is an error and never falls back to a title search.
- Read commands, which are `list`, `show` and `read`, never write, sync, publish or start a process. The `view` commands start only the viewer.
- Finite commands print readable text by default and a documented structured result with `--json`. The exit status says whether the command did all, part or none of what was asked.
- The herdr-app mod uses the public commands, and it alone arranges the herdr-app window.
- Every caller in the repository moves to the new commands in the same change. That covers the Pi extension, the Streams skill, the herdr-app mod, the work-card scripts, the README and `docs/`.

## Non-goals

- Changing the record format beyond `record-format` and `session-references`.
- Compatibility aliases for old command names. Streams is alpha, so the change is one cutover, and the changelog lists the renames.
- Opening work in cmux, classic Herdr or any frontend other than the herdr-app mod. Such a frontend can be written later against the public commands.
- Changing the classic Herdr work cards. `card pane` and `card watch`, which is today's classic Herdr path in `serve`, keep what they do today, including opening a card beside each Pi, naming workspaces and moving a Session when its pane moves to another workspace. They change only where a record, field or function they use changes. They could later become a separate program.
- Changing the prompt lab's commands, beyond dropping the workspace capture target. They move under `debug prompts`, and the lab reads records through the public commands, as phase 3 describes.
- Renaming configuration keys. The Pi settings screen uses them as they are.
- A query language for `list`. Title and goal search stays a plain substring match.
- A long-lived server, a batch command or any retry machinery for the herdr-app mod.

## Operating assumptions

- The users are one person at a terminal, agents following the Streams skill, the Pi extension, the herdr-app mod and the work-card scripts. All of these callers live in this repository and change in the same commit.
- Scripts outside the repository are few, and they can be updated from the changelog.
- A Streams process starts in about 10 ms, so the herdr-app mod can run one or two commands per refresh.
- Hidden help only shapes what `--help` shows. Any caller can run a hidden command.
- People edit while watching. Automatic reports run after each agent turn, and a report that is partly refused is acceptable, because the next turn reports again.

## Failure boundaries

- An unknown or old command name fails with the parser's usage error. No old name is mapped to its replacement.
- Every command names the records it reads or writes. No selector is worked out from the environment, the working directory, the window or a title.
- A missing key fails. A partial key that matches several records lists their full keys and changes nothing.
- An edit fails as a whole and writes nothing when it names an unknown, read-only or computed field, when its `before` has different keys from `changes`, or when it has no `before` and a value with a member missing.
- An edit of a record owned by another host fails. Only the owning host writes a record.
- Changing `catalog.root` or a `herdr` setting while the herdr-app mod runs needs an app reload. This is a written step with no code behind it, because a person makes these changes on purpose and rarely.
- `session resume`, `artifact read`, `artifact view`, `session document read` and `session document view` reject `--json` before they start anything.
- `session resume` makes no check that the Session already runs elsewhere. A person runs it while watching, and the docs give the check as a written step.
- Two catalogs on one machine, such as a development build beside the installed one, share hc labels unless each has its own hc core. The docs say to give each its own core.

## Open questions

None.

## Critical user journeys

### CUJ 1: Return to work

- Given a person has several active Streams on this host.
- When they run `streams list`, then `streams session list --stream 7c1e`, then `streams resume 01a1` in a free terminal, using unique prefixes of the IDs the lists printed.
- Then the list shows each Stream's full key, effective status, title and last activity, most recent first. `resume` runs the Session's harness in that terminal, in the Session's directory. Nothing is synced or published.

### CUJ 2: Correct the current Stream

- Given a person is in a shell in a herdr-app space, and the sidebar shows the Stream's ID, which starts `7c1e`.
- When they run `streams stream edit 7c1e --goal "Ship the CLI redesign" --status waiting`.
- Then both fields are saved in one save, and the output names the Stream by full key and says the edit was saved. If the prefix matches more than one Stream, the command fails, and the error lists their full keys.

### CUJ 3: An agent reports progress

- Given an agent following the Streams skill, outside Pi, in an enrolled conversation.
- When it runs `streams session show claude/ID --json`, builds its report, and sends it with `streams session edit HOST/claude/ID --input -`, giving the values it read as `before`.
- Then each field nobody changed since the read is saved. The result lists any field that someone else changed, with its current value, and the exit status is 3 when the list is not empty. Sending the same request again gives the same result and writes nothing new.

### CUJ 4: The herdr-app mod refreshes a tab

- Given the mod shows a tab whose focused terminal runs an enrolled Pi Session.
- When it reads the Session key from that terminal's label, runs `streams session show HOST/HARNESS/ID --json`, and then runs `streams stream show STREAM --json` for the Stream the Session names.
- Then the Session supplies the sidebar, with its references, recap, state, remaining work and document paths. The Stream supplies the goal and the member Sessions for every pane header. A visible terminal whose Session belongs to another Stream adds one `streams session list --key KEY… --all --json` read.

### CUJ 5: Add a document and open it

- Given Session S runs in `~/src/streams/feature-x`, and its ID is the only one starting `01a1`.
- When a person runs `streams session edit 01a1 --add-document docs/plan.md --name Plan --purpose "The plan" --role brief`, and then opens the reference in the app.
- Then the reference is saved on S, and the app shows `~/src/streams/feature-x/docs/plan.md` through `streams session document read` or `streams session document view`. No Git or jj process runs. If S has no directory on this host, the app shows that the document is not available here.

### CUJ 6: A script reads a Session

- Given a script holds a Session's full key from an earlier `--json` result.
- When it runs `streams session show HOST/HARNESS/ID --json --fields title,status`.
- Then it receives an object with `key`, `title` and `status`. If the key doesn't exist, it receives a structured not-found error and exit status 1.

## Technical plan

### Pieces

The command line has three layers.

- The command tree in `src/cli/args.rs` defines every command, flag and help text. It is rewritten around the entity namespaces below.
- One handler per namespace in `src/cli/` turns parsed arguments into calls on the application layer and renders the result as text or JSON. The handlers are rewritten with the tree.
- The application layer in `src/application/` reads and saves records. It holds the operations every caller uses. It gains the one edit operation and loses the code that opened work.

```text
 person, skill agent      herdr-app mod          Pi extension        work card
        |                       |                      |                  |
  streams stream edit     streams stream show    streams session edit  streams card pane
        |                       |                      |                  |
        v                       v                      v                  v
 +----------------------- command tree (src/cli/args.rs) ------------------------+
 | stream  session  artifact  config  catalog  |  card  debug                    |
 +---------------------------------+---------------------+---------+-------------+
                                   v                               |
              namespace handlers (src/cli/*.rs), text or --json    |
                                   v                               v
                    application layer (src/application/*)  <-------+
                                   v
                          record store
```

Every caller reaches the same application layer. `card pane` and `card watch` call it from their own modules in `src/cli/cards*`, and watch the record files for changes. Everything else goes through the namespace handlers.

### Current command tree

This is the tree on the `herdr-app` branch, with each command's arguments shortened.

```text
streams
  init       --sync none|git [--remote URL] [--branch BRANCH]
  config     [list|get KEY|set KEY VALUE|unset KEY|path] [--overrides]
  frontend   get [--frontend F]
  sync
  list       [QUERY] [--status S]… [--closed|--archived|--all] [--refresh]
  show       [STREAM | --current] [--refresh]
  create     TITLE [STREAM FIELDS] [--cwd DIR]
  start      TITLE [STREAM FIELDS] [--cwd DIR] [--frontend F]
  open       [STREAM | --current] [--session REF | --session-key HOST/ID] [--cwd DIR]
  resume     [STREAM | --current] [--session REF | --session-key HOST/ID] [--cwd DIR]
  update     [STREAM | --current] [--title TEXT] [STREAM FIELDS]
  close      [STREAM | --current]          alias: archive
  restore    [STREAM | --current]
  check      [STREAM]
  tidy       [--days N]
  session    list [--stream STREAM]
             show SESSION
             attach [STREAM | --current] --harness H --session-id NATIVE_ID --title TEXT [--cwd DIR] [--repository ALIAS --ref REF] [--directory-within-checkout PATH]
             update SESSION [--title TEXT] [SESSION FIELDS]
             directory SESSION [--cwd DIR | --checkout ID] [--within PATH] [--repository ALIAS]
             lifecycle resume|park --harness H --session-id NATIVE_ID
  reference  list STREAM
             add|edit STREAM KEY [REFERENCE FIELDS]
             drop STREAM KEY
             restore STREAM KEY --state active|fulfilled
             delete STREAM KEY --confirm
  artifact   write NAME --session-id CATALOG_ID --description TEXT [--stream-id ID] [--file FILE | --description-only]
             read|view|delete HOST/STREAM/SESSION/NAME
             list [--host HOST] [--stream-id ID] [--session-id CATALOG_ID]
  checkout   list
             show ID
             register DIRECTORY [--repository URL]
             document ID PATH --revision REF
             move ID DIRECTORY
             default ID REPOSITORY_URL
             resolve REPOSITORY_URL
  agent      context --harness H --session-id NATIVE_ID [--stream-id ID]
             apply|enroll|preview            (JSON on stdin)
             capture-inputs [--stream-id ID | --workspace-id ID]
  serve      --session-id NATIVE_ID --plugin-root PATH
  integration describe --frontend hc
             request --request-json JSON
             view-document --scope-json JSON --reference-json JSON [--mode M]
             bind-space --space-ref JSON --stream ID --scope-json JSON
  native     observe|open|project|check|focus|close   (native caller flags)
             pane                                     (hidden)
  debug      capture|prepare|run|replay|inspect|list  (prompt lab, forwarded to Node)
```

### Proposed command tree

Every command in this tree is planned, and none exists yet.

```text
streams
  list    …                                       same as `stream list`, with its arguments
  resume  SESSION                                 same as `session resume`

  stream list     [QUERY] [--status S]… [--all] [--host HOST | --local] [LIST FLAGS]
  stream show     STREAM [SHOW FLAGS]
  stream create   --title TEXT [STREAM FIELDS]
  stream edit     STREAM (STREAM FIELDS | --input -)

  session list    [QUERY] [--stream STREAM] [--key SESSION]… [--status S]… [--all] [LIST FLAGS]
  session show    SESSION [SHOW FLAGS]
  session create  HARNESS/ID --stream STREAM --title TEXT
                  [SESSION FIELDS]
  session edit    SESSION ([SESSION FIELDS] [REFERENCE FLAGS] | --input -)
  session resume  SESSION
  session document read  SESSION PATH
  session document view  SESSION PATH

  artifact list    (--session SESSION | --stream STREAM) [LIST FLAGS]
  artifact read    ARTIFACT
  artifact view    ARTIFACT
  artifact write   NAME --session SESSION --description TEXT [--file FILE | --description-only]
  artifact delete  ARTIFACT --confirm

  config get    [KEY | --overrides]
  config set    KEY VALUE
  config unset  KEY
  config path

  catalog init   --sync none|git [--remote URL] [--branch BRANCH]
  catalog sync
  catalog check  [STREAM]
  catalog tidy   [--days N]

hidden from help, documented in docs/
  card   pane
  card   watch  HARNESS/ID --plugin-root PATH
  card   open   HARNESS/ID --plugin-root PATH
  card   close  HARNESS/ID
  card   mode   HARNESS/ID [automatic|manual]
  debug  prompts …
```

The tree has five namespaces and two root aliases. Each alias means exactly the same as the command it names.

The flag bundles are these:

- LIST FLAGS are `--fields NAMES`, `--limit N` and `--json`. SHOW FLAGS are `--fields NAMES` and `--json`. The section on output explains them.
- STREAM FIELDS are `--title`, `--title-mode`, `--goal` or `--goal-file` or `--clear-goal`, `--goal-mode` and `--status`. `stream create` and `session create` take the same field flags as `edit`, and require `--title`.
- SESSION FIELDS are `--title`, `--title-mode`, `--status`, `--stream` and `--directory`, and `--recap`, `--state` and `--remaining`. Each of the last three has a file form and a clear form, such as `--recap-file` and `--clear-recap`.
- `--goal`, `--recap`, `--state` and `--remaining` each take an optional short form, such as `--state-short SHORT`, which is valid only beside its field's flag. The short forms appear under an advanced heading in help.
- `--status S` keeps records with that status, and `--all` includes closed records, which `list` leaves out otherwise. `--status closed` includes them without `--all`. For Streams, `--status` matches `effective_status`.
- `--local` keeps Streams owned by this host. It cannot be combined with `--host`.
- REFERENCE FLAGS add or drop references. One of `--add-pr URL`, `--add-issue URL`, `--add-link URL` or `--add-document PATH` adds one reference, and needs `--name`, `--purpose` and `--role`. Repeatable `--drop-reference KEY` drops references.

These commands leave or are absorbed:

- `start`, `open`, `close`, `archive`, `restore`, `update`, `show`, `check`, `tidy` and `sync` at the root, and the flags `--refresh`, `--closed`, `--archived`, `--current`, `--session-key` and `--frontend`.
- Root `open`, today's root `resume` with its Stream and `--current` forms, and `create --open`. The herdr-app mod opens work, and `session resume` resumes one Session in the caller's terminal. The section on the herdr-app mod explains this.
- `reference list`, `add`, `edit`, `drop`, `restore` and `delete`. References move to Sessions, `session show` lists them, and REFERENCE FLAGS on `session edit` add and drop them.
- `checkout` in full, with the checkout registry and the directory bindings. A Session saves its own directory, as `docs/session-references.md` describes.
- `session attach` becomes `session create`. It creates the Session record for a conversation that already exists, as `stream create` creates a Stream.
- `session directory` becomes `session edit --directory`, and the directory appears in `session show`. `--cwd` becomes `--directory` everywhere, matching the field.
- `session lifecycle` and the whole `agent` set. Agents read with `session show` and `stream show`, write with `edit --input -`, and enroll with `session create`. The Pi extension runs the same commands, as the section on machine interfaces lists.
- `serve`. Its record requests become public commands, and its classic Herdr path becomes `card watch`.
- `integration` in full. The herdr-app mod runs public commands.
- `native` in full. `native pane` becomes `card pane`. The other `native` commands are card actions that `serve` ran for the extension, and `card open` and `card close` replace them. `frontend get` served opening work from Rust, which is removed. `frontend.mode` stays, because `card watch` uses it to decide whether to run.

### Fields

Each entity has one set of named fields. The same name is:

- the JSON key in `show` and `list` output, and the column name in text output;
- a name accepted by `--fields`;
- a key in an edit's `changes` and `before`;
- the stem of the matching flag, so `--goal` sets `goal`. The one exception is `--stream`, which takes a Stream selector and sets `stream_id`.

Each field is in one of three classes.

- Editable fields are stored fields that an edit may write.
  - Stream: `title`, `goal` and `status`.
  - Session: `title`, `stream_id`, `status`, `recap`, `state`, `remaining`, `directory` and `references`.
- Read-only fields are stored fields that only Streams writes, such as `id`, `host_id`, `harness`, `revision`, `updated_at_ms` and a Session's `artifacts`, which maps each artifact's name to `{description}` and which `artifact write` and `artifact delete` keep. Their names are the record's names, as `record-format` and `session-references` define them.
- Computed fields are worked out when a record is read.
  - Stream: `key`, `effective_status`, `active_sessions`, `total_sessions`, `last_activity_ms`, `sessions` and `artifacts`.
  - Session: `key` and `document_files`.
  - Artifact: `key`.

An edit that names a read-only or computed field fails as a whole.

`recap`, `state` and `remaining` are each one field holding `{text, short}`, as `session-references` defines. A title is one field holding `{text, manual}`, on Streams and Sessions, and a Stream's goal is one field holding `{text, short, manual}`. A flag writes the whole field, so a flag never leaves a short goal beside a different goal. A field or member with no value is `null`, never empty text and never left out, so `recap`, `state`, `remaining` and `goal` are `null` when they are clear.

The computed fields hold these values:

- `effective_status` is the effective status from the terms.
- `last_activity_ms` is the latest update of the Stream or any member Session.
- A Stream's `sessions` lists each member Session's `key`, `title`, `status`, `harness` and `state`.
- A Stream's `artifacts` lists each artifact its member Sessions hold, with the artifact list's default fields. An artifact belongs to its Session alone, so a moved Session takes its artifacts with it and their keys do not change.
- `document_files` maps each document reference's path to `{"path": FULL_PATH}`, or to `{"reason": TEXT}` when it has no file here.
- An artifact's `key` is its Session's key followed by `/NAME`, which is `HOST/HARNESS/ID/NAME`.

A read works out the computed fields it prints and those its filters and sort need. `stream list` reads member Sessions anyway, to filter by effective status and sort by activity. `document_files` resolves files on disk, so only a read that prints it does that work, and `session list` does so only when `--fields` names it.

### Selecting a record

Every STREAM and SESSION argument names an existing record, and resolves to one record or fails. It accepts the same forms.

- A full key names exactly one record, and reads one file.
- A partial key leaves out leading parts. A Session takes `HARNESS/ID` or `ID`, and a Stream takes `ID`. A bare ID can also be a unique prefix. A partial key matches against the existing records, and lists their directories to do it. When it matches more than one, the error lists their full keys.
- There is no selector for the caller's own record. An agent in Pi names its Session as `"$PI_SESSION_ID"`, a bare-ID selector, because Pi sets that variable on each shell command it runs for the model. An agent elsewhere names it as `HARNESS/ID`, from its harness and the harness's ID for the conversation. Either finds its Stream from the Session's `stream_id`. A person types a prefix from `list` or from the app's sidebar. Streams reads no Pi variable and no hc label to select a record.
- A conversation with no Session gives not-found, which tells an agent to run `session create`.
- Free text is accepted only by `list`, as the positional QUERY. For Streams it matches titles and goals, and for Sessions it matches titles.
- `session list --key` leaves out keys that name no Session, so a caller can pass keys it read from terminals that may be stale.

An ARTIFACT argument is its full key, or the same key with leading parts left out.

`session create` and the `card` commands take exactly `HARNESS/ID`, read as `HOST/HARNESS/ID` for this host, for a Session that may not exist yet. They refuse a bare ID, a prefix or a full key, and never match against existing records. Every write to another host's record fails.

### Output

Every finite command prints text by default and its documented result with `--json`. Text names each record by its full key. `artifact read` and `session document read` print the file's exact bytes and reject `--json`. `config get` prints TOML for a section and a plain value for a key. `session resume`, `artifact view` and `session document view` take over the terminal.

`show` prints every field of the record, stored and computed. `--fields NAMES` limits the output to the named fields, in that order, and `key` is always included. An unknown name is an error that lists the valid names, which is also how a person finds them. In JSON, `show` prints one flat object.

`list` prints a default set of fields, the same in text and JSON:

- Stream: `key`, `effective_status`, `title` and `last_activity_ms`;
- Session: `key`, `status`, `title` and `updated_at_ms`;
- Artifact: `key`, `name` and `description`.

`--fields` chooses others, as for `show`. In `list` text, a field that holds a collection, such as `sessions`, `artifacts`, `references` or `document_files`, is an error that points to `--json`. A title, goal, `recap`, `state` or `remaining` prints its `text`, or `-` when it is `null`, and `directory` prints its `path`. Streams and Sessions are sorted by most recent activity, and artifacts by key. `--limit N` keeps the first `N`, and there is no default limit. When a limit cuts a list, text ends with "N more; raise --limit", and JSON sets `"more": true`. In JSON, a list is this object:

```json
{
  "items": [{ "key": "HOST/ID", "effective_status": "active", "title": { "text": "…", "manual": false }, "last_activity_ms": 1763500000000 }],
  "unreadable": [{ "kind": "session", "host": "HOST", "path": "…", "cause": "…", "repair": "…" }],
  "more": false
}
```

`unreadable` names each record that could not be read, with how to repair it. Its entries keep today's shape, which the Pi extension and the mod already parse.

A failure with `--json` prints one structured error on stdout:

```json
{ "error": { "kind": "not-found", "message": "no Stream HOST/ID" } }
```

This is today's shape from `CliError::data`. When `show` cannot read the record, the message gives the cause and the `repair` text that a `list` entry would carry. `kind` is `not-found`, `ambiguous`, `invalid-ref` or `error`, and an ambiguous partial key also gives `candidates`, the full keys it matched. `unsupported-command` goes with the commands that needed it.

`stream create` and `session create` print the new record's key and the file they wrote:

```json
{ "key": "HOST/HARNESS/ID", "files": ["…/sessions/HARNESS/ID/session.json"] }
```

`artifact write` and `artifact delete` print the key they changed in the same way. The `catalog` commands print their report in the shape they have today.

The exit status is the same for every command.

| Status | Meaning |
|---|---|
| 0 | Done. |
| 1 | Failed. With `--json`, the structured error is on stdout. |
| 2 | Usage error from the parser. |
| 3 | Partly done, or a requested change not written. The result says which part did not happen. |

Exit 3 means the command ran and printed its usual result, and part of what was asked did not happen. Exit 1 means the command failed and printed an error. The cases of 3 are these:

- an edit with a non-empty result;
- `catalog sync` that leaves conflicts.

Today every failure and partial result exits 2. Phase 2 moves each site to 1 or 3 by this rule. Each status comes with a message that names what to do next.

### Edits

`stream edit` and `session edit` take field flags, or `--input -` with a JSON body on stdin. They cannot be combined. `--json` controls only the output.

The body is two objects of fields:

```json
{
  "changes": { "goal": { "text": "Ship the CLI redesign", "short": "CLI" }, "status": "waiting" },
  "before": { "goal": { "text": "Draft the CLI redesign", "short": null, "manual": false }, "status": "active" }
}
```

- `before` is optional. When it is present, its keys must be exactly the keys of `changes`, or the edit fails as a whole. This is the rule `docs/session-references.md` gives, and it applies inside `references` too.
- With `before`, each value in `changes` is merged onto its `before` value by JSON Merge Patch (RFC 7386). Objects merge member by member, an absent member keeps its `before` value, and `null` sets a member to `null`. A field is written only if its saved value equals its `before` value.
- Without `before`, every value must be complete, and an object with a member missing fails the edit as a whole. Every field is then written whatever its saved value.
- Flags send no `before`, so a flag always writes. A flag that changes part of a field sends the whole field as it read it, with that part changed.
- Every written field lands in one save of the record, and an edit writes no other record. Closing a Stream leaves its Sessions' statuses as they are, and `catalog tidy` closes Sessions that have gone quiet.
- The result lists each requested field whose saved value differs from the requested one after the save, with its current value. An empty result means every requested field holds the requested value, including a request that changed nothing.

The result in JSON names the record and the fields not written:

```json
{ "key": "HOST/ID", "not_written": { "goal": { "text": "Ship the redesign by Friday", "short": null, "manual": true } } }
```

A flag writes the whole field. `--state TEXT` writes the text with `short` set to `null`, `--state TEXT --state-short SHORT` writes both, and `--clear-state` sets `state` to `null`. This matches what the code does today when the text changes. `--title TEXT` and `--goal TEXT` write the text with `manual` set to `true`. `--title-mode automatic|manual` and `--goal-mode automatic|manual` set `manual` to `false` or `true`, alone or beside the text flag, and win over the mark the text flag implies. A goal that is `null` has no mark. `--goal-mode` beside `--clear-goal` is a usage error, and `--goal-mode` alone on a Stream whose saved goal is `null` fails the edit as a whole.

An agent reads with `show --json`, builds its changes, and sends the values it read as `before`. A field a person changed in the meantime keeps the person's value and appears in the result, and the agent's next report sees it. The flags mean the same in `create` as in `edit`, and a body sent with `--input -` implies nothing. An agent leaves a manual title or goal out of its changes. When it writes one, its `before` holds the value it read, with `manual` set to `false`, so a person who marks the text manual during the report makes that write fail. The Pi extension also proposes no titles when `naming.stream` or `naming.session` is `manual`. The CLI does not enforce the marks, because they are not an access control.

A Session's `directory` is `{path, repository: {url, within, branch}}`, or `null`, as `docs/session-references.md` defines it. `--directory DIR` resolves `DIR` with `fs::canonicalize`, inspects the clone once to fill in `repository`, and saves the whole field with any other flags in one save. This is the only place a command starts Git or jj. A report updates the branch with `--input -`, sending `directory` with only `branch` changed and the directory it read as `before`. A person refreshes the branch by running `--directory` again, so there is no branch flag.

`session edit --stream STREAM` moves a Session by writing `stream_id`. `stream_id` is the Stream's ID on the Session's host. The edit reads `hosts/HOST/streams/ID` for the Session's host, and fails as a whole if that Stream does not exist. The `--stream` selector also refuses a Stream on another host, before the edit. It writes only the Session record, and the Stream's `sessions` field is worked out from the Sessions when it is read. The classic Herdr path moves a Session when its pane moves to another workspace by calling the same edit, with `before` holding the old `stream_id`, and still creates a Stream first when the workspace has none.

The edit has no actor, catalog source, captured context or membership check. The old `agent apply` outcomes of superseded and conflict become the result list.

### Creating a Session

`session create HARNESS/ID --stream STREAM --title TEXT` creates the Session record for an existing conversation. `ID` is the harness's own ID for the conversation, and it must be a safe token, as `docs/session-references.md` defines. The host is always this host, so the key is `HOST/HARNESS/ID`.

- `--stream STREAM` names an existing Stream, and the command fails if it does not exist. A conversation that starts new work runs `stream create` first, then `session create` with the key it printed. Each command saves one record and either succeeds or fails. If the second fails, the new Stream stays with no Sessions, and a person closes it.
- The Session takes the SESSION FIELDS that `edit` takes. An agent naming its own Session passes `--title-mode automatic`.
- Running the command again for the same Stream changes nothing. A different Stream is refused, and `session edit --stream` moves a Session.

### References

References belong to Sessions, as `docs/session-references.md` describes. Each Session holds a map from a reference's key to its entry. The key is the URL for a pull request, issue or link, and the path relative to the Session's directory for a document. An entry holds a kind, name, purpose, role, status and manual mark. Its status is `active`, `fulfilled` or `dropped`.

Agents write most references. A person adds what an agent missed and drops what is wrong. The flags serve those two jobs.

```sh
streams session edit "$PI_SESSION_ID" --add-pr https://github.com/org/repo/pull/43 \
  --name "Add session references" --purpose "Implements this design" --role deliverable
streams session edit "$PI_SESSION_ID" --drop-reference https://github.com/org/repo/issues/12
```

- One command adds at most one reference, because `--name`, `--purpose` and `--role` belong to it. Every entry has a name, a purpose and a role from whoever adds it, as `docs/session-references.md` requires, and Streams fills in none of them.
- The flag names the kind, and Streams never works it out from the URL.
- Adding a key that already exists writes the new name, purpose and role, with status `active`. That also restores a dropped entry.
- `--drop-reference KEY` sends the entry it read with status `dropped`. The entry stays, so a report never adds it back.
- Every reference flag sets `manual` to `true`. A report sets `manual` to `false` on the entries it adds, and leaves a manual entry's name, purpose and role alone.
- A URL key is saved exactly as given.
- `--add-document` turns an absolute path into a key relative to the Session's directory. It refuses a path with a `..` component, removes `directory.path` with `Path::strip_prefix`, and if that fails, resolves the longest existing ancestor of the path's parent directory with `fs::canonicalize`, keeps the final name unchanged, and tries again. A symlinked file keeps its own name. Any other path fails, with an error that names the directory and suggests `artifact write` or a link. `--input -` takes only relative document paths.
- `session create` takes no reference flags. A new Session adds references with `session edit`.

Changing a name, purpose or role uses `session edit --input -`. Each entry is one unit under `references`, and `before` holds the same keys:

```json
{
  "changes": { "references": { "docs/plan.md": { "kind": "document", "name": "Plan", "purpose": "The agreed plan", "role": "brief", "status": "active", "manual": true } } },
  "before": { "references": { "docs/plan.md": { "kind": "document", "name": "Plan", "purpose": "Draft plan", "role": "brief", "status": "active", "manual": true } } }
}
```

`stream show` lists Sessions and no references.

### Documents

A document reference resolves to `directory` joined with its path. The file must be inside the directory and must be a regular file. Resolving reads only the filesystem and starts no Git or jj process. `session show` gives each result in `document_files`, either the full path or the reason there is none, such as no directory on this host or a missing file.

Two commands open a document, as `artifact read` and `artifact view` open an artifact:

- `session document read SESSION PATH` prints the file's exact bytes.
- `session document view SESSION PATH` takes over the terminal with the viewer that `herdr.viewer.mode` names: the editor, `less` or `hunk`.

The mod's runtime cannot read files, so its native viewer shows what `session document read` or `artifact read` prints. The mod refuses binary or non-UTF-8 content and cuts long content, the checks `prepare_native_content` in `src/cli/integration.rs` makes today. Otherwise the mod opens a pane running `session document view` or `artifact view`. Both commands resolve the path with `documents::locate` when they run, so a person and the mod get the same file or the same reason from them. The work card opens the full path its last refresh gave, as `docs/session-references.md` describes, so its containment check holds as of that refresh.

Moving a worktree means running `session edit --directory NEW` for each of its Sessions. The README gives that step, and the steps for recreating a deleted worktree from `directory.repository`.

### The herdr-app mod

The mod is the herdr-app frontend. It reads records through the public commands, and it is the only program that arranges the herdr-app window.

Two hc labels connect terminals to records. Each has one writer.

- `streams.session`, holding a Session's full key. The Pi extension writes it on its terminal as Pi starts, with `hc label "$HC_TERMINAL_ID" streams.session=KEY`. The value is the Session's key when the conversation is enrolled and empty otherwise, which clears it. The extension writes the key again when the conversation enrolls later. It is not removed on exit.
- `streams.stream`, holding a Stream's full key. The mod writes it on each terminal it opens for a Stream. At each refresh it also compares, on each visible terminal running a Session, the label with that Session's Stream, and writes the label when they differ. A space a person opened by hand, and a Session that moved, then get the right label.

The CLI reads neither label.

The mod treats a terminal as running a Session when it carries `streams.session` and Herdr reports a live Pi agent in it. Because each Pi writes the label as it starts, a new Pi in a terminal never shows the previous Pi's Session.

After any save, the CLI writes the fixed hc label `streams.saved` with a new token on the terminal that `HC_TERMINAL_ID` names. A failed write prints a warning and does not fail the save. The mod watches that label on every terminal and reads again when a token changes. A save made outside hc writes no token, and the fallback timer covers it.

A refresh happens when the active space or tab changes, when focus moves to another terminal in the tab, when the visible terminals change, when a save token changes or when a fallback timer fires. It runs these commands:

- `session show SESSION --json` for the focused terminal's Session. It supplies the sidebar, and its `host_id` and `stream_id` form `HOST/ID`, the key of the tab's Stream. The sidebar shows the Stream's ID, so a person can type its first characters as a prefix.
- `stream show STREAM --json` for that Stream. Its `sessions` field supplies the pane headers.
- `session list --key KEY… --all --fields key,stream_id,title,status,state --json`, only when a visible terminal's Session is not among the Stream's `sessions`.
- `stream list --local --all --json --fields key,title,effective_status,last_activity_ms` for the Stream picker, when the tab has no Session.

The usual refresh is two commands. The mod reads `config get herdr --json` once when it starts.

Opening a Stream is the mod's job:

1. It runs `session list --stream STREAM --fields key,status,harness,directory --json`, and keeps the Pi Sessions whose status is `active` or `parked`. That is today's rule in `Status::is_open_eligible`. Every member Session is on the Stream's host, and the picker lists only local Streams, so the mod needs no host filter.
2. It finds the Stream's space as the `herdr.space` label of any terminal labelled `streams.stream` with the Stream's key. When there is none, it picks a fresh space key, which makes Herdr create the space.
3. For each kept Session that no terminal shows as running, it runs `hc run --bg --no-parent --cwd PATH --label streams.stream=STREAM --label herdr.space=SPACE --label herdr.tab=NEW_TAB_KEY streams session resume SESSION`, where `PATH` is the Session's `directory.path`. A Session with no directory here is left out, and the mod says so.
4. It opens documents, artifacts and URLs, as the section on documents describes.

`session resume` checks that the Session has a harness, an ID and a `directory.path`, and replaces itself with the harness, run in `directory.path` with the ID, such as `pi --session ID`. It sets no environment variable, and what the harness does next is its own concern. If it can't resume, it prints why in the new terminal, so the mod needs no resume argument list of its own.

The link from a space to its Stream is the `streams.stream` labels on the space's terminals. The `streams` hc document that the Rust hc code kept for this goes, along with the `streams.work` label it wrote. An empty space is then no longer reused, and the next open creates a new one.

### Machine and developer interfaces

The Pi extension runs public commands, one process per call. It names its own Session as `pi/ID` in every call, taking the ID from `ctx.sessionManager.getSessionId()`. It sets no environment variable.

| Extension need | Command |
|---|---|
| Read its Session and Stream | `session show pi/ID --json`, then `stream show STREAM --json` |
| Enroll in an existing Stream | `session create pi/ID --stream STREAM --directory CWD --title TEXT --title-mode automatic`, where `CWD` is Pi's working directory |
| Enroll in new work | `stream create --title TEXT --title-mode automatic --goal TEXT --goal-mode automatic`, then `session create` as above |
| Report progress | `session edit KEY --input -` for the Session's title, state, recap, remaining work, references and branch, and a separate `stream edit STREAM --input -` for the Stream's title and goal, leaving out each that is manual. Each edit has its own `before` and result. |
| Mark the Session active as Pi starts and parked as it exits | `session edit KEY --input -`, with `before` holding the status it read |
| List Streams for the chooser | `stream list --local --json` |
| Settings, including `naming.*` | `config get --json`, `config set`, `config unset` |
| Sync | `catalog sync` |
| The artifact tool | `artifact list`, `read`, `write` and `delete` |
| An unreadable record | The `repair` text, shown to the person. A `list` carries it in each `unreadable` entry, and a failed `show` in its error message. |
| Open or close the card beside Pi | `card open` and `card close` |
| The card auto-open preference | `card mode` for this conversation, `config set herdr.cards.auto_open` for all |
| Pi's session name | The Session title from its last read, when `pi.naming.session` is on |

Each command either saves or fails, and says which, so the extension does not resend a request.

- `card watch` is the classic Herdr controller, which is today's classic Herdr path in `serve`. The extension starts it beside each conversation and stops it when Pi exits, and it exits at once unless `frontend.mode` selects classic Herdr. It keeps what it does today. It opens the card beside the Pi pane when auto-open is on, names the workspace and tab after the titles, follows the pane to another workspace, and closes the card when Pi exits. It takes no requests. It reads standard input only to see it end, which means Pi has exited, and then closes the card within today's grace period. It sees a new Session by watching the record files, as it already does, so it needs no message when Pi starts or a conversation enrolls.
- `card open` and `card close` open or close the card beside the caller's Pi pane, which `HERDR_PANE_ID` names, and exit 0 when that is done or 1 with the reason. As today, `card open` saves the conversation's card mode as `automatic` and `card close` saves it as `manual`, so `card watch` does not reopen a card a person closed. `/streams pane` and `/streams pane close` run them. When `card watch` and `card open` race, the card's existing-card check and lock make the second open harmless.
- `card mode` prints or sets one conversation's card mode, `automatic` or `manual`, which `src/runtime/card_settings.rs` keeps in a host-local file. A conversation starts `automatic`. `herdr.cards.auto_open` says whether cards open automatically at all, so a card opens on its own when both are `automatic`. This matches the manual marks, which have no `inherit` either.
- `card pane` is the work-card program for classic Herdr. It reads its launch inputs from the environment, and keeps its Herdr client and its own reads of the record files. When a person quits the card, it runs `card mode HARNESS/ID manual` in place of today's control socket.
- `debug prompts` is the prompt lab. It captures records with `stream show`, `session list --stream` and `session show`, and finds the caller's Stream from its Session's `stream_id`, so its workspace target goes. A replay shows the edit body the model produced, so the lab needs no preview of the saved records.
- All of these are hidden from `streams --help` and documented in `docs/`, where people who write callers look.

### Callers

The change is one cutover. Every caller in the repository moves in the same branch:

- the README, `AGENTS.md`, `CONTRIBUTING.md` and `docs/`;
- `skills/streams/SKILL.md` and its references;
- the Pi extension in `extensions/stream-checkpoint/`, which moves from `serve` to the public commands and `card watch`;
- the herdr-app mod in `integrations/herdr-app/`, which moves to the public commands and takes over opening work;
- the classic Herdr work-card plugin in `integrations/herdr-work-cards/`, whose scripts call `card pane`, `artifact view` and the document viewer;
- the commands Streams prints for people to run, in error messages.

`CHANGELOG.md` lists every rename and removal.

## Alternatives considered

### Verb first, with typed targets

Commands such as `streams edit stream:ID --goal …` put the verb first, and a single `edit` covers every kind. `title` and `status` exist on both Streams and Sessions, both use UUIDs, and a bare ID would be ambiguous between them. The kind would have to be written in every target, so `streams stream edit` costs nothing extra and gives help per entity.

### Generic field setters

`streams stream set goal "…"` and Taskwarrior's `goal:"…"` need no flag per field. A setter saves one field per command, or needs a second grammar for several. Typed flags let help list each field with its meaning, and `--goal-file` reads long text. Scripts that want generic access use `--input -`, which names fields directly.

### A separate edit path for agents

The `agent` commands captured a whole record, carried an actor and a catalog source, and returned their own outcomes. Comparing each field with the value the writer read gives agents the same protection with one rule for every writer, and the result shows exactly what did not land.

### A batch command for the app

One process could run several reads against one opened catalog. A refresh needs two commands in the usual case and three at most, and a process starts in about 10 ms. A batch would add an allowed-command list, size limits, a version and a reply format. Results from separate commands can come from slightly different moments, and the next save notice causes another read.

### A long-lived server for the app

A server would keep the catalog open between refreshes. It adds a lifecycle, a cache and stale state, for a cost that one process per refresh already keeps small.

### Looking up a terminal's Session in Rust

The CLI could read hc, check that the Pi process is alive, and return the Session for a terminal. The mod already sees every terminal's labels and Herdr's live agents. A label holding the Session key gives it the answer with no extra process and no states to interpret.

### Selecting the caller's own record

An `@` selector could name the caller's Stream or Session from the environment or from the window. Agents name their Session already, from `"$PI_SESSION_ID"` or their own harness and ID, and read its Stream from `stream_id`. A shell is not a Session, and a tab can hold several, so only the app knows which one a person means, and the app makes that edit. For a Stream, `@` would read hc labels or the classic Herdr workspace marker, fail when a space names two Streams or the caller is in no space, and tie the CLI to frontend state. A prefix from `list` or the sidebar costs a person four characters.

### Opening work from Rust

Streams could keep opening work from Rust in hc, classic Herdr and cmux, and the mod could call `open`. Two programs would then arrange the same window, and the mod would perform window actions the CLI returned to it. Letting the mod open work keeps one owner of the window, and removes opening, presentation and viewer actions from Rust. cmux and classic Herdr lose opening until a frontend is written for them against the CLI. The classic Herdr work cards keep working, because they show work that is already open.

### Reducing the work card to a record viewer

`card pane` could drop its Herdr client, render a Session from the records, and leave placement to whoever opens it. That would remove its event subscription, placement checks and workspace markers. The card works and belongs with its frontend, so it stays as it is, and it could later become a separate program.

### Guessing a reference's kind from its URL

`--add-reference URL` could tell a pull request from an issue by the URL's shape. The rule would depend on each Git host's URL layout. One flag per kind states it.

### Defaults for a new reference

`--add-pr URL` could fill in the name from the key and leave the purpose empty, so several references fit in one command, as `gh issue edit --add-label` adds labels. `docs/session-references.md` requires every entry to carry a name, purpose and role from whoever adds it, so a card never shows an entry that says nothing about why it matters. One reference per command is the cost.

### A document linking spaces to Streams

The Rust hc code kept an hc document that maps each space to its Stream, written with a version check. The mod already labels every terminal it opens with its Stream, so the labels answer the same question with one writer. The document would only add reuse of a space whose terminals are all gone.

### A default limit on lists

`gh` lists 30 items unless told otherwise. Stream lists leave out closed Streams, so they are sized by current work, and a default limit would hide results nobody asked to hide.

## Non-functional requirements

- Listing and showing start no Git or jj process, and write nothing.

## Detailed implementation plan

This design and `docs/session-references.md` land in one cutover on the branch `cli-by-entity`, after `record-format`. This section is the plan for both, and `docs/session-references.md` points here for the order.

Code that this design deletes is deleted, and is not ported first. Phase 1 deletes, on format 3, everything that can go before the model changes. Phase 2 moves to format 4 and builds the new commands together, because format 4 changes what every command reads. Phase 3 moves the callers, and phase 4 writes the documents and the conversion.

Each phase ends with `just build`, `just check-clippy`, `just check-fmt` and `just test-rust-fast`. A phase also edits or deletes each file the compiler names once its own changes are made, and the tables name the files known now. A phase that adds or removes a root test file changes its `mod` line in `tests/contracts.rs`, which `every_root_test_file_is_registered` checks. The Pi extension and the herdr-app mod are broken from phase 1 until phase 3 moves them, so `just test` runs from the end of phase 3. Each phase is one or a few commits.

The classic Herdr path keeps compiling and passing its Rust tests throughout. It is `card pane`, `card watch`, `src/cli/cards*`, `src/frontend/herdr*`, `src/runtime/native_cards*`, `src/runtime/native_markers.rs`, `src/runtime/card_settings.rs`, `src/tui/` and `integrations/herdr-work-cards/`.

### Phase 0: before this branch

- `record-format` lands.

### Phase 1: deleting on format 3

| Where | Change |
|---|---|
| Opening code | Delete `open`, `start`, today's root `resume`, `create --open` and `frontend get`, with `src/runtime/workspace*`, `src/runtime/hc/open.rs`, `src/runtime/hc/launch.rs`, `src/runtime/hc/sources.rs`, `src/frontend/cmux.rs`, `tests/frontend_launch.rs` and `tests/runtime_launch.rs`. In `src/frontend/herdr*` and `tests/frontend.rs`, delete only the code and tests for opening work. `src/runtime/launch.rs` keeps `SessionCommand` and `resolve_executable`. Phase 2 adds `resume` back as the alias of `session resume`. |
| `integration` | Delete the command, `src/cli/integration.rs`, `src/application/integration.rs`, `tests/integration_cli.rs`, `tests/integration_contract.rs` and `tests/integration_hc.rs`. The types from `src/integration.rs` that the classic Herdr path uses move into `src/runtime/native_cards/`. They are `LifecycleEvent`, `LifecycleReport`, `CompanionIntent`, `DesiredProjection`, `NativeRef`, `AssociationIntent`, `NameIntent`, `ConversationId` and `resolve_artifact_summary`. The rest of `src/integration.rs` goes, including the envelopes, `Streams::admit`, `WorkScope`, `ModAction`, `OpenReport` and the view types. |
| `src/runtime/hc.rs` and `src/frontend/hc.rs` | Keep the label write that the save notice needs. Delete the routing, `RegisteredSources`, the space document and the `streams.work` label. `notify_hc_save` in `src/cli/mod.rs` writes the fixed `streams.saved` label. |
| `serve` becomes `card watch` | Move `src/cli/serve/native*` and `src/cli/serve/files.rs` to `src/cli/cards/watch*`, with a loop over Herdr subscription updates, file events and the end of standard input, and no request frames. Its open and close paths become `card open` and `card close`, and `set_card_mode` becomes `card mode`. `CardMode` loses `inherit`, and the card's target type moves to `src/runtime/card_settings.rs`. Delete `src/cli/serve.rs`, `src/cli/serve/dispatch.rs`, `src/cli/serve/hc.rs`, `src/cli/serve/control.rs`, `src/runtime/native_cards/control.rs`, `LaunchedCardContext.control_socket` and `src/child_protocol*`. |
| `native` | Delete `native observe`, `open`, `project`, `check`, `focus` and `close`, with `src/cli/native_tests.rs`. Rename `native pane` to `card pane`, and change `integrations/herdr-work-cards/bin/open-rust-streams-pane.sh` in the same commit. When a person quits the card, `card pane` runs `card mode HARNESS/ID manual`. |
| Other commands | Delete `session lifecycle`, the title-default commands with `set_title_default` and `TitleTarget`, `agent capture-inputs` and `agent preview` with `src/agent/capture.rs` and `src/agent/preview.rs`, the `reference` commands with `src/cli/references.rs`, and the `checkout` commands with `src/cli/checkouts.rs`. `tests/agent_capture.rs` and `tests/agent_preview.rs` go. |
| Temper | Delete the check of the old report admission: `specs/report_admission.ioa.toml`, `tests/verification.rs`, its `[[test]]` target, the `temper-jit` and `temper-verify` dev-dependencies with their `profile.test.package` and `Cargo.lock` entries, and `just test-verify`. |
| Tests | `tests/serve.rs`, `tests/serve/cards.rs` and `tests/serve/subscriptions.rs` move to `tests/card_watch.rs` and `tests/card_watch/`. Cases that sent a card request run `card open`, `card close` or `card mode` instead. `tests/serve/hc.rs` and `tests/serve/artifacts.rs` go. The fake hc core moves from `tests/integration_hc.rs` to `tests/support/hc.rs`. New cases are `card_open_reports_failure`, `card_close_keeps_watch_from_reopening`, `watch_closes_card_when_stdin_ends` and `save_writes_streams_saved_on_hc_terminal`. |

### Phase 2: format 4 and the commands

This phase holds the record, edit and document parts of `docs/session-references.md`, with the edit operation built once, and every new command.

| Where | Change |
|---|---|
| `src/model.rs`, `src/codec.rs` and `src/file_store/records.rs` | Move to format 4, as `docs/session-references.md` describes. A Session's key becomes `HOST/HARNESS/ID`. Each Session has its own directory, `hosts/HOST/sessions/HARNESS/ID/`, holding `session.json` and `artifacts/NAME`, and `Session::id_for` and `harness_session_id` go. The host ID, the harness and the Stream ID follow the same safe-token rule as the Session ID. Sessions gain `references` and `directory`, the three Session field pairs become `{text, short}`, `title` and its title mode become `{text, manual}`, the goal and its goal mode become `{text, short, manual}`, and `Stream.references`, `Stream.repositories`, `Session.checkout` and `membership_epoch` go. The Stream's `owner: {host_id}` becomes `host_id`, and every field or member with no value is `null`. A reference's `state` becomes `status`, and each entry gains `manual`. Artifacts move under their Session, `ArtifactKey` loses its Stream, and `artifacts::groups` goes. `src/storage/records.rs`, where `RecordAddress` gains the harness, `src/file_store/records/inventory.rs`, `src/file_store/records/artifacts.rs`, `src/artifacts.rs` and `src/application/artifacts.rs` follow the new layout. |
| `src/operations/edits.rs` | Apply the edit rules to one record: the exact `before` keys, the merge onto `before`, complete values without `before`, reference keys and dropped entries. Remove `TitleMode`, its `resolve` and the goal's mode, and the check for a default that changed between capture and apply. `src/operations/references.rs`, `src/operations/membership.rs`, `MoveIntent`, `apply_move` and `move_session_observed` go. |
| `src/application/fields.rs` | Add. It lists each entity's editable, read-only stored and computed fields, parses `--fields`, computes the computed fields a read needs, and renders a record as a flat object of fields. |
| `src/application/edit.rs` | Add. It parses an edit body and refuses unknown, read-only and computed fields and another host's record. Inside the `RecordStore::save` callback, on the current record, it compares `before`, merges the changes with `src/operations/edits.rs`, checks a `stream_id` destination and builds the result, so the edit is one save. |
| `src/application/documents.rs` | Add `documents::locate`, and make inspecting a clone one function. Remove `documents::context`, `catalog.checkouts`, `document_locations`, `RepositoryDocument` and the repository form of `ResourceKey`. `src/runtime/locations.rs`, `src/runtime/checkouts.rs`, `src/runtime/checkouts/state.rs` and `src/runtime/revisions.rs` go, with `tests/runtime_registry.rs` and `tests/runtime_revisions.rs`. |
| Code the model replaces | Delete the rest of `src/agent/`, `src/application/record_update.rs`, `src/application/enrollment*` and `src/cli/enrollment*`. `src/application/create.rs` creates a Stream or a Session for the new `create` commands, and new Streams get random IDs. `src/application/lifecycle.rs` keeps the age decay for `catalog tidy`, and its close cascade goes. |
| Classic Herdr path | `src/cli/cards.rs`, `src/cli/cards/source.rs`, `src/cli/cards/host.rs`, `src/cli/cards/watch*`, `src/runtime/native_cards/projection.rs`, `src/runtime/native_cards/documents.rs` and `src/runtime/native_cards/launch_context.rs` stop using `RecordContext`. They read the Session by its key, resolve documents with `documents::locate`, and pass `config::source_id` and the host. Moving a Session with its pane calls `application::edit` with `stream_id` in `changes` and the old `stream_id` in `before`. The card's re-read before acting goes. `src/runtime/card_settings.rs` is keyed by `HARNESS/ID`, and its files reset. `src/tui/` and `src/tui/fixtures/*.json` move to the format 4 fields. |
| `src/cli/args.rs` | Replace the root commands and `session` with the `stream`, `session`, `artifact`, `config` and `catalog` namespaces, the root aliases `list` and `resume`, and the flag bundles above. |
| `src/cli/select.rs` | Add the record selector for full keys, partial keys and unique prefixes. `current_stream_id` in `src/cli/mod.rs` and its reads of hc labels and the classic Herdr workspace marker go. |
| `src/cli/fields.rs` | Build an edit body from field flags, including `{text, short}` units, the mode flags, `--directory` and the reference flags with their path conversion. `stream edit` and `session edit` both call `application::edit`. |
| `src/cli/workspaces.rs`, `src/cli/sessions.rs` and `src/cli/documents.rs` | Rewrite for `stream list\|show\|create\|edit` and `session list\|show\|create\|edit\|resume`, and add `session document read\|view` using `documents::locate`, with text and `--json`. `session resume` runs the harness through `SessionCommand` in `src/runtime/launch.rs`. `SessionCommand::verified` passes the ID and no longer resolves or searches for a transcript file, so that code in `src/runtime/launch.rs` and `src/runtime/conversations.rs` goes. |
| `src/cli/artifacts.rs` | Take `--session` and `--stream`, and the list flags. `artifact list --stream` reads the Stream's member Sessions. |
| `src/cli/settings.rs` | Serve `config get\|set\|unset\|path`, with `--overrides` conflicting with `KEY`. `naming.*` is a plain setting. |
| `src/cli/catalog.rs` | List Streams by activity with the computed fields, `--host`, `--local` and `--limit`, and serve `catalog init\|sync\|check\|tidy`. Remove `cascade_stream_close_if_transitioned`. |
| `src/cli/mod.rs` | Exit with 0, 1, 2 or 3 as the output table says, moving each exit-2 site that reports a change not written to 3. |
| Tests | The codec, edit, `documents::locate`, path conversion, clone inspection and Session layout cases in `docs/session-references.md`. The edit cases run in `src/application/edit/tests.rs` with a fake store that changes records between read and save, and add `matching_before_writes`, `body_sent_twice_writes_once`, `read_only_and_computed_fields_are_refused`, `other_host_is_refused`, `changes_merge_onto_before`, `incomplete_value_without_before_is_refused`, `title_marked_manual_during_a_report_is_not_written` and `stream_id_edit_needs_an_existing_same_host_stream`. `src/application/fields/tests.rs` has `every_stored_field_has_a_class`, which serializes a default `Stream`, `Session` and `ArtifactEntry` and checks each key against the three lists. `tests/cli.rs`, `tests/cli_revisions.rs`, `tests/record_updates.rs`, `tests/application_records.rs`, `tests/fixtures/`, `src/file_store/records/tests.rs`, `src/file_store/records/inventory/tests.rs` and `src/sync/records/tests*` move to format 4 and the new commands. They add `stream_edit_by_prefix_saves_both_fields` (CUJ 2), `partial_key_resolves_to_one_session`, `ambiguous_prefix_lists_full_keys`, `session_create_refuses_a_bare_id_or_prefix`, `session_create_twice_for_one_stream_changes_nothing`, `session_create_for_another_stream_is_refused`, `add_document_turns_an_absolute_path_into_a_key`, `list_json_has_items_unreadable_and_more`, `exit_status_three_cases`, `session_create_saves_the_directory` and `resume_runs_the_harness_with_the_id`, which checks the arguments a fake launcher receives. |

### Phase 3: the callers

| Where | Change |
|---|---|
| `extensions/stream-checkpoint/` | `streams.ts`, `report-workflow.ts` and `native-rust.ts` run the commands in the extension table, name the Session as `pi/ID` in every call, write `streams.session` with `hc label`, and set Pi's name from the Session title each time they read it. `child.ts` starts `card watch`, keeps its standard input open, and closes it as Pi exits. `child-protocol.ts` goes. `core.ts`, `panel.ts`, `agent.ts` and `report-workflow.ts` move to Session references and the report rules in `docs/session-references.md`. |
| The prompt lab | `lab.ts`, `lab-cli.mjs`, `prompt-lab.ts`, `capture.ts` and `replay.ts` read records with the commands in the section on machine interfaces, and `src/cli/debug.rs` becomes `debug prompts`. |
| `integrations/herdr-work-cards/src/streams-context.ts` | Parses format 4, and drops what `docs/session-references.md` lists: the Stream's `references` and `repositories`, the reference array, the string progress fields and `Session.checkout`. `extensions/stream-checkpoint/streams.ts`, `core.ts`, `capture.ts`, `panel.ts` and `lab-artifacts.ts` keep importing it. |
| `integrations/herdr-app/state.js` and `main.js` | Read labels, reconcile `streams.stream`, run the refresh commands above, show the focused Session's references and the Stream's ID in the sidebar, open work with `hc run … streams session resume KEY`, and view documents and artifacts through `read` or `view`. |
| Extension tests | `tests/stream_*_test.mjs` use a fake `streams` program that records its arguments. `tests/stream_child_test.mjs`, `tests/stream_native_test.mjs`, `tests/stream_rust_native_test.mjs`, `tests/helpers/native-child.mjs` and `tests/helpers/session-id.mjs` move to `card watch` and the public commands, or go where they tested only the child protocol, and the Justfile recipes `test-pi`, `test-pi-child`, `test-pi-native` and `test-native-rust` follow. They gain `new_work_runs_stream_create_then_session_create`, `enroll_sends_directory`, `start_writes_or_clears_the_session_label`, `report_sends_before_for_each_change` and the Pi extension cases in `docs/session-references.md`. |
| App tests | `integrations/herdr-app/tests/state.test.mjs` gains `refresh_reads_the_focused_session`, `focus_change_in_a_tab_refreshes`, `stale_session_label_is_ignored`, `opening_reuses_a_running_session`, `opening_keeps_parked_sessions`, `sidebar_shows_the_stream_id`, `space_is_found_from_stream_labels`, `refresh_relabels_a_moved_session` and one case for each kind of reference, with a fake process runner and a fake window. |
| Card watch tests | `tests/card_watch/` adds the `card watch` cases in `docs/session-references.md`. |

`just test` passes from the end of this phase.

### Phase 4: documents and conversion

| Where | Change |
|---|---|
| `src/cli/args.rs` | Hide `card` and `debug` from `--help`. |
| Conversion | The conversion script in `.tmp/session-references/`, kept out of the repository, with the conversion cases in `docs/session-references.md`. |
| Documents | Update the README, `AGENTS.md`, `CONTRIBUTING.md`, `docs/` except `docs/hc-herdr-app-design.md`, `skills/streams/SKILL.md` and its references, and `CHANGELOG.md`. The README's line on closing a Stream changes, because closing no longer closes its Sessions. |
| Written steps | Write the steps that replace code, which are reloading the app after changing `catalog.root` or a `herdr` setting, checking that a Session is not open before `session resume`, repairing an unreadable record, giving a second catalog its own hc core, and moving or recreating a worktree. |
| Labels | Document the `streams.session`, `streams.stream` and `streams.saved` labels as the contract with the app, and `"$PI_SESSION_ID"` as how an agent in Pi names its Session. |
| `AGENTS.md` | The "Modular" goal says that frontends sit outside Streams and use the CLI. |
| Repository | Search the repository for every old command name. None appears outside the changelog, this document and `docs/hc-herdr-app-design.md`. |

### Verification plan

- Every CUJ has a test with fakes. CUJ 1, 2, 3, 5 and 6 are in `tests/cli.rs`, and CUJ 4 is in `integrations/herdr-app/tests/`.
- CUJ 3's test changes a field between the agent's read and its edit, and checks the result and exit status 3.
- The classic Herdr path keeps passing `tests/tui_pty.rs`, `tests/card_watch/` and `tests/runtime_native.rs` from phase 1. `just test-herdr` and `just test-native-rust` run TypeScript and Node callers, so they pass again from phase 3.
- `just test` runs every suite, and the final check is the old-name search from phase 4.
