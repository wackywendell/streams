# Active Streams renderer fixtures

`active-renderer.json` preserves 134 safe-input outputs from the retired
JavaScript `renderStreamsCard` implementation. Its snapshots use the current
reference schema with deterministic revision tokens. Six unsafe URL inputs moved
to codec rejection tests; repository history retains their original outputs.
`card-text-renderer.json` records 150 current Rust layouts, visible document keys
and links, including 16 progress and lifecycle variants. Its historical fields
retain the original renderer outputs.

The original renderer, tests and `extract.mjs` remain in revision `fd744307`.
Reproduce them in a separate workspace at that revision, not from the current Rust
renderer. The historical extractor used only the pure renderer: no pane, catalog
read, document resolution or Herdr/CMUX call.

Cases come from the active `streams-view`, `streams-documents`, `quiet-view` and
`link-display` tests: domain statuses; exact viewing contribution; combining
characters, CJK, emoji modifiers, regional indicators and ZWJ input; zero/tiny
viewports; footer-clipped and offscreen shortcuts; row-major document grids;
multiline summaries; overflow item counts; distinguishing checkout ancestors;
direct repository URLs and unbound references; recovery provenance; missing
document, missing checkout and ambiguous binding errors; duplicate names;
reference grouping, roles and states; link ordering, exact URLs and unsafe URL
rejection; empty and failed frames. Rust reducer tests cover refresh-in-progress/stale frames, superseded
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

The migration accepted standard Rust layout rather than adding a Unicode
compatibility patch. The original JavaScript outputs remain historical evidence;
current expected lines are in `card-text-renderer.json`. The fixture test checks
every input against those lines, visible document keys, exact document targets and
links. Native activation is not included.

The historical JavaScript sanitizer stripped all Cc/Cf scalars, including ZWJ, before
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
ownership and filesystem evidence. Numeric URL labels remain inert as keyboard
shortcuts. The renderer returns complete saved References and hit regions.
The terminal adapter guards link gestures with fresh full-reference comparisons.

Document targets retain their complete saved Reference, viewing scope,
owner observation, checkout ID, root, app-resolved file or visible error.
Dropped entries produce no normal-card action or overflow count. Revision,
relationship or target changes invalidate the painted reference. Editor
identity retains catalog source, host, checkout lifetime and canonical file. It
is not a reuse authorization: the native layer must also verify viewing Session,
workspace, editor tuple, canonical containment, root identity and current file
existence. Missing documents never trigger reference cleanup or checkout fallback.
