# Herdr UI design for Streams

This document defines the user experience and component boundaries for the Streams integration in `herdr-app`, targeting the format-5 entity CLI (`docs/cli-by-entity.md`) and session-owned references (`docs/session-references.md`). The design separates live workspace navigation in the left sidebar and tab bar from durable work inspection in the right panel. It restructures the right panel into three views (`Session`, `Stream`, and `All`) matching the Streams data model, adds folding, read-only preview of unopened work, and scoped record actions, and keeps open work discoverable after the terminal harness becomes idle.

## Terms

Each term below keeps one meaning throughout this document.

- A Stream is a durable catalog record representing one shared body of work, with a title, a goal, a short goal, and a derived work status computed from its member Sessions.
- A Session is a durable catalog record representing one agent conversation belonging to a Stream, with a title, a work status, a state, a recap, remaining work, a references map, and an artifacts map.
- A Reference is a saved `Document`, `Pr`, `Issue`, or `Link` entry owned by a Session in its `references` map.
- An Artifact is an immutable, read-only file saved under a Session in its `artifacts` map, displayed alongside References in `Session`.
- A Space is a `herdr-app` workspace in the left sidebar, containing one or more Tabs.
- A Tab is one tab in the top bar of a Space, hosting one or more terminal panes.
- Enrolled describes a local terminal running a live Pi agent whose `streams.session` label names a saved Session on this host. An enrolled Tab hosts at least one enrolled terminal.
- Primary agent terminal is the focused enrolled terminal inside a Tab, or the first enrolled terminal in Tab order when no enrolled pane is focused.
- The coordinator is the in-mod state controller inside `mods/streams` that calls the public `streams` CLI, reconciles terminal labels, opens Sessions, and publishes shared state.
- Work status is the durable catalog status (`active`, `waiting`, `blocked`, `parked`, `done`, or `closed`). Sessions store `status` directly; Streams derive `status` as the highest member Session status under `active > blocked > waiting > parked > done > closed`, or `active` when a Stream has no Sessions.
- Harness status is the live turn state reported by `herdr-app` for a running agent (`working`, `blocked`, `done`, or `idle`).
- Openness is whether a Session or Stream currently has a live enrolled terminal pane in `herdr-app`. A record with no live pane is unopened; the word `closed` refers strictly to the `closed` work status.

## Purpose

A user working across several Streams needs two distinct views of their work. The left sidebar and top tab bar answer what is open right now and where to switch next. The right panel answers what each piece of work is about, what remains, what references belong to it, and what other Streams exist whether open or unopened.

Today, two gaps break that workflow. First, harness status clears when a user views a finished agent turn, so an open Session whose work status is still `active` moves into the idle group and loses visibility. Second, the right panel blends Stream and Session fields into one view, renders openness and work status as competing badges side by side, prevents enrolled Tabs from reaching the full Streams list, and provides minimal actions on references and sessions.

This design gives each surface one clear responsibility. Enrolled Tabs display a compact work status glyph, and the left sidebar adds a folded `Active idle` section for open Sessions in other Spaces that are idle in the harness while still in progress in Streams. The right panel splits into `Session`, `Stream`, and `All`, separates openness from work status visually, supports symmetric read-only inspection of unopened Streams and Sessions, and adds folding, reference dropping, terminal closing, and status updates through the public entity CLI.

## Assumptions

The design relies on the following conditions.

- Single-user desktop `herdr-app` runs locally alongside `herdr-core` (`hc`) and the format-5 `streams` CLI (`docs/cli-by-entity.md`, `docs/session-references.md`).
- `mods/streams` mounts into `herdr-app` layout slots as defined in [herdr-app mod packaging](herdr-app-mod-packaging.md). Tab bar and left sidebar chrome mods live outside `mods/streams` and stay orthogonal to Streams by reading generic terminal marks from shared state; layouts with unmodified consumers render ordinary Herdr chrome.
- The public `streams` CLI is the sole reader and writer of catalog records, and `mods/streams` uses public entity commands (`stream`, `session`, `artifact`, `config`) with no custom integration RPC layer.
- `mods/streams` is the sole writer of `terminal.marks` in `herdr-app` shared state, and other mods treat that key as read-only by convention.
- Herdr's Space mods (`spaces`, `jj-spaces`, `worktree`, and `jj-workspace`) own git worktree and jj workspace creation and deletion.

## Guarantees

Users and mod authors can observe seven guarantees from this design.

1. Single status grammar. Every Stream or Session row in the right panel shows at most one work status badge. Openness appears through icon treatment or secondary count text, so two status badges never appear side by side on one row.
2. Quiet work stays discoverable. Each enrolled Tab displays the work status glyph of its primary agent terminal, and the left sidebar's folded `Active idle` section lists every open Session in another Space whose harness status is `idle` while its work status is `active`, `waiting`, or `blocked`.
3. Consistent drill-down navigation. Clicking any Stream in `All` always displays `Stream`; clicking any Session in `Stream` always displays `Session`. When the clicked Stream or Session has a live enrolled pane, `herdr-app` focuses that pane without launching new terminals. When it is unopened, the right panel previews it in place without launching terminals.
4. Clear detached preview state. Whenever `Stream` or `Session` displays a record that differs from the active Tab's record, a top context bar identifies the preview state, names whether the record is unopened, and provides navigation back (`← All` or `← Stream`, plus `Back to current` whenever the active Tab is enrolled, and a `Resume` button when previewing an unopened Session). External Tab changes, external Space changes, or returning to the active Tab clear the preview override.
5. Explicit, scoped actions. `Drop` writes `dropped` to a Session's `Document`, `Pr`, `Issue`, or `Link` Reference via `streams session edit --drop-reference` so automatic reporting leaves it dropped; `Artifact` rows provide read-only open and copy actions only. `Close terminal` closes only the live pane from the UI without deleting or closing the Session record (normal agent shutdown hooks may mark an `active` Session `parked`). Status actions write only valid Session statuses so derived Stream status remains truthful.
6. Clear worktree boundary. Streams UI actions leave git worktrees and jj workspaces intact, keeping checkout cleanup in Herdr's Space menu.
7. No duplicate CLI calls across mods. Tab bar and left sidebar mods read Streams status from shared state published by `mods/streams` and never invoke the `streams` CLI themselves.

## Alternatives considered

Three alternatives were evaluated and rejected during design.

- Keep a single blended right-panel view. Showing Stream goals, current Session progress, References, and sibling Sessions in one scrollable panel hides full recaps during active work and mixes Session-owned References with Stream-level navigation. Splitting into `Session`, `Stream`, and `All` matches the format-5 records one to one.
- Ship forked `tabs` and `agents` mods inside the `streams` package. Forked chrome mods break composition with other layout packages such as `jj-tabs`, violating [herdr-app mod packaging](herdr-app-mod-packaging.md). Publishing generic `terminal.marks` to shared state preserves single-writer ownership while keeping any tab bar or sidebar mod orthogonal to Streams.
- Launch terminals on primary click for unopened Sessions. Making primary click on unopened Sessions in `Stream` call `session resume` prevents read-only inspection of recaps and references on finished or parked work and risks reactivating parked Sessions accidentally. Opening an unopened Session in read-only `Session` preview on primary click, paired with a hover `▶` Resume button and a `Resume` button in the preview header, keeps both inspection and resuming one click away.

## Implementation

### Pieces

The design spans four pieces across `herdr-app` mods and the coordinator in `integrations/herdr-app/state.js`, requiring no Rust CLI changes beyond the format-5 entity CLI.

#### 1. Host-wide observation and shared publication

`mods/streams` stays mounted in `pane.header.content` (`role = "frame"`, `reveal = "always"` in `integrations/herdr-app/layouts/streams.json`), so the coordinator observes local terminals continuously even while the right panel is hidden. On each observation cycle, `runningSessionTerminals` identifies every local live enrolled terminal across all Spaces from `streams.session` labels and live Pi agent state. The coordinator extends its observation key and refresh pass to keep all three datasets fresh regardless of whether the active Tab is enrolled.

- Active or previewed record details via `streams session show KEY --json` and `streams stream show KEY --json`.
- Off-tab live enrolled Sessions (any terminal in `runningSessionTerminals` whose Session is not already in the active Stream's `sessions`) in one batched `streams session list --key KEY... --all --fields key,stream_id,title,status,state --json` call.
- Local host Streams via `streams stream list --local --all --json --fields key,title,goal,status,active_sessions,total_sessions,last_activity_ms`, fetched on refresh notices and when viewing `All`.

Each cycle prunes exited or unenrolled terminals from `coordinator.frames` against `runningSessionTerminals`. After refreshing, the coordinator translates each enrolled terminal's work status into a generic mark and writes `terminal.marks` to `herdr-app` shared state, keyed by local terminal address.

```json
{
  "term-abc": {
    "symbol": "●",
    "color": "ansi2",
    "attention": true,
    "label": "Active",
    "source": "streams"
  }
}
```

Participating tab bar mods read `shared("terminal.marks")` for each Tab's primary agent terminal and draw its `{ symbol, color }` glyph beside the Tab label without referencing Streams concepts. Participating left sidebar agent views (including Focused mode) read the same map to draw glyphs on idle rows and collect open terminals in other Spaces whose harness status is `idle` while `attention` is `true`. Those entries appear inside a dedicated `Active idle` section that starts folded and displays its count in the header; clicking an entry focuses its pane.

#### 2. Right panel navigation and views

The right panel header holds a three-way segmented switcher (`Session`, `Stream`, `All`) alongside the Refresh control. Each view maps directly to one record level. The active view tab (`Session`, `Stream`, or `All`) is stored separately from the detached preview override (`previewStreamKey` and optional `previewSessionKey`), so programmatic focus into `Stream` or `Session` stays on the chosen view tab while external focus changes clear any detached preview.

- `Session` displays the active Tab's enrolled Session, or an unopened Session opened for preview from `Stream` via `streams session show KEY --json`. When displaying a previewed unopened Session, a top context bar identifies it as unopened and provides `← Stream`, a `Resume` button, and `Back to current` (whenever the active Tab is enrolled). A compact Stream breadcrumb (`≋ Stream title`) sits at the top and switches to `Stream` on click. Below it appear the Session title, its clickable work status badge (opening the status menu directly), a `⋮` menu, and the full `state`, `recap`, and `remaining` fields. A collapsible `References` section follows, starting expanded when populated and holding this Session's non-dropped `references` and its `artifacts`. `Session` omits sibling session lists because sibling Sessions belong to the Stream record. On an unenrolled Tab with no previewed Session, the `Session` switcher segment appears muted, and selecting it displays a short empty state explaining that the active Tab is unenrolled.
- `Stream` displays the active Tab's Stream or an unopened Stream previewed from `All` via `streams stream show KEY --json`. When the displayed Stream differs from the active Tab's Stream, a top context bar identifies the detached preview and provides `← All` on the left and `Back to current` on the right (or only `← All` when the active Tab is unenrolled). The body shows the Stream title, its derived work status badge, a `⋮` menu, its full `goal`, and a collapsible `Sessions` roster starting expanded with a `↕ Recent` / `↕ A–Z` sort toggle (defaulting to recency). Within the roster, Sessions are partitioned by openness first and then archive into three foldable sections (`Open`, default open, containing every Session with a live terminal regardless of status so finished open tabs remain visible; `Not open`, default open, containing unopened `active`, `waiting`, `blocked`, and `parked` Sessions; and `Done & closed`, default closed, automatically expanded if it is the only non-empty section). Open Sessions display their full-opacity harness icon and reveal a hover `×` Close terminal button beside `⋮`; unopened Sessions appear muted with an outline marker and reveal a hover `▶` Resume button beside `⋮`. Clicking any Session row opens `Session` (focusing its live pane if open, or opening the read-only preview with the unopened header if unopened).
- `All` displays every local Stream on the host from any Tab. Its toolbar provides text search and a `↕ Recent` / `↕ A–Z` sort toggle (defaulting to recency). Streams are partitioned into the same three foldable sections as `Stream` (`Open`, default open, for any Stream with at least one open enrolled terminal; `Not open`, default open, for unopened `active`, `waiting`, `blocked`, and `parked` Streams; and `Done & closed`, default closed, automatically expanded while a search query is active or when it is the only non-empty section). Each row shows `≋`, the Stream title, one derived work status badge, and relative age on the top line. The second line shows `active_sessions`, openness derived from live enrolled terminals (`M open` or `none open`), and the Stream's short goal, such as `2 active · 1 open · short goal` or `2 active · none open · short goal`. Clicking a row switches the panel to `Stream`; if the Stream already has an open enrolled pane, the coordinator focuses that pane via `pane.focus`, preferring the target Space's active Tab when enrolled in that Stream and otherwise selecting the first live enrolled pane in layout order.

In the pane header (`frame` role), the enrolled Session chip (`· ● <Session title>`) appears only on multi-pane split tabs where the tab bar cannot label every pane. Single-pane tabs leave the Session name and status to the tab bar so the pane header displays only the program icon, program name, working directory, and pane controls.

Section fold state (`References` in `Session`, `Sessions` in `Stream`) persists locally in the view instance across Tab switches. Any external Tab or Space focus change clears the preview override so the right panel tracks the newly focused Tab.

#### 3. Right panel interactions and menus

Row menus use Herdr's native popover overlay (`⋮` button and right-click on rows) so secondary actions stay out of the way during scanning. Actions are segmented by target kind to match format-5 record rules.

| Surface | Primary click | Hover / menu actions |
| --- | --- | --- |
| `Document` reference row (`Session`) | Open in configured viewer (`session document read` or `view`) | Open in editor (`session document view`), Copy path, Drop |
| `Pr`, `Issue`, or `Link` reference row (`Session`) | Open in browser | Copy URL, Drop |
| `Artifact` row (`Session`) | Open in read-only viewer (`artifact read` or `view`) | Copy artifact key |
| Open `Session` row (`Stream`) | Focus live pane and open `Session` | Hover `×` button and menu `Close terminal`, Switch to, Set status (`Active`, `Waiting`, `Blocked`, `Parked`, `Done`, `Closed`) |
| Unopened `Session` row (`Stream`) | Preview read-only in `Session` | Hover `▶` button and menu `Resume`, Inspect, Set status (`Active`, `Waiting`, `Blocked`, `Parked`, `Done`, `Closed`) |
| `Session` header (`Session`) | Status badge opens status menu | `Resume` button (in unopened preview bar), Set status (`Active`, `Waiting`, `Blocked`, `Parked`, `Done`, `Closed`) |
| `Stream` header (`Stream`) | None | Open all active sessions, Park all, Close all, Delete empty stream |

Action execution uses existing format-5 CLI commands directly from the coordinator.

- `Drop` runs `streams session edit SESSION_KEY --drop-reference REF_KEY` on `Document`, `Pr`, `Issue`, and `Link` References. `Artifact` rows omit `Drop` and `Open in editor` because artifacts are immutable outputs with read-only viewing and permanent `artifact delete` only.
- `Set status` on a Session runs `streams session edit SESSION_KEY --status STATUS` for any of the six statuses (`active`, `waiting`, `blocked`, `parked`, `done`, `closed`), greying out the Session's current status.
- `Stream` header bulk actions reflect that format-5 Stream status is purely derived from member Sessions, preventing invalid Stream-level overrides. `Open all active sessions` invokes `openWork` with explicit `sessions` filtering to `status === "active"` only (avoiding launching `parked` Sessions) and is greyed out when no `active` Sessions exist. `Park all` is enabled when any member Session is `active`, `waiting`, or `blocked` (greyed out otherwise) and runs `streams session edit KEY --status parked` on each of those Sessions so the Stream derives `parked`. `Close all` is enabled when any member Session is neither `done` nor `closed` (greyed out otherwise) and runs `streams session edit KEY --status closed` on each open member Session. `Delete empty stream` is enabled only when the Stream has zero member Sessions and runs `streams stream delete KEY --json`.
- `Resume` invokes `openWork` for that single Session (`hc run ... streams session resume KEY`).
- `Close terminal` calls `pane.close` for that Session's live pane.

Popup overlays stay pure pickers with no async work of their own. Clicking a menu row closes the overlay and enqueues the action for the sidebar via shared state (`streams.actionSeq`), where the sidebar runs it on its own GPUI task context. After any catalog write succeeds (or partially succeeds on a bulk action), the coordinator runs a targeted authoritative refresh (`streams session show` for the touched Session when applicable and `streams stream show` for its parent Stream), projects those fresh records across `frames`, `previewSessions`, `streams`, and `hostStreams`, republishes `terminal.marks`, and bumps `streams.viewRev` so every view redraws immediately without scanning the whole host. The coordinator tracks background `readError` and user-triggered `actionError` separately so routine observation refreshes never erase an unread action error. CLI errors are parsed from structured JSON on both `stdout` and `stderr`; when `Resume` fails because a Session's saved folder no longer exists, the banner explains the missing path and repository URL and offers a `Copy command` button with the ready-to-edit `streams session edit --directory` command. Every failed action pins an error banner directly beneath the panel header (above the scroll region) and sends a native Herdr error toast via `notify()`.

#### 4. Coordinator changes on format-5 `state.js`

Four changes in `integrations/herdr-app/state.js` and `integrations/herdr-app/main.js` implement this design on top of the format-5 entity CLI without adding Rust commands or RPC envelopes.

1. Host-wide observation and pruning. Extend `observe` and `refresh` to batch off-tab live enrolled Sessions (`session list --key ...`), keep `hostStreams` fresh from enrolled Tabs (`stream list --local --all`), prune exited terminals from `frames`, and publish generic `terminal.marks` to `herdr-app` shared state.
2. Separate view tab and detached preview state. Store `activeView` (`session`, `stream`, or `all`) independently from `previewStreamKey` / `previewSessionKey`, fetch `session show KEY --json` when previewing an unopened Session, and clear preview overrides on external Tab or Space changes.
3. Focus-only navigation for open records. Clicking an open Stream in `All` or an open Session in `Stream` focuses an observed live enrolled pane via `pane.focus` while landing on `Stream` or `Session` respectively, without running `openWork`.
4. Entity CLI mutations and persistent error surfacing. Wire `dropReference` (`session edit --drop-reference`), `setSessionStatus` (`session edit --status`), `parkAllSessions`, `closeAllSessions`, and active-only `openAllActiveSessions` directly onto `this.exec(this.streamsBin, ...)`, reporting failures through `actionError` (pinned banner with `Copy` and `×` controls) and Herdr's native `notify()` toast.

### Architecture diagram

```text
+-------------------------------------------------------------------------+
| herdr-app window                                                        |
|                                                                         |
| +--------------------+ +----------------------------------------------+ |
| | Left sidebar       | | Tab bar                                      | |
| |                    | | [ Tab A * ] [ Tab B v ] [ Tab C o ]          | |
| | Spaces             | |   (reads shared terminal.marks)              | |
| |  - space-1         | +----------------------------------------------+ |
| |  - space-2         | | Main terminal pane    | Right panel          | |
| |                    | |                       | [Session|Stream|All] | |
| | Agents / Focused   | |                       |                      | |
| |  - Needs you       | |                       | Session              | |
| |  - Working         | |                       |  Stream link         | |
| |  > Active idle (2) | |                       |  State/Recap/Rem     | |
| |    (reads shared)  | |                       |  v References [...]  | |
| +--------------------+ +-----------------------+----------------------+ |
+--------------------------------------------------------------|----------+
                                                               |
                                    writes shared              | public entity
                                    terminal.marks             | CLI commands
                                           ^                   v
                                    +--------------+    +--------------+
                                    | mods/streams |<-->| streams CLI  |
                                    | coordinator  |    | (format 5)   |
                                    +--------------+    +--------------+
```

In this layout, `mods/streams` owns all calls to the format-5 `streams` CLI, renders the three-view right panel, and publishes generic terminal marks into shared state for participating tab bar and left sidebar mods to read.

### Walkthrough

Suppose a user is working on two Streams, `Billing API` in Space 1 and `Herdr UI` in Space 2.

1. In Space 1, an agent on `Billing API` finishes a design phase and stops. The harness marks it `done`. The user switches to Space 1, reads the reply, and switches back to Space 2 to answer a blocked agent on `Herdr UI`. Viewing Space 1 cleared the harness `done` badge, so `Billing API` is now `idle` in Herdr while its Session work status remains `active` in Streams.
2. Twenty minutes later, the user finishes the turn in Space 2 and checks what else is open. In the left sidebar's Focused view, the folded `Active idle (1)` section shows `Billing API · Design phase`. Clicking it focuses Space 1.
3. In Space 1's tab bar, the `Design phase` Tab displays the `active` glyph while a finished sibling Tab displays `✓`.
4. The right panel follows the active Tab and opens on `Session`. The user reads the full recap, remaining work, and the already-expanded `References` list, clicks a PR to open it, and opens the `⋮` menu on an outdated scratch note to click `Drop`. The coordinator runs `streams session edit KEY --drop-reference REF_KEY`, the catalog saves the drop, and the reference disappears from the list.
5. Next, the user clicks `All` in the right panel to check unopened work. The list shows `Docs cleanup` with `● waiting` on the right and `1 active · none open` on the subtitle line. Clicking `Docs cleanup` switches the right panel to `Stream` for `Docs cleanup` without launching any terminals, showing the top context bar `← All` and `Back to current (Billing API)`.
6. Clicking the unopened Session row inside `Docs cleanup` opens a read-only preview in `Session` showing an unopened preview header with `← Stream`, `Resume`, and `Back to current`, along with the Session's recap and references (without launching a terminal). After confirming no action is needed yet, the user clicks `Back to current`, returning the panel to `Billing API`.

## Failures

Each failure case names the guarantee it tests and explains how the system behaves.

- Unenrolled active Tab (tests Guarantees 3 and 4). When the active Tab has no enrolled Session, the right panel defaults to `All` and mutes the `Session` switcher segment. Clicking an unopened Stream opens `Stream` with only `← All` in its top bar, and clicking one of its Sessions opens a read-only `Session` preview with `← Stream` and `Resume` (no `Back to current` button because no current Session exists).
- Write or resume action fails or times out (tests Guarantee 5). If `Resume`, `Drop`, `Set status`, `Park all`, `Close all`, or opening a reference returns a non-zero exit code from `streams` or `hc`, the coordinator keeps the cached view unchanged, displays a pinned error banner beneath the panel header with `Copy` and `×` controls, and posts an error toast via Herdr's `notify()`. Background refreshes update only `readError` and leave `actionError` visible until dismissed or replaced by a later action. When `Resume` fails because a Session's saved worktree directory was deleted, the error names the Session, missing path, and saved repository URL without switching Spaces. For bulk Stream actions (`Park all`, `Close all`), any Sessions that succeeded before an error remain saved, the banner names the failing Session, and a fresh refresh displays the exact saved state.
- `streams` CLI unavailable at startup (tests Guarantee 7). `mods/streams` leaves `terminal.marks` empty and shows the error message inside the right panel. Participating tab bar and left sidebar mods observe an empty map and draw ordinary Herdr chrome without spawning subprocesses or showing broken status glyphs.
- External Tab or Space switch during detached preview (tests Guarantee 4). If the user previews unopened Stream B or one of its Sessions while in Space A and then switches to Space C via a shortcut or the left sidebar, the observation pass detects the active Tab or Space change and clears the preview override. The right panel follows Space C's active Tab automatically.
- Closing a terminal that already exited (tests Guarantee 5). If the user opens a Session row's menu while its terminal is exiting and clicks `Close terminal`, `pane.close` is a harmless no-op on the missing pane and the saved Session record stays intact. The next observation updates the row's openness marker to unopened.
- Stale `streams.session` label on a terminal (tests Guarantees 2 and 7). If a terminal carries a `streams.session` label whose Session no longer exists, `session list --key` omits it and `session show` returns `not-found`; the coordinator drops that terminal from `frames` and `terminal.marks` without surfacing a false badge or error banner.
