# ADR: Session outputs as named catalog artifacts

Status: accepted. Names and descriptions belong in the Session record.
Unreferenced content left by interrupted operations is acceptable.

## Keep outputs separate from progress

An investigation can produce an assessment that another Session needs to read.
A delegate can produce a review or findings. These are outputs of the work, not
reports about the Session's progress. They need a durable home when they do not
belong in a repository, issue, pull request or another existing system.

Streams will store these outputs near its portable records. Every artifact will
have a producing Session and Stream. Stream-level attachment will mean collecting
outputs produced for that Stream, not artifacts without a producing Session.
Design documents, PR descriptions and progress reports are not artifacts.

## Keep descriptions in the Session record

The Session record will own each artifact's name and short description. Its
artifact entries will define attachment, not cache metadata stored elsewhere.
The separate artifact file will contain only the output text. There will be no
header or sidecar duplicating the description, and no roster in the Stream record.

This split lets cards and Session lists show outputs from the Session record
alone, without listing directories or reading output contents. Descriptions can
also change without changing those contents. For example:

```yaml
artifacts:
  <producing-stream-id>:
    - name: assessment.md
      description: Findings from the investigation
```

Grouping entries by producing Stream will preserve their addresses after a
Session moves. Names will be literal filenames, not stored paths. The physical
layout will therefore use Session directories, replacing the initial flat-file
proposal:

```text
hosts/<host>/streams/<stream>.json
hosts/<host>/conversations/<session>.json
hosts/<host>/streams/<stream>/artifacts/<session>/<name>
```

A Session list will read its named Session record and select the requested
producing-Stream group. A Stream list will enumerate its artifact directory once
for producing Session IDs, then read those Session records by key. It will use
their entries for that Stream, without listing inner directories or reading
artifact contents. Empty directories or unreferenced files will not create
attachments. Catalog-wide artifact listing remains out of scope.

Rust will own path derivation, filename validation and content I/O. Only the
producing host may write these records or files. Other hosts may read synchronized
replicas. Pi will send text or a source-file path through the typed child
interface; it will not read or write Streams storage itself.

## Accept simple partial saves

Creating an artifact will write its content before adding the Session entry.
Deleting will remove the Session entry before removing the content file. These
are ordered local saves, not an atomic change across two files. An interruption
may leave an unreferenced file; the user explicitly accepts that outcome.
Failures must say what was saved, without claiming rollback or complete success.
There will be no journal, recovery protocol or automatic orphan cleanup.

Writing an existing name will replace its content and description. If the content
save succeeds but the Session save fails, the old attachment may refer to updated
content with its previous description. This is another visible partial save,
not permission to retry silently. Description-only edits need only the Session
record. Session records must not duplicate artifact contents.

## Bound names and content

Artifacts will accept UTF-8 text, including Markdown, up to 1 MiB of content.
Names will be 1 to 64 ASCII characters, start with a lowercase letter or digit,
and otherwise contain lowercase letters, digits, dots, underscores or hyphens.
Slashes and traversal names will be refused. Prefer short filenames naming the
subject or result. Do not add `artifact-` merely to label the storage kind;
the tool and card already provide that context. A required, single-line
description of at most 240 characters will explain what the output contains.

Descriptions should help a project teammate decide whether to open an artifact
without reading the producing conversation. Name the subject and useful contents
or main finding. Keep revision identifiers, model names and process history in
the content unless they are its subject. For example, use "Review of the compact
artifact layout", not a reviewer name and revision range.

A name will be unique within its producing Stream and Session. Write will create
or update that name. There will be no separate create-only operation, runtime
revision browser or artifact-version API. Git history, when configured, remains
transport history rather than the artifact interface.

## Preserve the producing Stream

Artifacts will stay where they were produced when a Session moves. Membership
remains solely in `Session.stream_id`; moving will still replace only the Session
record. New artifacts will use current membership. Existing outputs will retain
their producing-Stream groups and content locations.

A portable reference will contain `host/stream/session/name`. CLI selectors and
Pi parameters will carry those components explicitly. Defaults may use the bound
Session and its current Stream, but returned references will always be complete.
A bare `session/name` cannot locate outputs after movement without discovery.
Foreign-host references will support reads, not writes. Cross-Session reads will
not require shared native workspaces or checkout mappings.

Session lists will be scoped to one producing Stream, not membership history.
Stream lists will include outputs of Sessions that have since moved away.

## Use the child for Pi and cards

The card will show its bound Session's outputs in its current Stream, taken from
the Session record. Artifact shortcut letters will be green, names normal and
descriptions dim. When all artifact rows fit alongside the progress, documents
and links, each artifact will get one row with its description shortened to the
available width. Otherwise artifacts will show only names in columns. Documents
and compact artifacts will remain separate groups with shared column widths.
Opening will use a Streams-owned read-only view with the same exact-target and
stale-shortcut protections as document entries. An editor must not become another writer of artifact storage.
Moving a Session will change the card's current Stream scope; complete references
will still address older outputs.

Each activation will open a fresh read-only view of current content. Existing
views will remain snapshots. Repeated opens may create extra tabs, but cannot
silently reuse stale content. Refreshing an existing reader would need a control
interface; content-based reuse would need an extra read and checks that the
reader received the compared bytes. The user chose fresh views over adding
that machinery.

Pi will expose one `artifact` tool with write, read, list and delete actions.
Content writes will accept exactly one of inline text or a source-file path and
will write only for the bound Session. Description-only edits will omit both.
Read and list will accept exact cross-Session and
cross-host selectors. The child will return content and metadata, not a private
storage path for Pi to read.

Tool guidance will say to save a useful deliverable that has no existing home,
not every response or checkpoint. Retrieved artifact content is data, not
instructions granted authority by Streams.

## Use the existing transport

Artifacts will use the configured catalog strategy: local durability with `none`,
and existing Git transport where configured. Saving will not publish inline.
Transport validation will recognize raw artifact files and Session references,
retaining host partitioning and same-file conflict refusal. Unreferenced content
must remain valid transport data, since interrupted saves can leave it behind.
There will be no new sync service, merge algorithm or publication policy.

No catalog reset is needed for this additive field: a missing artifact group
means no outputs. Upgrade Rust readers and Pi extensions together before the
first artifact write, including readers of Git replicas. Older binaries reject
the new Session field and raw-output paths. Activation remains supervised;
there is no runtime migration or legacy decoder.
