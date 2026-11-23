// Smoke tests for the split herdr-app UI modules (main, runtime, frame, sidebar,
// view_all, view_stream, view_session, viewer, popovers, ui) using stubbed gpui/herdr modules.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";

const mockEnv = {
  checking: false,
  props: {},
  spaces: [],
  panes: [],
  terminals: [],
  agents: [],
  machines: [{ name: null, layout: "ready" }],
  focused: null,
  layout: null,
  window: { active: true },
  shared: new Map(),
  runs: [],
  overlays: [],
  closedOverlays: [],
  exec: async () => ({ code: 0, stdout: JSON.stringify({ items: [], unreadable: [] }), stderr: "" }),
};

function makeNode(tag, initial = {}) {
  const node = {
    tag,
    _id: null,
    _children: [],
    handlers: {},
    props: { ...initial },
  };
  const proxy = new Proxy(node, {
    get(target, prop) {
      if (prop === "id") return (val) => { target._id = val; return proxy; };
      if (prop === "child") return (c) => { if (c != null) target._children.push(c); return proxy; };
      if (prop === "children") return (arr) => { for (const c of arr ?? []) if (c != null) target._children.push(c); return proxy; };
      if (prop === "when") return (cond, fn) => (cond ? fn(proxy) ?? proxy : proxy);
      if (prop === "trigger") return (t) => { target._children.push(t); return proxy; };
      if (prop === "content") return (c) => { target._children.push(c); return proxy; };
      if (prop === "on_click") return (fn) => { target.handlers.click = fn; return proxy; };
      if (prop === "on_mouse_down") return (...args) => { target.handlers.mouse_down = args.at(-1); return proxy; };
      if (prop === "on_key_down") return (fn) => { target.handlers.key_down = fn; return proxy; };
      if (prop === "on_hover") return (fn) => { target.handlers.hover = fn; return proxy; };
      if (prop in target) return target[prop];
      return (...args) => {
        // Execute hover/style callback builders so any ReferenceError inside them fires too.
        if (prop === "hover" && typeof args[0] === "function") {
          args[0](proxy);
        }
        return proxy;
      };
    },
  });
  return proxy;
}

function findEl(root, id) {
  if (!root || typeof root !== "object") return null;
  if (root._id === id) return root;
  for (const child of root._children ?? []) {
    const found = findEl(child, id);
    if (found) return found;
  }
  return null;
}

globalThis.__STREAMS_UI_TEST_ENV__ = { mockEnv, makeNode };

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier === "gpui-kit" || specifier === "gpui-base" || specifier === "herdr") {
      return { url: `stub:${specifier}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url === "stub:gpui-kit") {
      return {
        format: "module",
        shortCircuit: true,
        source: `
          const { makeNode } = globalThis.__STREAMS_UI_TEST_ENV__;
          export class View {}
          export const div = () => makeNode("div");
          export const svg = (path) => makeNode("svg", { path });
          export const image = (path) => makeNode("image", { path });
        `,
      };
    }
    if (url === "stub:gpui-base") {
      return {
        format: "module",
        shortCircuit: true,
        source: `
          const { makeNode } = globalThis.__STREAMS_UI_TEST_ENV__;
          export const h_flex = () => makeNode("h_flex");
          export const v_flex = () => makeNode("v_flex");
          export const ScrollHandle = { new: () => ({}) };
          export const Popover = { new: (id) => makeNode("popover", { id }).id(id) };
          export const Editor = { new: (st) => makeNode("editor", { st }) };
          export const EditorState = { new: (opts) => ({ opts, release() {} }) };
          export const TextView = { markdown: (id, md) => makeNode("markdown", { id, md }).id(id) };
          export const Input = { new: (st) => makeNode("input", { st }) };
          export const InputState = {
            new: (opts) => ({
              _val: "",
              _handlers: {},
              value() { return this._val; },
              set_value(v) { this._val = v; },
              set_selected_range() {},
              focus() {},
              on(ev, fn) { this._handlers[ev] = fn; },
              release() {},
            }),
          };
        `,
      };
    }
    if (url === "stub:herdr") {
      return {
        format: "module",
        shortCircuit: true,
        source: `
          const { mockEnv } = globalThis.__STREAMS_UI_TEST_ENV__;
          export const spaces = () => mockEnv.spaces;
          export const panes = () => mockEnv.panes;
          export const terminals = () => mockEnv.terminals;
          export const agents = () => mockEnv.agents;
          export const machines = () => mockEnv.machines;
          export const focused = () => mockEnv.focused;
          export const props = () => mockEnv.props;
          export const run = (cmd, args) => mockEnv.runs.push({ cmd, args });
          export const openOverlay = (opts) => mockEnv.overlays.push(opts);
          export const closeOverlay = (id) => mockEnv.closedOverlays.push(id);
          export const theme = () => ({
            background: "#111", chrome: "#181818", raised: "#222", popover: "#242424",
            hover: "#2a2a2a", selected: "#303030", border: "#333", foreground: "#eee",
            muted: "#aaa", dim: "#777", accent: "#4af", working: "#fa0", blocked: "#f44",
            done: "#4d4", ansi1: "#f44", ansi2: "#4d4", ansi3: "#fa0", ansi4: "#48f",
            ansi5: "#c4f", ansi6: "#4dd",
          });
          export const settings = () => ({ font_family: "JetBrains Mono" });
          export const home = () => "/Users/test";
          export const layout = () => mockEnv.layout;
          export const window = () => mockEnv.window;
          export const checking = () => mockEnv.checking;
          export const exec = (cmd, args, opts) => mockEnv.exec(cmd, args, opts);
          export const shared = (k) => mockEnv.shared.get(k);
          export const setShared = (k, v) => mockEnv.shared.set(k, v);
          export const notify = () => {};
        `,
      };
    }
    return nextLoad(url, context);
  },
});

const { default: StreamsMod } = await import("../main.js");
const {
  coordinator,
  isStreamsSidebarVisible,
  isSidebarDrivingObserve,
  registerSidebarMount,
  unregisterSidebarMount,
  waiterSetForRole,
  notifyStreamsViews,
  paneFrame,
  folder,
  collectSnapshot,
  resetRuntimeStateForTest,
} = await import("../runtime.js");
const { icon, statusStyle } = await import("../ui.js");
const { LABEL_SESSION, LABEL_STREAM } = await import("../state.js");

function makeCx() {
  return {
    focus_handle: () => ({ focus() {} }),
    spawn: (fn) => {
      const p = Promise.resolve().then(() => fn({ notify() {}, open_url() {} }));
      return { cancel() {}, promise: p };
    },
    notify() {},
    stop_propagation() {},
    write_to_clipboard() {},
  };
}

test("UI smoke: sidebar renders All, Stream, and Session views without ReferenceErrors", async () => {
  const s1 = {
    key: "h1/pi/s1",
    host_id: "h1",
    id: "s1",
    stream_id: "st1",
    stream: "h1/st1",
    title: "Refactor UI",
    status: "active",
    state: "Splitting main.js",
    state_short: "Splitting main.js",
    recap: "Split modules cleanly",
    remaining: "Run final checks",
    references: [{ kind: "link", url: "https://example.com", title: "Spec" }],
    artifacts: [],
    updated_at_ms: Date.now() - 60_000,
    detail: true,
  };
  const st1 = {
    key: "h1/st1",
    host_id: "h1",
    id: "st1",
    title: "Flash Reload",
    status: "active",
    goal: "Eliminate flashes",
    goal_short: "Eliminate flashes",
    sessions: [s1],
    sessionKeys: [s1.key],
    active_sessions: 1,
    total_sessions: 1,
    last_activity_ms: Date.now(),
    unreadable: [],
    detail: true,
  };
  coordinator.sessions.set(s1.key, s1);
  coordinator.streams.set(st1.key, st1);
  coordinator.recordHostMachine("h1", null);
  const bucket = coordinator._getListEntry(null, "local");
  bucket.keys = [st1.key];
  bucket.loaded = true;
  coordinator.lastCompletedAt = Date.now();

  mockEnv.spaces = [{ id: "sp1", machine: null, active: true, tabs: [{ id: "tab1", active: true, name: "tab" }] }];
  mockEnv.panes = [
    { pane: "p1", tab: "tab1", terminal: "t1", focused: true, cwd: "/Users/test/repo", program: { key: "pi", name: "pi", agent: true } },
    { pane: "p2", tab: "tab1", terminal: "t2", focused: false, cwd: "/Users/test/repo", program: { key: "zsh", name: "zsh", shell: true } },
  ];
  mockEnv.terminals = [
    { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } },
    { address: "t2", state: "live", labels: {} },
  ];
  mockEnv.agents = [{ address: "t1", app: "pi", program: { key: "pi", agent: true } }];
  mockEnv.focused = { pane: "p1" };
  mockEnv.layout = {
    id: "root",
    type: "row",
    children: [
      {
        id: "panel",
        type: "column",
        hidden: false,
        children: [
          {
            id: "panel-body",
            type: "stack",
            active: "streams",
            children: [{ id: "streams", type: "block", content: { mod: "streams" } }],
          },
        ],
      },
    ],
  };

  const cx = makeCx();
  mockEnv.props = { label: "Streams" };
  const sidebar = new StreamsMod();
  sidebar.init(mockEnv.props, cx);

  // Session view
  coordinator.preferredView = "session";
  const sessTree = sidebar.render(cx);
  assert.ok(findEl(sessTree, "session-header-title"));

  // Stream view
  coordinator.preferredView = "stream";
  const streamTree = sidebar.render(cx);
  assert.ok(findEl(streamTree, "stream-header-title"));
  assert.ok(findEl(streamTree, "stream-session-s1"));

  // All view (including when currentStream is null -> verifies Stage 1 spaceStreamKey fix!)
  coordinator.preferredView = "all";
  const allTree = sidebar.render(cx);
  assert.ok(findEl(allTree, `host-stream-${st1.key}`));

  await sidebar.close();
});

test("UI smoke: frame pill click (panel & popup fallback), viewer, popup, visibility, and role-scoped wakeups", async () => {
  const cx = makeCx();
  mockEnv.runs.length = 0;
  mockEnv.overlays.length = 0;

  // Frame role with NO sidebar panel -> clicking session pill opens popup overlay (catches Stage 2 `snap` bug!).
  mockEnv.layout = { id: "root", type: "row", children: [] };
  mockEnv.props = { pane: "p1", terminal: "t1" };
  const frame = new StreamsMod();
  frame.init(mockEnv.props, cx);
  const frameTree = frame.render(cx);
  const pill = findEl(frameTree, "ph-stream-p1");
  assert.ok(pill, "enrolled multi-pane tab should render session pill");
  pill.handlers.click({}, cx);
  assert.equal(mockEnv.overlays.length, 1);
  assert.equal(mockEnv.overlays[0].props.kind, "session-popup");
  assert.equal(mockEnv.overlays[0].props.space_id, "sp1");
  await frame.close();

  // Popup role
  mockEnv.props = { kind: "session-popup", session: coordinator.sessions.get("h1/pi/s1"), stream_title: "Flash Reload", overlay: "ov1" };
  const popup = new StreamsMod();
  popup.init(mockEnv.props, cx);
  const popupTree = popup.render(cx);
  assert.ok(findEl(popupTree, "popup-close"));
  await popup.close();

  // Viewer role (markdown + code editor)
  mockEnv.props = { kind: "reference-viewer", filename: "notes.md", title: "Notes", content: "# Hello", overlay: "ov2", truncated: true, reference: { kind: "document", path: "notes.md" } };
  const viewer = new StreamsMod();
  viewer.init(mockEnv.props, cx);
  const mdTree = viewer.render(cx);
  assert.ok(findEl(mdTree, "streams-md-viewer"));
  assert.ok(findEl(mdTree, "streams-viewer-close"));
  mockEnv.props = { kind: "reference-viewer", filename: "main.rs", title: "main.rs", content: "fn main() {}" };
  viewer.render(cx);
  assert.ok(viewer._editorState);
  await viewer.close();

  // Layout visibility & sidebar driving guard (catches hidden panel / inactive Files tab!)
  const hiddenLayout = {
    id: "panel",
    type: "column",
    hidden: true,
    children: [{ id: "panel-body", type: "stack", active: "streams", children: [{ id: "streams", type: "block", content: { mod: "streams" } }] }],
  };
  const filesActiveLayout = {
    id: "panel",
    type: "column",
    hidden: false,
    children: [{ id: "panel-body", type: "stack", active: "files", children: [{ id: "streams", type: "block", content: { mod: "streams" } }] }],
  };
  const visibleLayout = {
    id: "panel",
    type: "column",
    hidden: false,
    children: [{ id: "panel-body", type: "stack", active: "streams", children: [{ id: "streams", type: "block", content: { mod: "streams" } }] }],
  };
  assert.equal(isStreamsSidebarVisible(hiddenLayout), false);
  assert.equal(isStreamsSidebarVisible(filesActiveLayout), false);
  assert.equal(isStreamsSidebarVisible(visibleLayout), true);

  registerSidebarMount();
  mockEnv.layout = hiddenLayout;
  assert.equal(isSidebarDrivingObserve(), false);
  mockEnv.layout = visibleLayout;
  assert.equal(isSidebarDrivingObserve(), true);
  unregisterSidebarMount();
  assert.equal(isSidebarDrivingObserve(), false);

  // Role-scoped wakeups: "sidebar" wakes ONLY sidebarWaiters, "all" wakes both.
  let sidebarWoken = 0;
  let frameWoken = 0;
  waiterSetForRole("sidebar").add(() => { sidebarWoken += 1; });
  waiterSetForRole("frame").add(() => { frameWoken += 1; });
  notifyStreamsViews("sidebar");
  assert.equal(sidebarWoken, 1);
  assert.equal(frameWoken, 0);
  waiterSetForRole("sidebar").add(() => { sidebarWoken += 1; });
  notifyStreamsViews("all");
  assert.equal(sidebarWoken, 2);
  assert.equal(frameWoken, 1);
  resetRuntimeStateForTest();
});

test("UI helpers: paneFrame (real layout + hidden/stack), icon tint, statusStyle, folder boundary, focus.agent", () => {
  resetRuntimeStateForTest();
  // Real layouts/streams.json paneFrame resolution
  const realLayout = JSON.parse(
    readFileSync(new URL("../layouts/streams.json", import.meta.url), "utf8")
  );
  const pf = paneFrame(realLayout.root ?? realLayout);
  assert.ok(pf, "paneFrame must find content.pane in layouts/streams.json");
  assert.equal(pf.header?.content?.mod, "streams");
  assert.equal(pf.header?.position, "top");

  // Hidden/inactive stack node must be ignored by paneFrame
  assert.equal(
    paneFrame({
      id: "stack",
      type: "stack",
      active: "a",
      children: [
        { id: "a", type: "block", content: { mod: "other" } },
        { id: "b", type: "block", content: { tab: true, pane: { header: { position: "bottom" } } } },
      ],
    }),
    null
  );

  // icon() uses p.tint (not icon_kind)
  const tinted = icon({ icon: "icons/pi.svg", tint: true }, 14, "#fff");
  assert.equal(tinted.tag, "svg");
  const untinted = icon({ icon: "icons/app.png", tint: false }, 14, "#fff");
  assert.equal(untinted.tag, "image");
  assert.equal(icon({}, 14, "#fff"), null);

  // statusStyle preserves working/offline/done/parked labels and theme colors
  const t = { ansi2: "#0f0", working: "#fa0", blocked: "#f00", done: "#4d4", dim: "#777" };
  assert.deepEqual(statusStyle("offline", t), { color: "#777", symbol: "○", label: "offline" });
  assert.deepEqual(statusStyle("working", t), { color: "#fa0", symbol: "●", label: "working" });
  assert.deepEqual(statusStyle("done", t), { color: "#4d4", symbol: "✓", label: "done" });
  assert.deepEqual(statusStyle("parked", t), { color: "#777", symbol: "○", label: "parked" });

  // folder() enforces '/' boundary after homeDir
  assert.equal(folder("/Users/test"), "~");
  assert.equal(folder("/Users/test/repo"), "~/repo");
  assert.equal(folder("/Users/test2/repo"), "/Users/test2/repo");

  // collectSnapshot falls back to focus.agent when focused pane is null
  mockEnv.spaces = [{ id: "sp1", machine: null, active: true, tabs: [{ id: "tab1", active: true }] }];
  mockEnv.panes = [];
  mockEnv.focused = { agent: "t-agent-block" };
  assert.equal(collectSnapshot().focusedTerminal, "t-agent-block");
  resetRuntimeStateForTest();
});
