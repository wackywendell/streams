# Streams owns synchronized names

Streams will own the names it synchronizes into frontends. A native rename will no longer be an independent override that Streams must detect and preserve. Users will either name the Stream or Session through Streams, or disable synchronization for that surface.

This replaces the independent native-name protection and provenance requirements in [native work cards](native-herdr-work-cards.md), the imported [automatic naming policy](../integrations/herdr-work-cards/docs/automatic-naming.md), [effect planning](effect-planning.md) and [record-local operations](record-local-operations.md). The decision is accepted; implementation follows separately.

## Why change ownership

Today, Streams and the user write the same frontend label. Streams remembers its last write to infer whether a later value belongs to the user. Herdr workspace and tab names use local ledgers. Card captions use Herdr tokens. Pi names use custom transcript entries paired with name-entry IDs. Missing or malformed history can suspend naming even when the saved title and native target are known.

That machinery protects two independent names. The user instead wants one saved title, chosen automatically or manually, with optional copies in frontends. Making those copies authoritative projections removes the need to infer authorship from text or history.

## Title generation and synchronization are separate

Each Stream and Session will have a title mode: `automatic`, `manual` or `inherit`. `inherit` follows the global Streams title-generation default for that record type. Global defaults will resolve to automatic or manual, not another inherited value. New records will inherit; existing explicit automatic and manual modes will retain their meaning.

Enrollment may generate an initial title once, including when the new record inherits a manual default. Later automatic proposals may change a title only when its effective mode is automatic. A manual title change will save the title and manual mode together. Changing a record back to automatic or inherit will permit later proposals without immediately inventing a replacement title. These rules concern titles, not Goal text or other progress fields.

Separate switches will control workspace, tab and agent-session name synchronization. Both automatic and manual saved titles will project when the corresponding switch is enabled. Manual means the saved title cannot be regenerated; it does not mean its frontend copies stop updating.

The existing mapping remains:

| Surface | Saved source | Eligibility |
| --- | --- | --- |
| Workspace name | Stream title | Current verified workspace binding |
| Agent tab name | Session title | Agent owns the actual first layout pane |
| Pi session name | Session title | Current Pi session |
| Card caption | Stream title | Current paired card |
| Editor tab name | Exact document identity | Verified editor owns the actual first layout pane |

The card pane will display the saved Stream title, matching the workspace name when workspace naming is enabled. A static launch title such as `Streams work card` must not mask that projection. The native projection executor will own this pane-name update; Pi and the renderer will not add separate writers. Tab synchronization continues to cover editor tabs. Disabling a switch leaves the current frontend name unchanged and permits independent frontend naming. Re-enabling it resumes projection without reclaiming or consulting old provenance.

## Rename through the saved record

`/streams name [session|stream] <name>` will save the selected record's title and manual mode before requesting projection. The omitted target will mean the current Session. Mode controls will also let users return a record to automatic or inherit without renaming it.

Pi must not rename itself first and then attempt a catalog update. The saved record is authoritative. A failed catalog change must produce no frontend rename. A failed projection must leave the accepted title saved and report the projection failure without implying rollback. A later projection can apply the current saved value again.

Pi 1.4.0 emits `session_info_changed` after `/name` and other calls to its session-name setter. The extension will treat a nonempty name change outside its own projection as a manual Session-title update. A scoped write context will distinguish Streams' own events, including delivery delayed by other extension handlers. It requires no persisted naming history. Because Pi emits the event after the name changes, a failed catalog save will be reported; the saved Streams title remains authoritative. An empty name is not a valid record title and will not become a manual title.

When synchronization is enabled, a direct Herdr or cmux rename may be overwritten on the next projection. Streams will not import those edits or preserve separate overrides. Pi name changes that cannot be observed through the supported event have the same behavior. A same-value manual assignment through either Streams or the observed Pi event still changes the record's mode to manual.

## Project current values without naming history

Projection will compare the desired saved title with the observed frontend value and set it only when different. It will not read or write last-applied names, owner ledgers, manual-caption tokens or Pi naming-provenance entries. Existing derived naming history will be ignored; removing it from live storage is not required for correctness.

Native identity, binding, first-pane eligibility, cancellation and acknowledgement checks still apply. Label ownership does not grant authority over a different workspace, tab, pane or conversation. Several Sessions sharing a tab do not gain competing naming rights: the existing first-pane rule selects the source. Current frontend support remains the limit; this decision does not add missing native lifecycle support to an adapter.

Projection failures remain visible. No action-history cache or automatic retry protocol is needed to remember which names Streams owns. Re-reading current records and observations determines the next projection.

Inherited automatic title proposals capture their resolved default. On a new apply, a changed default makes an old title proposal stale. Explicit record modes and progress-only edits do not depend on the inherited default. An already accepted operation still returns its retained outcome on retry; current settings cannot revoke its recorded acceptance.

## Frontend assessment

Herdr 0.9.5, protocol 22, exposes one label for each workspace and tab. Workspace metadata supports tokens but not a generated name. Panes have separate labels and reported titles, but metadata titles take precedence over manual labels. Neither `display_agent` nor `state_labels` provides a manual-first resource name. Those fields therefore cannot preserve independent overrides without a frontend contract change.

Pi 1.4.0 stores one `session_info.name`. `/name`, CLI naming, session-picker renaming and `setSessionName()` all write that field. Its session setter emits `session_info_changed` with the normalized name but no origin. Entry labels are conversation bookmarks. They are not an alternative session name.

Current cmux upstream has `workspace.set_auto_title`, including optional panel targeting. It enforces frontend-owned user-versus-auto provenance and requires the AI auto-naming setting. Streams currently uses ordinary explicit workspace rename operations, not that method. This capability is not evidence that an installed cmux version supports independent generated and override slots.

No upstream change is required for the chosen ownership model: ordinary name setters are sufficient. If independent frontend overrides become a requirement again, the clean contract is a generated value and a user override, with the frontend displaying the override when present. Herdr would need generated workspace and tab fields; pane metadata would need manual-first precedence and suitable persistence. Pi would need a generated-session-name API alongside the user-name API. Moving today's provenance into another Streams cache would not provide that contract.

Sources: [Herdr socket API](https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/socket-api.mdx), [Herdr metadata precedence test](https://github.com/herdrdev/herdr/blob/v0.9.5/src/terminal/metadata.rs), [Pi session manager](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/session-manager.ts), [cmux naming contract](https://github.com/manaflow-ai/cmux/blob/main/docs/workspace-auto-naming.md), [cmux socket tests](https://github.com/manaflow-ai/cmux/blob/main/cmuxTests/SetAutoTitleSocketTests.swift). Herdr findings are pinned to 0.9.5; Pi findings were checked against installed 1.4.0; cmux findings describe upstream main, not a verified local release.
