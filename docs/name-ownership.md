# Streams owns synchronized names

Streams owns the names it synchronizes into frontends. A native rename is not an independent override that Streams must detect and preserve. Users can either name the Stream or Session through Streams, or disable outgoing synchronization for that surface. Importing Pi name edits is separate; disable both directions to keep Pi and Streams names independent.

This implemented decision replaces independent native-name protection and provenance ledgers. [Native work cards](native-herdr-work-cards.md) owns presentation; [record-local operations](record-local-operations.md) owns saving.

## Why change ownership

Previously, Streams and the user wrote the same frontend label. Streams remembered its last write to infer whether a later value belongs to the user. Herdr workspace and tab names used local ledgers. Card captions used Herdr tokens. Pi names used custom transcript entries paired with name-entry IDs. Missing or malformed history could suspend naming even when the saved title and native target are known.

That machinery protects two independent names. The user instead wants one saved title, chosen automatically or manually, with optional copies in frontends. Making those copies authoritative projections removes the need to infer authorship from text or history.

## Title generation and synchronization are separate

Each Stream and Session has a title mode: `automatic`, `manual` or `inherit`. `inherit` follows the global Streams title-generation default for that record type. Global defaults resolve to automatic or manual, not another inherited value. New records inherit; existing explicit automatic and manual modes retain their meaning.

Enrollment may generate an initial title once, including when the new record inherits a manual default. Later automatic proposals may change a title only when its effective mode is automatic. A manual title change saves the title and manual mode together. Changing a record back to automatic or inherit permits later proposals without immediately inventing a replacement title. These rules concern titles, not Goal text or other progress fields.

Separate switches control workspace, tab and agent-session name synchronization. Both automatic and manual saved titles project when the corresponding switch is enabled. Manual means the saved title cannot be regenerated; it does not mean its frontend copies stop updating.

The Naming settings submenu keeps generation defaults, incoming Pi name edits and outgoing synchronization separate. Stream and Session defaults are Rust configuration values, not duplicate Pi reporting settings. Changing a default affects inherited titles on later reports; it does not rewrite record modes, run inference or rename a frontend.

The existing mapping remains:

| Surface | Saved source | Eligibility |
| --- | --- | --- |
| Workspace name | Stream title | Observed workspace associated with the Stream |
| Agent tab name | Session title | Agent owns the actual first layout pane |
| Pi session name | Session title | Current Pi session |
| Card caption | `Stream title: Session title` | Observed Session card |
| Editor tab name | Exact document identity | Observed editor owns the actual first layout pane |

The card pane displays `Stream title: Session title`, so each contribution remains identifiable when several cards share a workspace. This replaces the earlier Stream-title-only caption; workspace names still use only the Stream title. A static launch title such as `Streams work card` must not mask that projection. The native projection executor owns this pane-name update; Pi and the renderer do not add separate writers. Tab synchronization continues to cover editor tabs. Disabling an outgoing switch leaves the current frontend name unchanged. Pi imports remain separately controlled. Re-enabling it resumes projection without reclaiming or consulting old provenance.

## Rename through the saved record

`/streams name [session|stream] <name>` saves the selected record's title and manual mode before requesting projection. The omitted target means the current Session. Mode controls also let users return a record to automatic or inherit without renaming it.

The `/streams name` command must not rename Pi first and then attempt a catalog update. The saved record is authoritative. A failed catalog change must produce no frontend rename. A failed projection must leave the accepted title saved and report the projection failure without implying rollback. A later projection can apply the current saved value again.

### Native Pi `/name`

Pi 1.4.0 emits `session_info_changed` after `/name` and other calls to its session-name setter. The separate Pi name edits setting imports these names and defaults to enabled, independently of outgoing Pi synchronization. In a bound conversation, a nonempty name is saved with `title_mode: manual` only when it differs from the current saved Session title. Other enabled projections then follow that title.

Comparing the two values ignores ordinary echoes of Streams' `setSessionName` calls. There is no projection-origin flag, scoped async context, name history or transcript provenance shared with the event handler. Streams does not intercept or replace Pi's built-in command.

This simple comparison has two accepted costs:

- Repeating the saved name through `/name` does not pin an automatic or inherited title to manual. Use `/streams name session <name>` to pin it, even when the text is unchanged.
- A rare late projection echo may differ from a newer saved title and be imported as a manual name. The handler does not attempt to reconstruct its origin.

For an unbound conversation, `/name` remains native-only: it makes no catalog write, warning or enrollment attempt. Empty names are ignored. A failed catalog save is visible after Pi's own rename; Streams does not claim a save or project a replacement itself.

| Outgoing Pi synchronization | Pi name edits | Effect |
| --- | --- | --- |
| Enabled | Enabled | Changed Pi names become saved manual titles; ordinary projection mirrors saved titles. |
| Enabled | Disabled | Pi names are not imported; a later projection can restore the saved title. |
| Disabled | Enabled | Changed Pi names update saved titles; Streams never renames Pi. |
| Disabled | Disabled | Pi and Streams names are independent. |

When synchronization is enabled, a direct Herdr or cmux rename may be overwritten on the next projection. Streams does not import those edits or preserve separate overrides. Pi name changes that cannot be observed through the supported event have the same behavior. Same-value assignments through `/streams name` still set manual mode.

## Project current values without naming history

Projection compares the desired saved title with the observed frontend value and sets it only when different. It does not read or write last-applied names, owner ledgers, manual-caption tokens or Pi naming-provenance entries. Existing derived naming history is ignored; removing it from live storage is not required for correctness.

Observed Session identity, workspace association and actual first-pane position select native targets under the [observe-and-act design](native-observe-and-act.md), without durable native proof. Label ownership does not grant permission to rename a different workspace, tab, pane or conversation. Several Sessions sharing a tab do not gain competing naming rights: the existing first-pane rule selects the source. Current frontend support remains the limit; this decision does not add missing native lifecycle support to an adapter.

Projection failures remain visible. No action-history cache or automatic retry protocol is needed to remember which names Streams owns. Re-reading current records and observations determines the next projection.

Inherited automatic title proposals capture their resolved default. On a new apply, a changed default makes an old title proposal stale. Explicit record modes and progress-only edits do not depend on the inherited default. An already accepted operation still returns its retained outcome on retry; current settings cannot revoke its recorded acceptance.

## Frontend assessment

Herdr 0.9.5, protocol 22, exposes one label for each workspace and tab. Workspace metadata supports tokens but not a generated name. Panes have separate labels and reported titles, but metadata titles take precedence over manual labels. Neither `display_agent` nor `state_labels` provides a manual-first resource name. Those fields therefore cannot preserve independent overrides without a frontend contract change.

Pi 1.4.0 stores one `session_info.name`. `/name`, CLI naming, session-picker renaming and `setSessionName()` all write that field. Its session setter emits `session_info_changed` with the normalized name but no origin. Entry labels are conversation bookmarks. They are not an alternative session name.

Current cmux upstream has `workspace.set_auto_title`, including optional panel targeting. It enforces frontend-owned user-versus-auto provenance and requires the AI auto-naming setting. Streams currently uses ordinary explicit workspace rename operations, not that method. This capability is not evidence that an installed cmux version supports independent generated and override slots.

No upstream change is required for the chosen ownership model: ordinary name setters are sufficient. If independent frontend overrides become a requirement again, the clean contract is a generated value and a user override, with the frontend displaying the override when present. Herdr would need generated workspace and tab fields; pane metadata would need manual-first precedence and suitable persistence. Pi would need a generated-session-name API alongside the user-name API. Moving today's provenance into another Streams cache would not provide that contract.

Sources: [Herdr socket API](https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/socket-api.mdx), [Herdr metadata precedence test](https://github.com/herdrdev/herdr/blob/v0.9.5/src/terminal/metadata.rs), [Pi session manager](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/session-manager.ts), [cmux naming contract](https://github.com/manaflow-ai/cmux/blob/main/docs/workspace-auto-naming.md), [cmux socket tests](https://github.com/manaflow-ai/cmux/blob/main/cmuxTests/SetAutoTitleSocketTests.swift). Herdr findings are pinned to 0.9.5; Pi findings were checked against installed 1.4.0; cmux findings describe upstream main, not a verified local release.
