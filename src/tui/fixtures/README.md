# Active Streams renderer fixtures

`active-renderer.json` records 140 deterministic outputs of the active
`renderStreamsCard` implementation, before replacing or retiring any JavaScript.
The extractor imports only the pure renderer. It does not run a pane, read a
catalog, resolve documents on disk, or call Herdr/CMUX.

To reproduce from the repository root:

```sh
mise exec -- npm ci --ignore-scripts --prefix integrations/herdr-work-cards
mise exec -- npm --prefix integrations/herdr-work-cards run build
mise exec -- node src/tui/fixtures/extract.mjs
```

Cases come from the active `streams-view`, `streams-documents`, `quiet-view` and
`link-display` tests: domain statuses; exact viewing contribution; combining
characters, CJK, emoji modifiers, regional indicators and ZWJ input; zero/tiny
viewports; footer-clipped and offscreen shortcuts; row-major document grids;
multiline summaries; overflow item counts; distinguishing checkout ancestors;
shared-root aliases and unbound references; commit provenance; missing document,
missing checkout and ambiguous binding errors; duplicate link names; artifact
numbering, link ordering, exact URLs and unsafe URL rejection; empty and failed
frames. Rust reducer tests cover refresh-in-progress/stale frames, superseded
reads, scope changes, geometry changes and captured-target invalidation.

The fixture boundary alone uses serde DTOs. Production rendering consumes the
existing validated `Stream` and `Session`, plus typed application-supplied display
and document-location observations. No fixture native tuple is an attestation.

## Accepted Rust Unicode layout

The five cases named `unicode-aൎaൎaൎaൎaൎaൎ-{1,5,18,35,80}` differ only at the final
standalone U+0D4E MALAYALAM LETTER DOT REPH. JavaScript `string-width` 8.3 counts
that standalone grapheme as one cell. `unicode-width` 0.2.0 / ratatui 0.29 treats
it as zero-width; `Paragraph` does not paint the trailing standalone grapheme.
At width 1 this also removes its standalone content row, moving the following
section up one row. At widths 5, 18, 35 and 80 it removes only the trailing scalar.
Interior U+0D4E-plus-letter graphemes remain intact.

The migration explicitly accepts standard Rust layout for these five cases rather
than adding a Unicode compatibility patch. `unicode-width-differences.json` records
the exact approved line edits. The original JavaScript goldens remain unchanged.
The fixture test asserts 135 exact matches and the five approved Rust outputs,
including their unchanged document targets and links. No parity tests are ignored,
and no broader Unicode differences are accepted. Native activation is not included.

The active JavaScript sanitizer strips all Cc/Cf scalars, including ZWJ, before
layout. Rust preserves that policy: a supplied woman-technologist sequence is
painted as separate woman and laptop glyphs. Combining marks and emoji skin-tone
modifiers are not stripped. Ratatui owns wrapping and grapheme-safe painting;
there is no ANSI parser in the Rust renderer.

## Integration contract

Create `CardSnapshot` from domain records and supplied `DisplayContext`, then feed
it to `CardView::complete_refresh` using a token from `begin_refresh`. The caller
provides observations; `CardView` creates its own unique lifetime. `paint(Rect)` returns a ratatui
`Buffer` and frame-bound document shortcuts and visible hyperlink regions. The
buffer can be copied to a terminal frame by the eventual binary; this module has
no terminal backend dependency or keyboard loop.

Call `begin_refresh` before dispatching reads and `geometry_changed` before
queuing asynchronous resize paints. Refreshing snapshots may be repainted, but
have no actionable documents or links. Failed reads display no stale content.
Every new paint and notice change invalidates old action captures. Check
`is_current` after each await in an action path, then separately obtain native
ownership and filesystem evidence. Numeric issue/PR labels remain inert as
keyboard shortcuts. The renderer returns typed URLs and regions, not OSC8 bytes;
the future terminal adapter must implement guarded link gestures.

Document targets retain repository URL, recovery ref, provenance, viewing scope,
owner observation, checkout ID, root, app-resolved file or visible error. Editor
identity retains catalog source, host, checkout lifetime and canonical file. It
is not a reuse authorization: the native layer must also verify viewing Session,
workspace, editor tuple, canonical containment, root identity and current file
existence. Missing documents never trigger reference cleanup or checkout fallback.
