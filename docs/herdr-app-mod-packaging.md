# Design: Composable herdr-app Mod Packaging and Layout Slot Contract

- **Author**: Wendell Smith
- **Date**: 2026-11-15
- **Status**: Draft

## The Problem

`herdr-app` layouts assemble a workspace from independent JavaScript mods mounted into layout tree slots: the left sidebar (`spaces`, `agents`), the top tab bar (`tabs`), the pane header (`pane.header`), and the right-hand panel stack (`panel-body`). Because `herdr-app` mods cannot import JavaScript across mod directories and layout JSON files have no inheritance mechanism, a mod package that assumes it owns the entire layout or requires a forked tab bar mod cannot compose with other installed mod packages.

Specifically, the `streams` mod needs two visual mount points in a `herdr-app` layout:

1. A 30 px pane header (`pane.header.content`) that displays the normal pane controls for ordinary terminals and adds a clickable `Stream · Session` pill when a terminal hosts an enrolled Streams session.
2. A right-hand inspector panel inside the `panel-body` stack alongside `files`, `changes`, or other installed panel mods.

Currently, three packaging friction points get in the way of clean composition:

1. **Redundant `role` prop requirements:** Mounting `streams` in both `pane.header` and `panel-body` works by inferring the role when `props().pane` or `props().terminal` is present, yet example layouts and documentation still treat explicit `role` and `streams_bin` props as required boilerplate.
2. **Missing panel metadata for layout-driven tab bars:** When mounted as a child of the `panel-body` stack, a tab bar mod that discovers panel buttons from `layout()` needs a standard block convention (`id`, plus optional `props.label` and `props.icon`) so it can render a toggle button without hardcoding `"streams"` in the tab bar mod's source code.
3. **Lack of an installable standalone layout and installer task:** While `integrations/herdr-app/` provides `mod.json`, `main.js`, and `state.js`, the repository does not yet ship a standalone example layout (`integrations/herdr-app/layouts/streams.json`) or a reproducible install task that installs `mods/streams` without clobbering a user's custom or multi-package layout.

## Overview

Package `integrations/herdr-app` as a self-contained, slot-composable `herdr-app` mod with an optional reference layout:

1. **Zero-Boilerplate Slot Mounting:** A layout mounts `{ "mod": "streams" }` in `pane.header.content` and/or as a block inside `panel-body` without needing a forked tab bar mod or mandatory `role` props. Role resolution remains deterministic: popup overlay (`props().kind === "session-popup"`), pane header frame (`props().pane` or `props().terminal` supplied by `herdr-app`), and sidebar inspector otherwise.
2. **Unicode Symbol `panel-body` Block Metadata:** Mount `{ "id": "streams", "type": "block", "content": { "mod": "streams", "props": { "label": "Streams", "icon": "≋" } } }` inside `panel-body` (and prefix the pane header pill with `≋`). Layout-driven tab bars render `≋ Streams` alongside `Files` and `Changes` with zero external SVG files.
3. **Drop-In `pane` Header Parity:** Keep the `frame` role as a strict behavioral superset of Herdr's shipped `pane` mod (30 px height, program icon, title, home-shortened `cwd`, status indicators, and hover split/zoom/close buttons), adding the `Stream · Session` badge only when `streams` reports an enrolled session for that terminal.
4. **Standalone Example Layout vs. Mod-Only Installation:** Provide `integrations/herdr-app/layouts/streams.json` (built on Herdr's stock `main.json` slots) for standalone users, while separating mod installation (`mods/streams`) from layout installation so users combining multiple mod packages in their own dotfiles layout only need to add the `streams` blocks to their layout JSON.

## Goals

- Allow `mods/streams` to be mounted into any `herdr-app` layout's `pane.header` and `panel-body` slots without forking `tabs` or conflicting with other sidebar, VCS, or agent-list mods.
- Use the Unicode triple-wave symbol (`≋`) for the `Streams` panel button (`props: { label: "Streams", icon: "≋" }`) and pane header pill, requiring zero SVG files.
- Provide a reference `integrations/herdr-app/layouts/streams.json` layout and `Justfile` install recipes (`just install`, `just install-cli`, `just install-herdr-app-mod`, `just install-herdr-app`) with tool versions in `.tool-versions` and `.envrc`.
- Keep the Complexity Budget minimal: zero new daemon processes, zero cross-mod imports, and a 2-minute comprehension target for adding `streams` to an existing layout JSON.

## Operating Assumptions, Failure Boundaries & Non-Goals

### Operating Assumptions

- **Runtime environment:** Single-user desktop `herdr-app` (0.1.15+) running on macOS with a local `herdr-core` (`hc`) daemon and the `streams` CLI available on `PATH` (or specified via `props().streams_bin`).
- **Sandbox constraints:** `herdr-app` mods execute inside Herdr's QuickJS/GPUI view runtime and can only import files within their own `mods/<name>/` directory. Layout JSON files are parsed directly by Herdr's Rust layout loader with no native `extends` or include directive.
- **Single writer per piece of state:** `streams` (Rust CLI) owns catalog and `hc` association state; `herdr-app` and the active tab-bar mod own panel visibility and layout state.

### Failure Boundaries

- **Missing or failing `streams` CLI:** In the `frame` role (pane header), if `streams integration describe` or `request` fails or is still loading, the header renders the standard `pane` controls without error text so terminal chrome never breaks. In the `sidebar` role, the panel displays the error string from `describeError` or `lastError` directly with a `Refresh` button and performs no background retries until the next user action or observation change.
- **Unconfigured tab bar:** If a layout mounts `streams` in `panel-body` while using Herdr's stock unpatched `tabs` mod (which only renders buttons for `files` and `changes`), `streams` does not attempt to mutate the layout tree or force the panel open from the background; clicking the `Stream · Session` pill in the pane header still opens and selects the `streams` panel explicitly via `layout.select` and `layout.show`.

### Non-Goals

- Modifying or shipping VCS-specific mods (`spaces`, `changes`, `worktree`) or custom tab bar mods (`streams-tabs`) inside the `streams` repository.
- Inventing a custom layout-patching or JSON-inheritance preprocessor inside `streams`.
- Altering the Rust `streams integration` CLI protocol or `hc` association model.
- Supporting multi-tenant quotas, remote daemon telemetry, or automated background layout migrations.

## Technical Plan

### 1. Slot Architecture & Data Flow

In `herdr-app`, a workspace layout is a declarative JSON tree of container nodes (`row`, `column`, `stack`) and leaf `block` nodes. `mods/streams` participates in three mount surfaces within `herdr-app` without touching the left sidebar (`spaces`, `agents`), the top tab bar (`tabs`), or other `panel-body` children (`files`, `changes`):

```
herdr-app Layout Tree
├── sidebar (left column: spaces, agents, workspace-chrome)   <-- Untouched by streams
└── work (right column)
    ├── tabs (top bar: tabs / jj-tabs / custom)               <-- Discovers "streams" in panel-body
    ├── work-body (row)
    │   ├── main (active tab panes)
    │   │   └── pane.header.content: { "mod": "streams" }     <-- Role: "frame" (30 px header)
    │   └── panel (right side panel, collapsible)
    │       └── panel-body (stack)
    │           ├── { "id": "streams", "mod": "streams",      <-- Role: "sidebar" (Stream/Session view)
    │           │     "props": { "label": "Streams", "icon": "≋" } }
    │           ├── { "id": "files",   "mod": "files" }
    │           └── { "id": "changes", "mod": "changes" }
    └── status (bottom bar: workspace-status)                 <-- Untouched by streams
```

Each refresh of the active tab spawns one `streams integration request`. Its `tab` selection returns the tab's Stream view (from the first enrolled Session in pane order, else the space association, else the `streams.work` label) or the host's Streams, and its `frame` selections return each pane's enrolled Session for the pane header. Until a read shows that the tab has a Stream, the request also carries a `frame` for every live agent pane in the window, which the host listing uses to mark live Streams; once the tab's Stream is known it reads only the tab's own panes, so agents starting elsewhere and focus moves within the tab start no spawn.

When a user clicks the `Stream · Session` pill in the 30 px pane header:
1. If the right-hand `panel` is hidden or showing another stack child (`files` or `changes`), `mods/streams` runs `layout.select({ stack: "panel-body", child: "streams" })` and `layout.show({ id: "panel" })`.
2. If `panel` is already visible with `panel-body.active === "streams"`, clicking the pill opens the compact `session-popup` overlay (`openOverlay({ mod: "streams", kind: "popup", ... })`) anchored to the pane header.

### 2. Automatic Role Resolution & Prop Contract

`resolveRole(rawProps)` in `integrations/herdr-app/state.js` already resolves the role without requiring `props().role` in the layout JSON:
1. If `rawProps.role` is explicitly `"popup"`, `"frame"`, or `"sidebar"`, honor it.
2. Otherwise, if `rawProps.kind === "session-popup"`, return `"popup"`.
3. Otherwise, if `rawProps.pane` or `rawProps.terminal` is non-empty (injected automatically by `herdr-app` for every `pane.header` instance), return `"frame"`.
4. Otherwise, return `"sidebar"`.

This means a layout author can mount `{ "mod": "streams" }` in `pane.header.content` with zero `props`, or optionally pass `{ "streams_bin": "/path/to/streams" }` when overriding `PATH`.

### 3. Standard `panel-body` Metadata & Unicode Icon

To allow layout-driven tab bar mods to render a toggle button for every child in `panel-body` without hardcoding `"streams"`, the `streams` block in `panel-body` declares standard metadata in `content.props`:

```json
{
  "id": "streams",
  "type": "block",
  "content": {
    "mod": "streams",
    "props": {
      "label": "Streams",
      "icon": "≋"
    }
  }
}
```

Using the Unicode triple-wave symbol (`≋`) directly in `content.props.icon` requires zero external SVG files and resolves identically regardless of which other mods are installed.

### 4. Standalone Reference Layout & Installation Tasks

`integrations/herdr-app/layouts/streams.json` will be added to the repository as a clean clone of Herdr's stock `main.json` (omitting `"preview"` just like `main.json` so Herdr's Settings screen draws its native theme-aware layout thumbnail) with only two changes:
1. `pane.header.content` set to `{ "mod": "streams" }`.
2. `panel-body.children` prepended with `{ "id": "streams", "type": "block", "content": { "mod": "streams", "props": { "label": "Streams", "icon": "≋" } } }`.

In `Justfile` (backed by `.tool-versions` and `.envrc`), installation is split into explicit recipes so mod installation never overwrites a user's personal or multi-package layout unless requested:
- `just install-cli`: installs the release `streams` binary into `${STREAMS_INSTALL_ROOT:-~/.local}/bin/streams` via `cargo install --locked --path . --root "$root" --bin streams --force`.
- `just install-herdr-app-mod`: copies `integrations/herdr-app/{mod.json,main.js,state.js}` into `${HERDR_APP_HOME:-~/.config/herdr-app}/mods/streams/`.
- `just install`: runs `install-cli` and `install-herdr-app-mod`.
- `just install-herdr-app`: runs `install-herdr-app-mod` and also copies `integrations/herdr-app/layouts/streams.json` into `${HERDR_APP_HOME:-~/.config/herdr-app}/layouts/`.


## Alternatives Considered

### 1. Ship a Forked `streams-tabs` Mod in the `streams` Repository
- **Mechanism:** Clone Herdr's shipped `tabs` mod into `integrations/herdr-app/mods/streams-tabs/` and hardcode a `Streams` panel button next to `Files` and `Changes`.
- **Rejected by author:** A forked tab-bar mod breaks composition whenever a user installs another package that also customizes the tab bar (such as a VCS package with its own menu hooks). Keeping `streams` confined to `mods/streams` (mounted in `pane.header` and `panel-body`) and declaring standard `label`/`icon` metadata on the `panel-body` block lets any layout-driven tab bar render the `Streams` button without conflict.

### 2. Require Explicit `role: "frame"` and `role: "sidebar"` Props in Layout JSON
- **Mechanism:** Require layout authors to pass `"props": { "role": "frame" }` in `pane.header.content` and `"props": { "role": "sidebar" }` in `panel-body`.
- **Rejected by author:** `herdr-app` already passes `{ pane, terminal }` props to every `pane.header` mod instance and `{ kind: "session-popup", overlay }` to popup overlays. Inferring the role from those structural props removes redundant layout JSON configuration while still allowing an explicit `role` prop override in tests.

### 3. Mutate `shared("workspace.panels")` from Inside `mods/streams` at Startup
- **Mechanism:** Have `mods/streams` call `setShared("workspace.panels", ["streams", ...])` during `init()` so tab bars that filter strictly on `shared("workspace.panels")` include `"streams"`.
- **Rejected by author:** Violates single-writer ownership (`AGENTS.md`). `shared("workspace.panels")` is owned by Herdr's Welcome/Settings UI, and unpatched stock `tabs` still only checks `enabled.includes("files")` and `enabled.includes("changes")` anyway. Layout-driven tab bars should inspect `panel-body.children` directly rather than relying on panel mods mutating global shared preferences as a side effect.


## Detailed Implementation

1. **Create `./integrations/herdr-app/layouts/streams.json`**:
   - **What changes:** Add a standalone reference layout JSON (based on Herdr's stock `main.json`, with no `preview` SVG file) that mounts `{ "mod": "streams" }` in `pane.header.content` and prepends `{ "id": "streams", "type": "block", "content": { "mod": "streams", "props": { "label": "Streams", "icon": "≋" } } }` in `panel-body.children`.
   - **Why:** Provides an out-of-the-box layout for users installing `streams` standalone while demonstrating the minimal 2-slot block contract for users integrating `streams` into a shared layout.

2. **Modify `./integrations/herdr-app/main.js` and `./integrations/herdr-app/tests/state.test.mjs`**:
   - **What changes:** Prefix the pane header pill with `≋ `, ensure frame-pill click navigation targets the `panel-body` child ID matching `props()?.panel_id ?? "streams"` (defaulting to `"streams"`), and verify in unit tests that `resolveRole` handles empty `props()` (defaulting to `"sidebar"`), pane-injected `{ pane, terminal }` props (defaulting to `"frame"`), and popup `{ kind: "session-popup" }` props (defaulting to `"popup"`).
   - **Why:** Keeps the `≋` marker consistent between the pane header pill and tab bar button and guarantees zero-boilerplate mounting when `role` is omitted in layout JSON.

3. **Create `./.tool-versions`, `./.envrc`, and `./Justfile` (and untrack `mise.toml`)**:
   - **What changes:** Pin `nodejs`, `rust`, `just`, and `direnv` in `.tool-versions`, activate `mise env` in `.envrc`, define build/test/install recipes (`install`, `install-cli`, `install-herdr-app-mod`, `install-herdr-app`) in `Justfile`, and remove `mise.toml` from version control.
   - **Why:** Uses standard `.tool-versions`, `Justfile`, and `.envrc` conventions and separates CLI/mod installation from layout installation.

4. **Modify `./README.md` and `./docs/hc-herdr-app-plan.md`**:
   - **What changes:** Document `just install`, `just install-herdr-app-mod`, and `just install-herdr-app`, the zero-boilerplate `pane.header` + `panel-body` slot snippets, and the `label` / `icon` `panel-body` block metadata convention for composing `mods/streams` with layout-driven tab bars.
   - **Why:** Gives users clear instructions for both standalone installation and composing `mods/streams` into an existing layout JSON.
