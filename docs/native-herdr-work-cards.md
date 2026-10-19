# ADR: Native Herdr work cards as Streams projections

Status: accepted; the Streams-backed projection and restoration support are implemented and reviewed in this checkout. Native acceptance and activation remain pending. The imported implementation retains its original history. This decision replaces its separate catalog and membership model, not its presentation design.

## Context and decision

Keep the native Herdr Work Cards UI alongside Pi's Alt+P card. The user values the work already invested in its layout, navigation, independent agent views and naming protections. Import the existing package and history rather than reconstructing the display from its appearance.

Streams is the only writable work catalog. The native card reads a stream's shared purpose and references together with its owning session's contribution. It does not create another account of the effort or run a second summarizer. The [catalog ADR](durable-work-catalog.md) owns record meaning; the [Pi ADR](pi-work-card.md) owns reporting, editing and initial conversation enrollment.

The imported [orientation](../integrations/herdr-work-cards/docs/design.md), [workspace and display](../integrations/herdr-work-cards/docs/workspace-context.md) and [naming](../integrations/herdr-work-cards/docs/automatic-naming.md) decisions retain the reasoning behind the UI. Their legacy storage APIs, commands and membership transitions are not interfaces to the Streams catalog.

## Preserve the card, replace its authority

Retain a card beside each owning agent, not a new workspace-overview tab. Its shared section comes from the Stream; its title, status, current state and remaining scope come from the exact Session. Finishing that contribution must not imply finishing the effort. The field mapping is recorded in the catalog ADR.

Preserve the existing renderer's density, Unicode handling, clipping, quiet section grouping, responsive document grid and visible-shortcut rules. Keep terminal-link gestures, exact-target document navigation, editor reuse and independent manual names. The port must carry their regression coverage forward rather than treating a similar screenshot as equivalence.

The stream ID, catalog source, host and native conversation identity establish the view's scope. A native workspace association identifies an effort, not an arbitrary session within it. Resolve the owning conversation exactly; do not select a contribution by title, cwd, GUI focus or the number of sessions found. A missing or contradictory association is unavailable state, not permission to enroll or guess.

Catalog reads and validated operations remain behind the Streams application boundary. TypeScript does not write catalog YAML, mirror records into the old Work Cards store or infer ownership from matching record text. Cached display state never grants mutation or native-effect authority. Before each native effect, the backend attests the exact owner's pane and bounded saved Pi session header against a ready Stream workspace association. Reading or opening a card does not start reporting, attach a conversation or launch a workspace.

## Keep native effects host-local

Pane ownership, display attempts, terminal identities, naming provenance and editor bindings stay on the execution host. They are not portable catalog fields. Reuse the existing safety policies, but adapt their proofs to Streams identities: an old Work Cards store binding is not proof of a current Streams association.

Closing with `q` remains display-only. It must not stop catalog reporting, delete records, detach the conversation or cause background respawn. Persist uncertain launch attempts before effects; a timeout or missing display after an attempt must not authorize a duplicate launch. Cleanup may affect only positively verified owned resources.

Native workspace names and card captions project `stream.title`; Pi and eligible agent-tab names project `session.title`; editor names come from exact document identity. Compact purpose and progress use `goal_short` and `state_short`, not another inference or text truncation. Preserve the imported naming policy's independent switches, manual protection and actual-first-pane rule. Manual catalog text and manual native labels are separate decisions.

Native effects are not a transaction with catalog persistence. Verify current scope and acknowledgements, report partial effects and preserve recovery evidence. A successfully saved title is not proof that a native name changed.

A GUI-created terminal inherits the frontend server's environment, not the invoking CLI's. Launches explicitly carry the caller's catalog, host-state, checkout-registry and harness-selection paths, including effective defaults. The card subprocess separately captures the owning agent's bounded read environment. Neither boundary forwards the caller's full environment or credentials; otherwise a workspace or card could silently read another catalog despite carrying the same Stream ID.

## Documents open in the viewing session

Open a document in the viewing agent/tab's checkout, at that checkout's current state. Here A and B are separate tabs and jj workspaces inside one Herdr workspace, not separate Herdr workspaces. The shared reference supplies the repository and relative path; the viewing agent supplies the local checkout. The recorded ref supports recovery elsewhere, not revision-pinned navigation.

For example, agent A adds a document at `a1` and advances to `a5`. Agent B's jj workspace is at `b5`. Opening the reference from B's card opens B's working file, including uncommitted edits. It must not open A's copy, retrieve `a1` or switch branches.

Streams owns checkout selection and document resolution; the pane receives resolved locations, not a policy to reimplement. Each host-local checkout has a stable ID and an exact canonical root. Explicit repository-URL bindings distinguish intentional mappings from remotes or similarly named directories. A move updates that ID's root rather than changing every Session selection.

Each Session selects its own checkout and relative cwd. A Herdr-workspace-wide map would conflate A and B. Its selected checkout takes precedence for documents in that repository. An explicitly registered default can supply another repository's checkout. A missing or unbound selection must not silently redirect to that default; reading a card never registers checkouts or selects defaults.

Revalidate the painted reference, viewing scope and resolved root before opening or reusing an editor. Editor reuse must match the viewing Session and canonical file, not merely its repository-relative path.

If the document is absent, show “Document not present in this checkout.” Missing or ambiguous checkout bindings also remain visible errors. Never substitute another workspace's checkout, create one or remove the shared reference. The old package's automatic missing-document cleanup cannot carry over: absence in B says nothing about the file's existence in A.

## Restore work without inventing launch intent

`open STREAM` restores the effort's last recorded working arrangement or focuses its verified existing workspace. `resume` remains the lower-level operation: run one saved conversation in the invoking terminal, without reconstructing a workspace. These are different user intentions, not aliases.

Capture only observed conversations matched to local catalog Sessions. Historical helper contributions are not a list of processes to launch. Restore the captured tab grouping, order and manual labels; leave existing tabs and agents untouched. The initial case has no captured arrangement: restore the sole eligible Session, or open a shell and request a choice when several qualify. Empty or failed captures must not erase useful prior observations.

Exclude done and parked Sessions by default, while allowing explicit selection. Session status reports assignment completion, not process death: a done conversation can still be running, and unfinished work can have no live process. Verify live identities before reuse or launch; inspect saved headers rather than trusting filenames. A missing or ambiguous conversation is a visible recovery problem, never permission to start a replacement.

Keep one native workspace per Stream on the execution host, across frontend endpoints. Serialize launch and enrollment decisions per Stream, and retain attempts before effects. An association on an endpoint that cannot be checked blocks a second launch rather than proving the first workspace closed. This accepts manual recovery after uncertain effects in exchange for avoiding duplicate agents.

The arrangement records agent tabs and conversations, not arbitrary shell commands, editor-tab contents or exact pane geometry. Existing editor tabs are preserved, not automatically reconstructed after closure. These bounds avoid treating observed terminal state as executable intent.

## Alternatives and consequences

Using Alt+P alone would avoid a native port, but the user chose to retain the side pane. Rebuilding only its visual appearance would discard tested behavior and design context. Keeping the old writer and synchronizing two catalogs would introduce competing memberships, summaries and manual overrides. Importing its history and adapting the authority boundary preserves the implementation without that second catalog.

This costs more than changing a record reader. Legacy movement automatically reassigned an agent to its destination workspace; Streams deliberately has no implicit conversation move or fork. The native adapter must suspend conflicting effects rather than revive that behavior. Document resolution and native ownership need explicit adapters before activation.

The import and the functional port are separate changes. Retain original commits and documents, then verify catalog mapping, navigation, close/reopen behavior, naming and partial failures independently. Legacy record and helper tests remain historical coverage; they do not prove the new adapter correct. Run isolated native acceptance before activation, with the old writer and namers quiescent. Do not obtain quiescence by restarting busy agents without direction.
