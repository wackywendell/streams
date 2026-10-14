# Work cards design

## Scope

One ESM TypeScript package provides a Pi extension and compiled JavaScript Herdr terminal plugin. It requires Node.js 22 or later and uses `yaml` at runtime. Pi and AI host packages are peer dependencies, never bundled. Build with `npm run build`; `npm test` imports compiled `dist` with Node's built-in test runner. macOS verification is recorded in the [README](../README.md#verification-status); Linux remains an untested design target.

Cards outlive sessions and workspaces. Durable YAML lives at `$XDG_STATE_HOME/herdr/cards/<initial-title-slug>.yaml`, defaulting to `~/.local/state/herdr/cards`. There is no central registry or session catalogue. Substantive design documents stay inside the repository. No arbitrary card prose body, required timestamps, Obsidian integration, agent launch, or repository editing by the helper.

## Card schema

All seven fields are required; arrays may be empty. Unknown keys and YAML duplicate keys fail validation.

- `title`: non-empty single-line string.
- `status`: `active | waiting | blocked | parked | done`.
- `state`: short single-line summary (empty permitted).
- `cwd`: absolute POSIX path or `~/...`, expanded on the execution host.
- `documents`: `{ path: string, description: string }[]`, using repository-relative POSIX paths, no absolute paths, backslashes, empty segments, `.` or `..` segments.
- `issues` and `prs`: `{ url: string, description: string }[]`, valid absolute HTTP(S) URLs; whitespace, control characters, encoded controls and backslashes rejected.

Card identity is the filename slug, independent of later manual title edits. Automatic updates cannot change title or cwd. Descriptions are single-line strings. Unicode control/format characters are rejected in displayed text. Slugs are lowercase ASCII words separated by single hyphens, at most 120 characters. Slug generation normalizes accents, replaces punctuation with hyphens, and fails for titles with no ASCII letters/digits. Collisions fail without overwriting.

## Shared API (`src/card.ts` / `dist/card.js`)

Exported types: `CardStatus`, `CardDocument`, `CardLink`, `Card`, `CardPatch`, `CardSnapshot`.

```ts
type CardPatch = Partial<Pick<Card, 'status' | 'state' | 'documents' | 'issues' | 'prs'>>;
interface CardSnapshot { slug: string; path: string; card: Card; revision: string }
cardDirectory(env?: NodeJS.ProcessEnv): string;
slugify(title: string): string;
validateCardPatch(value: unknown): CardPatch;
createCard(card: Card, directory?: string): Promise<CardSnapshot>;
readCard(slug: string, directory?: string): Promise<CardSnapshot>;
listCards(directory?: string): Promise<CardSnapshot[]>;
updateCard(slug: string, patch: CardPatch, expectedRevision: string, directory?: string, signal?: AbortSignal): Promise<CardSnapshot>;
resolveDocument(card: Card, path: string): Promise<string>;
```

Revision is the SHA-256 hex digest of raw bytes. Reads return validated snapshots; listing returns slug-sorted snapshots, returns `[]` for an absent directory, and fails visibly on malformed cards rather than hiding them. Card files must be regular files, not symlinks.

Creation writes and syncs a same-directory exclusive temporary file, then atomically hard-links it into place without overwrite. Updates acquire `<slug>.yaml.lock` with exclusive creation, compare the revision inside the lock, validate the patch and sync a temporary file. They check the optional abort signal synchronously at the commit boundary, with no intervening await before atomic rename. An aborted prepared update removes its temporary file and releases its lock without replacing the card. Cancellation after rename is dispatched cannot roll back the write. A stale revision throws an actionable conflict error asking callers to reread. A held lock fails immediately with its path and manual-recovery guidance; locks are never stolen, retried, or timed out. Successful or failed updates release only their own lock. An unchanged semantic card returns the existing snapshot without changing bytes or mtime, including YAML formatting/comments. Locking protects cooperating package writers; arbitrary editors do not honor locks, so external edits during the locked read/rename interval cannot be guaranteed safe. Atomic visibility is not a guarantee against every filesystem or power-loss failure.

`resolveDocument` validates the path, resolves cwd and target via filesystem realpaths, requires an existing regular file, and rejects symlinks escaping cwd. It does not read or write document contents. The caller should use the returned realpath directly; an adversarial filesystem can still race a later open. cwd denotes the checkout boundary; there is no repository discovery or registry.

## Global configuration (`src/config.ts` / `dist/config.js`)

Explicit standalone loader, not Pi core settings. Path: `<PI_CODING_AGENT_DIR or ~/.pi/agent>/work-cards.json`. Missing file uses defaults; malformed JSON, unknown fields or invalid values throw with the config path. Partial configuration overlays defaults. No repository-level config.

Exports: `WorkCardsConfig`, immutable `DEFAULT_CONFIG`, `configPath(env?: NodeJS.ProcessEnv): string`, `loadConfig(path?: string): Promise<WorkCardsConfig>`. Re-exports all shared card data types for clients.

| Field | Default | Validation / tradeoff |
| --- | --- | --- |
| model | `helpers/simple` | non-empty provider/id or configured helpers alias |
| thinking_level | `null` | `null` or `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`; null inherits selected helpers/* profile thinkingLevel, otherwise off; explicit overrides tune physical models too |
| check_every_n_responses | 1 | integer >=0; zero independently disables response checks |
| check_every_n_cycles | 10 | integer >=0; zero independently disables cycle checks |
| max_context_chars | 24000 | positive integer, actual characters, not a token estimate |
| max_document_bytes | 262144 | positive integer; read-only document preview byte budget |
| max_output_tokens | 2048 | positive integer |
| request_timeout_ms | 30000 | integer 1–2147483647 (Node timer range) |
| pane_poll_interval_ms | 1000 | integer 1–2147483647 (Node timer range) |
| herdr_timeout_ms | 3000 | integer 1–2147483647 (Node timer range) |
| pane_height_fraction | 0.2 | finite number strictly between 0 and 1 |
| update_guidance | See default text below | non-empty string; preserve stable facts and avoid speculative progress |

The default `update_guidance` is:

> Update only for significant, evidenced changes. Preserve stable facts. Do not infer progress, invent links or documents, or rewrite wording without a substantive change. Return no_change when nothing important changed.

There are no automatic retries or cooldowns.

## Runtime components

The shared card module validates YAML, resolves document paths and handles revision-checked atomic writes. The configuration module loads package settings independently of Pi's core settings.

The Pi extension (`dist/pi/index.js`) handles explicit commands, restores session bindings and schedules helper checks from Pi evidence. Checks are single-flight per Pi session, not globally across separate bindings. New eligible evidence replaces pending evidence; stale results cannot update a changed card, binding or session. Helper output passes through `validateCardPatch` and `updateCard`, never direct file writes. Patches cannot change title, cwd or repository documents.

Projected host messages are normalized once into readonly role/text evidence at ingress, excluding thinking, images, tool arguments and custom messages. Queued text retains bounded newest-first excerpts. Prompt construction consumes these typed records and copies entries before clipping them to the separate exact JSON-encoded full-prompt character budget; it never shortens queued evidence.

The default `helpers/simple` model needs an existing registered helper profile. Users may configure a physical provider/model instead. Calls cost tokens even for `no_change`. Pi exposes no public API to add standalone helper usage to its usage totals. The local timeout stops waiting and requests cancellation, but cannot guarantee an upstream request stops.

The Herdr client exports `attachCard(snapshot: CardSnapshot, config)` and `detachCard(target: Pick<CardSnapshot, 'path'>, config)`. Existing snapshot callers remain valid; detach needs only the saved path and does not reread YAML. It targets the caller's workspace, verifies plugin registration and ownership, and serializes cooperating mutations with a workspace lock. It never uses GUI focus as a fallback.

The pane reads YAML independently of Pi and is the sole regular workspace-summary publisher. Tokens hold ownership and display metadata, not durable card data. Document previews are read-only and bounded by `max_document_bytes`; they never invoke a shell editor, `open` or `xdg-open`. HTTP links use OSC 8 and Herdr's local-client handling.

## Binding and owner recovery

Pi retains only an immutable slug/path target, originating session ID and update permission, not a cached card snapshot or separate directory. Reads and checks use fresh local snapshots, deriving the directory from the validated target path. Pi saves bindings in custom session entries. The host flushes these only after user or assistant activity; `--no-session` does not persist them. Restore accepts only the originating session and a valid existing YAML card. It does not reconstruct missing YAML or open a pane. Forks and cloned sessions need explicit `open`.

Detach records `updates: false` before configuration loading or Herdr cleanup. Invalid configuration or failed cleanup retains a durable paused binding for retry, subject to the host's flush caveat. Restore preserves that pause until explicit `open` succeeds. Legacy entries without the flag enable updates; malformed flags remain paused.

A pure ownership classifier distinguishes empty, live and recoverable-orphan inventories for explicit operations; attach/detach execute its decision without mutating the read inventory. Publication uses only live-owner matching against the current pane and workspace, never orphan recovery or a full inventory scan. Explicit attach or detach can recover a departed owner only under the workspace lock, after an unambiguous live inventory. The workspace must hold a complete, valid tuple for the same card: canonical card identity, nonce, pane ID and terminal ID. Both the referenced pane and terminal must be absent, with no conflicting marked panes. Recovery clears only Work Cards' `wc_*` workspace tokens. Surviving or restored owners, mismatched cards and ambiguous markers are never reclaimed. Polling and session restore never recover owners.

Current-binding detach can clean up after YAML deletion. It derives identity from the canonical existing parent directory and filename. A missing parent, dangling card symlink or ambiguous identity blocks cleanup.

In a document preview, `q` returns to the card. In the main view, it exits the display without detaching the Pi session or stopping updates. Explicit `open` recreates a departed pane, including after cold restart; explicit `detach` clears a validated orphan. Neither action reclaims a surviving or restored owner.

The package includes compiled entrypoints, docs and `herdr-plugin.toml`. Local consumers build explicitly; there is no prepare or postinstall hook.

## Planned document actions

Current document previews are read-only and stay in the card pane.
The agreed next-iteration direction is to open documents in an editor in a new tab; this is not implemented or approved for coding yet.
[Herdr Annotate / Plannotator](https://github.com/plannotator/herdr-annotate) is a deferred candidate for future document review.
Source research indicates its plugin document entrypoint can open existing Markdown.
Its [feedback delivery](https://github.com/plannotator/plannotator-tui/blob/main/crates/plannotator-tui/src/delivery.rs) targets a Herdr pane through `herdr agent prompt` (terminal input), not a callback pinned to a Pi session.
A replacement agent in that same pane could receive the feedback.
These findings are source research only; the integration has not been installed or runtime-tested.
