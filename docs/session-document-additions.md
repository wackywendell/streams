# ADR: Discover document links in Session reports

Superseded by [Stream references](stream-references.md) and its
[implementation contract](stream-reference-implementation.md). The historical
decision below records the former document-only, addition-only path. The shared
reference collection now supports direct document URLs, all four target kinds,
captured updates and drops, and explicit user restoration and deletion.

Document links should follow useful work without a separate request to register
each one. The old Work Cards helper collected reference additions from dialogue
and file activity. That path was removed without an equivalent in Streams
reporting. A separate, also-removed Python hook refreshed provenance on existing
links; it did not discover them.

## Use the existing Session report

The Session helper already receives the conversation. It can propose a small
reading list of project documents the conversation produced or meaningfully used,
alongside its progress report. The Stream helper sees saved contributions rather
than the original conversation, so it is not the discovery owner. No additional
model request or repository scan is needed.

Trimmed evidence retains attempted read, write and edit paths, including nested
codemode metadata. It does not copy file bodies, edit arguments or tool results.
An attempted operation is not proof that a file exists. The helper must have a
known Stream repository alias and repository-relative path; it should omit
uncertain or incidental references. A ref is optional recovery context, not part
of document identity or current-file opening. A null Session checkout is normal:
the host-local checkout bindings can identify a repository root without a
portable branch. Missing branch evidence must not suppress an otherwise useful
link, and the helper must not guess a ref or perform VCS discovery to fill one.
Temporary reports remain Session artifacts, not repository documents. Issues
and PRs are outside this restoration.

## Add links, never replace the list

Document proposals sit outside the Session's field changes. A separate automatic
save targets the captured Stream, reads that Stream and the captured Session by
key, and appends missing repository/path identities. It leaves current metadata
and concurrent additions untouched. A link already present when inference began
is not an addition, even if someone removes it before the result arrives.

The save records the source Session itself. The model cannot assign provenance,
and this path does not refresh existing refs or commit metadata. It validates
portable document identities, not file existence or VCS revisions.

Membership and repository bindings still matter. A move, including a return to
the original Stream, supersedes the additions rather than retargeting them. A
changed repository URL also supersedes proposals that depended on it. Unrelated
Session progress, including the report's own preceding save, does not invalidate
the reading list.

Each save keeps its own receipt. Successful Session progress remains saved if
the document save is refused; neither a retry loop nor a multi-record transaction
is introduced. Frozen replay uses the same pure addition and membership rules.
An explicit removal prevents an in-flight result from restoring a captured link;
it is not a permanent ban on rediscovering that path in a later report.

The [record-local operation contract](record-local-operations.md) still applies:
read the two named records, make a pure change and replace only the target Stream.
