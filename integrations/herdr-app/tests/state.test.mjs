import test from "node:test";
import assert from "node:assert/strict";
import {
  LABEL_SESSION,
  LABEL_STREAM,
  LABEL_SAVED,
  LABEL_HERDR_SPACE,
  resolveRole,
  observeNoticeToken,
  activeTabSession,
  tabSidebarView,
  resolveTabRenameTarget,
  effectiveSessionStatus,
  buildReferenceItems,
  prepareNativeContent,
  filterHostStreams,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
  liveStreamIds,
  findSpaceForStream,
  dispatchReferencePresentation,
  StreamsCoordinator,
} from "../state.js";

function makeSessionJson({
  host = "h1",
  harness = "pi",
  id = "sess-1",
  stream_id = "st-1",
  title = "Implement CLI",
  status = "active",
  recap = "Drafted parser",
  state = "Running tests",
  remaining = "Finish docs",
  directory = { path: "/work/repo", repository: null },
  references = {},
  artifacts = {},
  document_files = {},
  updated_at_ms = 1_700_000_000_000,
} = {}) {
  return {
    key: `${host}/${harness}/${id}`,
    host_id: host,
    harness,
    id,
    stream_id,
    title: { text: title, manual: false },
    status,
    recap: recap ? { text: recap, short: null } : null,
    state: state ? { text: state, short: null } : null,
    remaining: remaining ? { text: remaining, short: null } : null,
    directory,
    references,
    artifacts,
    document_files,
    revision: 1,
    updated_at_ms,
  };
}

function makeStreamJson({
  host = "h1",
  id = "st-1",
  title = "CLI Redesign",
  goal = "Organize commands by entity",
  status = "active",
  sessions = [],
  unreadable = [],
  last_activity_ms = 1_700_000_000_000,
} = {}) {
  return {
    key: `${host}/${id}`,
    host_id: host,
    id,
    title: { text: title, manual: false },
    goal: goal ? { text: goal, short: null, manual: false } : null,
    status,
    sessions,
    unreadable,
    last_activity_ms,
    revision: 1,
    updated_at_ms: last_activity_ms,
  };
}

function createFakeRunner(handlers = {}) {
  const calls = [];
  const exec = async (bin, args) => {
    calls.push({ bin, args });
    if (bin === "hc") {
      if (handlers.hc) return handlers.hc(args);
      return { code: 0, stdout: "", stderr: "" };
    }
    if (args[0] === "config" && args[1] === "get" && args[2] === "herdr") {
      const herdr = handlers.herdrConfig ?? {
        naming: { tab: true },
        viewer: { mode: "editor", placement: "pane-right" },
      };
      return { code: 0, stdout: JSON.stringify({ herdr }), stderr: "" };
    }
    if (args[0] === "session" && args[1] === "show") {
      const key = args[2];
      const found = handlers.sessions?.[key];
      if (!found) {
        return {
          code: 1,
          stdout: JSON.stringify({ error: { kind: "not-found", message: `no Session ${key}` } }),
          stderr: "",
        };
      }
      return { code: 0, stdout: JSON.stringify(found), stderr: "" };
    }
    if (args[0] === "stream" && args[1] === "show") {
      const key = args[2];
      const found = handlers.streams?.[key];
      if (!found) {
        return {
          code: 1,
          stdout: JSON.stringify({ error: { kind: "not-found", message: `no Stream ${key}` } }),
          stderr: "",
        };
      }
      return { code: 0, stdout: JSON.stringify(found), stderr: "" };
    }
    if (args[0] === "stream" && args[1] === "list") {
      if (handlers.streamListError) {
        return {
          code: 1,
          stdout: JSON.stringify({ error: { kind: "error", message: handlers.streamListError } }),
          stderr: "",
        };
      }
      return {
        code: 0,
        stdout: JSON.stringify({
          items: handlers.streamList ?? [],
          unreadable: handlers.streamListUnreadable ?? [],
          more: false,
        }),
        stderr: "",
      };
    }
    if (args[0] === "session" && args[1] === "list") {
      if (handlers.sessionList) {
        const res = handlers.sessionList(args);
        return { code: 0, stdout: JSON.stringify(res), stderr: "" };
      }
      const keyFlags = [];
      for (let i = 0; i < args.length; i++) {
        if (args[i] === "--key" && args[i + 1]) keyFlags.push(args[i + 1]);
      }
      if (keyFlags.length > 0) {
        const items = keyFlags.map((k) => handlers.sessions?.[k]).filter(Boolean);
        return { code: 0, stdout: JSON.stringify({ items, unreadable: [], more: false }), stderr: "" };
      }
      const streamIdx = args.indexOf("--stream");
      const streamKey = streamIdx >= 0 ? args[streamIdx + 1] : null;
      const items = Object.values(handlers.sessions ?? {}).filter(
        (s) => !streamKey || `${s.host_id}/${s.stream_id}` === streamKey
      );
      return { code: 0, stdout: JSON.stringify({ items, unreadable: [], more: false }), stderr: "" };
    }
    if (args[0] === "session" && args[1] === "document" && args[2] === "read") {
      const body = handlers.documentRead?.(args[3], args[4]);
      if (body === undefined) {
        return { code: 1, stdout: "", stderr: "document not found" };
      }
      return { code: 0, stdout: body, stderr: "" };
    }
    if (args[0] === "artifact" && args[1] === "read") {
      const body = handlers.artifactRead?.(args[2]);
      if (body === undefined) {
        return { code: 1, stdout: "", stderr: "artifact not found" };
      }
      return { code: 0, stdout: body, stderr: "" };
    }
    return { code: 1, stdout: "", stderr: `unexpected command: ${bin} ${args.join(" ")}` };
  };
  return { exec, calls };
}

test("resolveRole selects popup, frame, viewer, and sidebar roles deterministically", () => {
  assert.equal(resolveRole({ role: "popup" }), "popup");
  assert.equal(resolveRole({ role: "frame" }), "frame");
  assert.equal(resolveRole({ role: "viewer" }), "viewer");
  assert.equal(resolveRole({ role: "sidebar", pane: "p1" }), "sidebar");
  assert.equal(resolveRole({ kind: "reference-viewer", filename: "a.md" }), "viewer");
  assert.equal(resolveRole({ kind: "session-popup" }), "popup");
  assert.equal(resolveRole({ kind: "stream-menu" }), "popup");
  assert.equal(resolveRole({ pane: "p1", terminal: "t1" }), "frame");
  assert.equal(resolveRole({ terminal: "t1" }), "frame");
  assert.equal(resolveRole({}), "sidebar");
});

test("observeNoticeToken combines only local terminal streams.saved tokens", () => {
  const token = observeNoticeToken(
    [
      { address: "t2", labels: { [LABEL_SAVED]: "u2" } },
      { address: "t1", labels: { [LABEL_SAVED]: "u1" } },
      { address: "remote/t3", labels: { [LABEL_SAVED]: "u3" } },
      { id: "t4", machine: null, labels: { [LABEL_SAVED]: "u4" } },
      { id: "remote/t5", machine: "remote", labels: { [LABEL_SAVED]: "u5" } },
    ],
    LABEL_SAVED
  );
  assert.equal(token, "t1:u1|t2:u2|t4:u4");
});

test("refresh_reads_the_focused_session", async () => {
  const s1 = makeSessionJson({
    id: "s1",
    stream_id: "abc12345",
    title: "Session One",
    references: {
      "https://github.com/org/repo/pull/43": {
        kind: "pr",
        name: "PR 43",
        purpose: "Implements entity CLI",
        role: "deliverable",
        status: "active",
        manual: false,
      },
    },
  });
  const st = makeStreamJson({
    id: "abc12345",
    title: "CLI Redesign",
    sessions: [s1],
  });
  const runner = createFakeRunner({
    sessions: { [s1.key]: s1 },
    streams: { [st.key]: st },
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });

  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      {
        address: "t1",
        state: "live",
        labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st.key },
      },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } }],
    agents: [{ address: "t1", agent: "pi" }],
  });

  const cliCalls = runner.calls.filter((c) => c.bin === "streams").map((c) => c.args);
  assert.deepEqual(cliCalls, [
    ["config", "get", "herdr", "--json"],
    ["session", "show", s1.key, "--json"],
    ["stream", "show", st.key, "--json"],
  ]);
  const frame = coord.frames.get("t1");
  assert.equal(frame?.kind, "enrolled");
  assert.equal(frame.session.key, s1.key);
  assert.equal(coord.tabStreams.get("tab1"), st.key);
});

test("focus_change_in_a_tab_refreshes", async () => {
  const s1 = makeSessionJson({ id: "s1", stream_id: "st1", title: "First" });
  const s2 = makeSessionJson({
    id: "s2",
    stream_id: "st1",
    title: "Second",
    references: {
      "docs/plan.md": {
        kind: "document",
        name: "Plan",
        purpose: "Second session's brief",
        role: "brief",
        status: "active",
        manual: true,
      },
    },
  });
  const st = makeStreamJson({ id: "st1", title: "Work", sessions: [s1, s2] });
  const runner = createFakeRunner({
    sessions: { [s1.key]: s1, [s2.key]: s2 },
    streams: { [st.key]: st },
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });
  const common = {
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1", "t2"],
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st.key } },
      { address: "t2", state: "live", labels: { [LABEL_SESSION]: s2.key, [LABEL_STREAM]: st.key } },
    ],
    panes: [
      { pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } },
      { pane: "p2", tab: "tab1", terminal: "t2", program: { agent: "pi" } },
    ],
    agents: [
      { address: "t1", agent: "pi" },
      { address: "t2", agent: "pi" },
    ],
  };

  await coord.observe({ ...common, focusedTerminal: "t1" });
  const view1 = tabSidebarView({
    tabId: "tab1",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    frames: coord.frames,
    visibleTerminals: ["t1", "t2"],
    focusedTerminal: "t1",
  });
  assert.equal(view1.kind, "session");
  assert.equal(view1.session.key, s1.key);
  assert.equal(buildReferenceItems(view1.session).length, 0);

  await coord.observe({ ...common, focusedTerminal: "t2" });
  const view2 = tabSidebarView({
    tabId: "tab1",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    frames: coord.frames,
    visibleTerminals: ["t1", "t2"],
    focusedTerminal: "t2",
  });
  assert.equal(view2.kind, "session");
  assert.equal(view2.session.key, s2.key);
  const refs2 = buildReferenceItems(view2.session);
  assert.equal(refs2.length, 1);
  assert.equal(refs2[0].title, "Plan");

  const showCalls = runner.calls
    .filter((c) => c.bin === "streams" && c.args[0] === "session" && c.args[1] === "show")
    .map((c) => c.args[2]);
  assert.deepEqual(showCalls, [s1.key, s2.key]);
});

test("stale_session_label_is_ignored", async () => {
  const hostStream = makeStreamJson({ id: "st-pick", title: "Pick Me" });
  const runner = createFakeRunner({
    sessions: {},
    streamList: [hostStream],
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });

  // Case 1: `streams.session` points to a deleted/missing Session -> falls back to picker.
  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t-stale"],
    focusedTerminal: "t-stale",
    terminalList: [
      { address: "t-stale", state: "live", labels: { [LABEL_SESSION]: "h1/pi/deleted" } },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t-stale", program: { agent: "pi" } }],
    agents: [{ address: "t-stale", agent: "pi" }],
  });
  assert.equal(coord.tabStreams.get("tab1"), null);
  assert.equal(coord.frames.has("t-stale"), false);
  assert.equal(coord.hostStreams.length, 1);

  // Case 2: terminal carries `streams.session` but has no live Pi agent -> ignored without `session show`.
  runner.calls.length = 0;
  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab2",
    visibleTerminals: ["t-shell"],
    focusedTerminal: "t-shell",
    terminalList: [
      { address: "t-shell", state: "live", labels: { [LABEL_SESSION]: "h1/pi/old-pi" } },
    ],
    panes: [{ pane: "p2", tab: "tab2", terminal: "t-shell", program: { agent: null } }],
    agents: [],
  });
  assert.equal(
    runner.calls.some((c) => c.args[0] === "session" && c.args[1] === "show"),
    false
  );
  assert.equal(coord.tabStreams.get("tab2"), null);
});

test("sidebar_shows_the_stream_id", async () => {
  const s1 = makeSessionJson({ id: "s1", stream_id: "8a9b0c1d-stream" });
  const st = makeStreamJson({ id: "8a9b0c1d-stream", title: "Visible ID", sessions: [s1] });
  const runner = createFakeRunner({
    sessions: { [s1.key]: s1 },
    streams: { [st.key]: st },
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });
  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st.key } },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } }],
    agents: [{ address: "t1", agent: "pi" }],
  });

  const view = tabSidebarView({
    tabId: "tab1",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    frames: coord.frames,
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
  });
  assert.equal(view.kind, "session");
  assert.equal(view.streamIdDisplay, "8a9b0c1d-stream");
  assert.equal(view.stream.id, "8a9b0c1d-stream");
});

test("space_is_found_from_stream_labels", () => {
  const terminals = [
    { address: "t1", labels: { [LABEL_STREAM]: "h1/st-a", [LABEL_HERDR_SPACE]: "space-alpha" } },
    { address: "t2", labels: { [LABEL_STREAM]: "h1/st-b", [LABEL_HERDR_SPACE]: "space-beta" } },
    { address: "remote/t3", labels: { [LABEL_STREAM]: "h1/st-c", [LABEL_HERDR_SPACE]: "remote-sp" } },
  ];
  assert.equal(findSpaceForStream(terminals, "h1/st-b"), "space-beta");
  assert.equal(findSpaceForStream(terminals, "h1/st-c"), null);
  assert.equal(findSpaceForStream(terminals, "h1/missing"), null);
});

test("opening_reuses_a_running_session", async () => {
  const runningSess = makeSessionJson({
    id: "s-run",
    stream_id: "st1",
    status: "active",
    directory: { path: "/work/a", repository: null },
  });
  const stoppedSess = makeSessionJson({
    id: "s-stop",
    stream_id: "st1",
    status: "active",
    directory: { path: "/work/b", repository: null },
  });
  const runner = createFakeRunner({
    sessions: { [runningSess.key]: runningSess, [stoppedSess.key]: stoppedSess },
  });
  let seq = 0;
  const coord = new StreamsCoordinator({ exec: runner.exec, newId: () => `id${++seq}` });
  const windowCommands = [];
  const focusedTerminals = [];

  const result = await coord.openWork(
    {
      stream: "h1/st1",
      terminalList: [
        {
          address: "t-live",
          state: "live",
          labels: {
            [LABEL_SESSION]: runningSess.key,
            [LABEL_STREAM]: "h1/st1",
            [LABEL_HERDR_SPACE]: "space-existing",
          },
        },
      ],
      panes: [{ pane: "p1", terminal: "t-live", program: { agent: "pi" } }],
      agents: [{ address: "t-live", agent: "pi" }],
    },
    {
      run: (cmd, args) => windowCommands.push({ cmd, args }),
      focusTerminal: (term) => focusedTerminals.push(term),
    }
  );

  assert.equal(result.ok, true);
  assert.equal(result.space, "space-existing");
  assert.deepEqual(windowCommands, [{ cmd: "space.select", args: { id: "space-existing" } }]);
  assert.deepEqual(focusedTerminals, ["t-live"]);
  assert.equal(result.reused.length, 1);
  assert.equal(result.launched.length, 1);
  assert.equal(result.launched[0].session, stoppedSess.key);

  const hcRuns = runner.calls.filter((c) => c.bin === "hc");
  assert.equal(hcRuns.length, 1);
  assert.deepEqual(hcRuns[0].args, [
    "run",
    "--bg",
    "--no-parent",
    "--cwd",
    "/work/b",
    "--label",
    "streams.stream=h1/st1",
    "--label",
    "herdr.space=space-existing",
    "--label",
    "herdr.tab=streams-tab-id1",
    "streams",
    "session",
    "resume",
    stoppedSess.key,
  ]);
});

test("opening_keeps_parked_sessions", async () => {
  const active = makeSessionJson({ id: "s-act", stream_id: "st1", status: "active", directory: { path: "/work/a", repository: null } });
  const parked = makeSessionJson({ id: "s-park", stream_id: "st1", status: "parked", directory: { path: "/work/b", repository: null } });
  const done = makeSessionJson({ id: "s-done", stream_id: "st1", status: "done", directory: { path: "/work/c", repository: null } });
  const closed = makeSessionJson({ id: "s-closed", stream_id: "st1", status: "closed", directory: { path: "/work/d", repository: null } });
  const claude = makeSessionJson({ id: "s-claude", harness: "claude", stream_id: "st1", status: "active", directory: { path: "/work/e", repository: null } });
  const noDir = makeSessionJson({ id: "s-nodir", stream_id: "st1", status: "parked", directory: null });

  const runner = createFakeRunner({
    sessions: {
      [active.key]: active,
      [parked.key]: parked,
      [done.key]: done,
      [closed.key]: closed,
      [claude.key]: claude,
      [noDir.key]: noDir,
    },
  });
  let seq = 0;
  const coord = new StreamsCoordinator({ exec: runner.exec, newId: () => `k${++seq}` });

  const result = await coord.openWork({ stream: "h1/st1", terminalList: [], panes: [], agents: [] }, {});
  assert.equal(result.ok, true);
  assert.equal(result.space, "streams-k1");
  assert.deepEqual(
    result.launched.map((item) => item.session),
    [active.key, parked.key]
  );
  assert.deepEqual(result.missingDirectory, [noDir.key]);
  assert.match(coord.lastError, /s-nodir/);
});

test("refresh_relabels_a_moved_session", async () => {
  const moved = makeSessionJson({ id: "s-moved", stream_id: "st-new", title: "Moved Session" });
  const newStream = makeStreamJson({ id: "st-new", title: "New Stream", sessions: [moved] });
  const runner = createFakeRunner({
    sessions: { [moved.key]: moved },
    streams: { [newStream.key]: newStream },
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });

  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      {
        address: "t1",
        state: "live",
        labels: {
          [LABEL_SESSION]: moved.key,
          [LABEL_STREAM]: "h1/st-old",
        },
      },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } }],
    agents: [{ address: "t1", agent: "pi" }],
  });

  const hcLabels = runner.calls.filter((c) => c.bin === "hc");
  assert.deepEqual(hcLabels, [
    { bin: "hc", args: ["label", "t1", "streams.stream=h1/st-new"] },
  ]);
});

test("references cover pr, issue, link, document, and artifact kinds and hide dropped entries", async () => {
  const session = makeSessionJson({
    id: "s-refs",
    stream_id: "st1",
    references: {
      "https://github.com/org/repo/pull/43": {
        kind: "pr",
        name: "PR 43",
        purpose: "Main implementation",
        role: "deliverable",
        status: "active",
        manual: false,
      },
      "https://github.com/org/repo/issues/9": {
        kind: "issue",
        name: "Issue 9",
        purpose: "Blocked on upstream",
        role: "dependency",
        status: "active",
        manual: true,
      },
      "https://example.com/spec": {
        kind: "link",
        name: "RFC Spec",
        purpose: "External specification",
        role: "background",
        status: "active",
        manual: false,
      },
      "docs/plan.md": {
        kind: "document",
        name: "Plan",
        purpose: "Design brief",
        role: "brief",
        status: "active",
        manual: true,
      },
      "https://github.com/org/repo/issues/99": {
        kind: "issue",
        name: "Dropped Issue",
        purpose: "Irrelevant",
        role: "background",
        status: "dropped",
        manual: true,
      },
    },
    artifacts: {
      "assessment.md": { description: "Audit findings" },
    },
    document_files: {
      "docs/plan.md": { status: "available", path: "/work/repo/docs/plan.md" },
    },
  });

  const items = buildReferenceItems(session);
  assert.deepEqual(
    items.map((i) => ({ kind: i.kind, title: i.title })),
    [
      { kind: "pr", title: "PR 43" },
      { kind: "issue", title: "Issue 9" },
      { kind: "document", title: "Plan" },
      { kind: "link", title: "RFC Spec" },
      { kind: "artifact", title: "assessment.md" },
    ]
  );

  // Open each of the 5 reference kinds:
  // 1-3. pr, issue, link -> openUrl
  const runner = createFakeRunner({
    herdrConfig: { naming: { tab: true }, viewer: { mode: "native", placement: "floating" } },
    documentRead: (sess, path) => (sess === session.key && path === "docs/plan.md" ? "# Plan\n" : undefined),
    artifactRead: (key) => (key === `${session.key}/assessment.md` ? "# Findings\n" : undefined),
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });
  const openedUrls = [];
  const overlays = [];
  const windowRuns = [];

  for (const kind of ["pr", "issue", "link"]) {
    const item = items.find((i) => i.kind === kind);
    await coord.openReference(item.reference, {
      openUrl: (u) => openedUrls.push(u),
    });
  }
  assert.deepEqual(openedUrls, [
    "https://github.com/org/repo/pull/43",
    "https://github.com/org/repo/issues/9",
    "https://example.com/spec",
  ]);

  // 4. document in native mode -> `session document read` -> overlay
  const docItem = items.find((i) => i.kind === "document");
  await coord.openReference(docItem.reference, {
    openOverlay: (o) => overlays.push(o),
  });
  assert.equal(overlays.length, 1);
  assert.equal(overlays[0].props.content, "# Plan\n");

  // 4b. document in editor mode -> `session document view` launch command
  await coord.openReference(docItem.reference, {
    mode: "editor",
    placement: "pane-right",
    targetPaneId: "p1",
    run: (cmd, args) => windowRuns.push({ cmd, args }),
  });
  assert.deepEqual(windowRuns.at(-1), {
    cmd: "pane.split",
    args: {
      direction: "right",
      pane: "p1",
      argv: ["streams", "session", "document", "view", session.key, "docs/plan.md"],
    },
  });

  // 5. artifact in native mode -> `artifact read` -> overlay
  const artItem = items.find((i) => i.kind === "artifact");
  await coord.openReference(artItem.reference, {
    openOverlay: (o) => overlays.push(o),
  });
  assert.equal(overlays.length, 2);
  assert.equal(overlays[1].props.content, "# Findings\n");

  // 5b. artifact in terminal mode -> `artifact view` launch command
  await coord.openReference(artItem.reference, {
    mode: "less",
    placement: "tab",
    activeSpaceId: "sp1",
    run: (cmd, args) => windowRuns.push({ cmd, args }),
  });
  assert.deepEqual(windowRuns.at(-1), {
    cmd: "tab.new",
    args: {
      space: "sp1",
      name: "assessment.md",
      argv: ["streams", "artifact", "view", `${session.key}/assessment.md`],
    },
  });
});

test("prepareNativeContent rejects binary content and truncates oversized text", () => {
  assert.deepEqual(prepareNativeContent("hello\0world"), {
    ok: false,
    error: "binary file cannot be opened in native viewer",
  });
  assert.deepEqual(prepareNativeContent("plain text"), {
    ok: true,
    content: "plain text",
    truncated: false,
  });
  const big = "a".repeat(400_050);
  const res = prepareNativeContent(big);
  assert.equal(res.ok, true);
  assert.equal(res.truncated, true);
  assert.match(res.content, /file truncated/);
});

test("resolveTabRenameTarget, effectiveSessionStatus, filterHostStreams, unreadableNotice, and formatRelativeAge", () => {
  const frames = new Map([
    ["t1", { kind: "enrolled", session: { key: "h1/pi/s1", stream: "h1/st1", title: "CLI Work" } }],
  ]);
  assert.deepEqual(
    resolveTabRenameTarget({
      activeTab: { id: "tb1", name: "Old" },
      visibleTerminals: ["t1"],
      frames,
      tabNaming: true,
    }),
    { tabId: "tb1", currentName: "Old", desiredName: "CLI Work" }
  );

  assert.equal(effectiveSessionStatus({ status: "active" }, { terminal: { state: "exited" } }), "offline");
  assert.equal(effectiveSessionStatus({ status: "done" }, { terminal: { state: "exited" } }), "done");

  const filtered = filterHostStreams(
    [
      { id: "a", title: "Alpha", status: "active", last_activity_ms: 100 },
      { id: "b", title: "Beta", status: "parked", last_activity_ms: 200 },
    ],
    { filter: "in_progress", sort: "recent" }
  );
  assert.deepEqual(filtered.map((s) => s.id), ["a"]);

  assert.equal(unreadableNotice([]), null);
  assert.equal(
    unreadableNotice([{ path: "x.json", cause: "bad", repair: "fix" }]).title,
    "1 unreadable record is not shown"
  );
  assert.equal(
    hostStreamsEmptyMessage({ error: null, streams: [], shown: [], unreadable: [] }),
    "No Streams on this host."
  );
  assert.equal(formatRelativeAge(10_000, 15_000), "now");
  assert.equal(
    liveStreamIds({
      terminalList: [
        { address: "t1", state: "live", labels: { [LABEL_SESSION]: "h1/pi/s1", [LABEL_STREAM]: "h1/st1" } },
      ],
      panes: [{ terminal: "t1", program: { agent: "pi" } }],
      agents: [{ address: "t1", agent: "pi" }],
      frames,
    }).has("h1/st1"),
    true
  );

  const dispatches = [];
  const res = dispatchReferencePresentation(
    { kind: "launch_command", placement: "tab", title: "Doc", args: ["artifact", "view", "k"] },
    { activeSpaceId: "sp1", run: (cmd, args) => dispatches.push({ cmd, args }) }
  );
  assert.equal(res.ok, true);
  assert.equal(dispatches[0].cmd, "tab.new");
  assert.equal(activeTabSession({ visibleTerminals: ["t1"], focusedTerminal: "t1", frames }).terminal, "t1");
});
