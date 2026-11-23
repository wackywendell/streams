import test from "node:test";
import assert from "node:assert/strict";
import {
  LABEL_SESSION,
  LABEL_STREAM,
  LABEL_SAVED,
  LABEL_HERDR_SPACE,
  LABEL_HERDR_TAB,
  SHARED_TERMINAL_MARKS_KEY,
  SESSION_STATUSES,
  resolveRole,
  parseTerminalAddress,
  recordHostId,
  qualifyForMachine,
  unqualifyForMachine,
  observeNoticeToken,
  activeTabSession,
  tabSidebarView,
  resolveSpaceStreamKey,
  resolveSidebarView,
  resolveTabRenameTarget,
  effectiveSessionStatus,
  buildReferenceItems,
  prepareNativeContent,
  filterHostStreams,
  groupHostStreams,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
  liveStreamIds,
  countLiveTerminalsByStream,
  findLivePaneForStream,
  findLivePaneForSession,
  statusToTerminalMark,
  buildSharedTerminalMarks,
  groupStreamSessions,
  extractCliError,
  streamBulkActionState,
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
  const exec = async (bin, args, opts = undefined) => {
    calls.push(opts?.on ? { bin, args, on: opts.on } : { bin, args });
    if (handlers.onExec) {
      const custom = await handlers.onExec(bin, args, opts);
      if (custom !== undefined) return custom;
    }
    if (bin === "hc") {
      if (handlers.hc) return handlers.hc(args, opts);
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
    if (args[0] === "session" && args[1] === "edit") {
      if (handlers.sessionEdit) {
        return handlers.sessionEdit(args);
      }
      return { code: 0, stdout: "", stderr: "" };
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

test("observeNoticeToken combines local and remote terminal streams.saved tokens", () => {
  const token = observeNoticeToken(
    [
      { address: "t2", labels: { [LABEL_SAVED]: "u2" } },
      { address: "t1", labels: { [LABEL_SAVED]: "u1" } },
      { address: "remote/t3", labels: { [LABEL_SAVED]: "u3" } },
      { id: "t4", machine: null, labels: { [LABEL_SAVED]: "u4" } },
      { id: "remote/t5", machine: "remote", labels: { [LABEL_SAVED]: "u5" } },
      { id: "t6", machine: "remote", labels: { [LABEL_SAVED]: "u6" } },
    ],
    LABEL_SAVED
  );
  assert.equal(token, "remote/t3:u3|remote/t5:u5|remote/t6:u6|t1:u1|t2:u2|t4:u4");
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
  coord.recordHostMachine("h1", null);

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
  coord.recordHostMachine("h1", null);
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

test("host-wide observation publishes generic terminal.marks and prunes exited terminals", async () => {
  const s1 = makeSessionJson({ id: "s1", stream_id: "st1", title: "Active Tab", status: "active" });
  const s2 = makeSessionJson({ id: "s2", stream_id: "st2", title: "Other Space", status: "waiting" });
  const st1 = makeStreamJson({ id: "st1", title: "Herdr UI", status: "active", sessions: [s1] });
  const st2 = makeStreamJson({ id: "st2", title: "Billing API", status: "waiting", sessions: [s2] });

  const runner = createFakeRunner({
    sessions: { [s1.key]: s1, [s2.key]: s2 },
    streams: { [st1.key]: st1, [st2.key]: st2 },
    streamList: [st1, st2],
  });
  const published = [];
  const coord = new StreamsCoordinator({
    exec: runner.exec,
    publishShared: (key, val) => published.push({ key, val }),
  });

  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } },
      { address: "t2", state: "live", labels: { [LABEL_SESSION]: s2.key, [LABEL_STREAM]: st2.key } },
      { address: "t3", state: "live", labels: { [LABEL_SESSION]: "h1/pi/deleted" } },
    ],
    panes: [
      { pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } },
      { pane: "p2", tab: "tab2", terminal: "t2", program: { agent: "pi" } },
      { pane: "p3", tab: "tab3", terminal: "t3", program: { agent: "pi" } },
    ],
    agents: [
      { address: "t1", agent: "pi" },
      { address: "t2", agent: "pi" },
      { address: "t3", agent: "pi" },
    ],
  });

  assert.equal(published.length, 1);
  assert.equal(published[0].key, SHARED_TERMINAL_MARKS_KEY);
  assert.deepEqual(published[0].val, {
    t1: {
      symbol: "●",
      color: "ansi2",
      attention: true,
      label: "Active",
      source: "streams",
    },
    t2: {
      symbol: "●",
      color: "working",
      attention: true,
      label: "Waiting",
      source: "streams",
    },
  });
  assert.equal(coord.frames.has("t3"), false);

  // When t2 exits externally, next observe prunes it from frames and shared state.
  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } },
      { address: "t2", state: "exited", labels: { [LABEL_SESSION]: s2.key, [LABEL_STREAM]: st2.key } },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } }],
    agents: [{ address: "t1", agent: "pi" }],
  });

  assert.equal(coord.frames.has("t2"), false);
  assert.deepEqual(Object.keys(published.at(-1).val), ["t1"]);
});

test("findLivePaneForStream prefers target Space active Tab and countLiveTerminalsByStream counts open terminals", () => {
  const frames = new Map([
    ["t1", { kind: "enrolled", session: { key: "h1/pi/s1", stream: "h1/st1", status: "done" } }],
    ["t2", { kind: "enrolled", session: { key: "h1/pi/s2", stream: "h1/st1", status: "active" } }],
  ]);
  const spaces = [
    { id: "sp1", tabs: [{ id: "tab-inactive", active: false }, { id: "tab-active", active: true }] },
  ];
  const panes = [
    { pane: "p1", tab: "tab-inactive", terminal: "t1", program: { agent: "pi" } },
    { pane: "p2", tab: "tab-active", terminal: "t2", program: { agent: "pi" } },
  ];
  const terminalList = [
    { address: "t1", state: "live", labels: { [LABEL_SESSION]: "h1/pi/s1", [LABEL_STREAM]: "h1/st1" } },
    { address: "t2", state: "live", labels: { [LABEL_SESSION]: "h1/pi/s2", [LABEL_STREAM]: "h1/st1" } },
  ];
  const agents = [
    { address: "t1", agent: "pi" },
    { address: "t2", agent: "pi" },
  ];

  const counts = countLiveTerminalsByStream({ panes, terminalList, agents, frames });
  assert.equal(counts.get("h1/st1"), 2);

  const preferred = findLivePaneForStream({ streamKey: "h1/st1", spaces, panes, terminalList, agents, frames });
  assert.deepEqual(preferred, { pane: "p2", terminal: "t2", tab: "tab-active" });

  const sessPane = findLivePaneForSession({ sessionKey: "h1/pi/s1", panes, terminalList, agents });
  assert.deepEqual(sessPane, { pane: "p1", terminal: "t1", tab: "tab-inactive" });

  const shared = buildSharedTerminalMarks({ frames, running: new Map([["t1", "h1/pi/s1"]]) });
  assert.deepEqual(shared.t1, statusToTerminalMark("done"));
});

test("All -> Stream and Stream -> Session navigation separates activeView from preview overrides", async () => {
  const s1 = makeSessionJson({ id: "s1", stream_id: "st1", title: "Current Sess" });
  const s2 = makeSessionJson({ id: "s2", stream_id: "st2", title: "Open Other" });
  const s3 = makeSessionJson({
    id: "s3",
    stream_id: "st3",
    title: "Unopened Sess",
    status: "parked",
    recap: "Archived recap",
  });
  const st1 = makeStreamJson({ id: "st1", title: "Current Stream", sessions: [s1] });
  const st2 = makeStreamJson({ id: "st2", title: "Open Stream", sessions: [s2] });
  const st3 = makeStreamJson({ id: "st3", title: "Unopened Stream", status: "parked", sessions: [s3] });

  const runner = createFakeRunner({
    sessions: { [s1.key]: s1, [s2.key]: s2, [s3.key]: s3 },
    streams: { [st1.key]: st1, [st2.key]: st2, [st3.key]: st3 },
    streamList: [st1, st2, st3],
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });
  const spaces = [
    { id: "sp1", machine: null, tabs: [{ id: "tab1", active: true }] },
    { id: "sp2", machine: null, tabs: [{ id: "tab2", active: true }] },
  ];
  const panes = [
    { pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } },
    { pane: "p2", tab: "tab2", terminal: "t2", program: { agent: "pi" } },
  ];
  const terminalList = [
    { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } },
    { address: "t2", state: "live", labels: { [LABEL_SESSION]: s2.key, [LABEL_STREAM]: st2.key } },
  ];
  const agents = [
    { address: "t1", agent: "pi" },
    { address: "t2", agent: "pi" },
  ];

  await coord.observe({
    activeSpace: spaces[0],
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList,
    panes,
    agents,
  });

  // Clicking an unopened Stream from All previews it in Stream view without launching terminals.
  const runCmds = [];
  const resUnopened = await coord.selectStreamFromAll(
    st3.key,
    { spaces, panes, terminalList, agents },
    { run: (cmd, args) => runCmds.push({ cmd, args }) }
  );
  assert.equal(resUnopened.focused, false);
  assert.equal(runCmds.length, 0);
  const streamPreview = resolveSidebarView({
    preferredView: coord.preferredView,
    tabId: "tab1",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    previewSessions: coord.previewSessions,
    frames: coord.frames,
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    previewStreamKey: coord.previewStreamKey,
    previewSessionKey: coord.previewSessionKey,
  });
  assert.equal(streamPreview.view, "stream");
  assert.equal(streamPreview.stream.key, st3.key);
  assert.equal(streamPreview.isPreview, true);
  assert.equal(streamPreview.canReturnToCurrent, true);

  // Clicking an unopened Session inside that Stream previews it read-only in Session view.
  const resSess = await coord.selectSessionFromStream(
    s3.key,
    st3.key,
    { panes, terminalList, agents },
    { run: (cmd, args) => runCmds.push({ cmd, args }) }
  );
  assert.equal(resSess.focused, false);
  assert.equal(runCmds.length, 0);
  const sessPreview = resolveSidebarView({
    preferredView: coord.preferredView,
    tabId: "tab1",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    previewSessions: coord.previewSessions,
    frames: coord.frames,
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    previewStreamKey: coord.previewStreamKey,
    previewSessionKey: coord.previewSessionKey,
  });
  assert.equal(sessPreview.view, "session");
  assert.equal(sessPreview.session.key, s3.key);
  assert.equal(sessPreview.isPreview, true);
  assert.equal(sessPreview.isUnopened, true);
  assert.equal(sessPreview.canReturnToCurrent, true);

  // Clicking an open Stream from All focuses its live pane via pane.focus and stays on Stream view.
  const resOpen = await coord.selectStreamFromAll(
    st2.key,
    { spaces, panes, terminalList, agents },
    { run: (cmd, args) => runCmds.push({ cmd, args }) }
  );
  assert.equal(resOpen.focused, true);
  assert.deepEqual(runCmds, [{ cmd: "pane.focus", args: { pane: "p2" } }]);
  await coord.observe({
    activeSpace: spaces[1],
    activeTabId: "tab2",
    visibleTerminals: ["t2"],
    focusedTerminal: "t2",
    terminalList,
    panes,
    agents,
  });
  assert.equal(coord.preferredView, "stream");
  assert.equal(coord.previewStreamKey, null);

  // An external Space/Tab change clears detached preview while keeping preferredView sticky.
  await coord.inspectStream(st3.key);
  assert.equal(coord.previewStreamKey, st3.key);
  await coord.observe({
    activeSpace: spaces[0],
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList,
    panes,
    agents,
  });
  assert.equal(coord.previewStreamKey, null);
  assert.equal(coord.previewSessionKey, null);
  assert.equal(coord.preferredView, "stream");
});

test("sticky preferredView falls back Session -> Space Stream -> All and restores automatically", async () => {
  const sA = makeSessionJson({ id: "sA", stream_id: "stA", title: "Session A" });
  const sB = makeSessionJson({ id: "sB", stream_id: "stB", title: "Session B" });
  const stA = makeStreamJson({ id: "stA", title: "Stream A", sessions: [sA] });
  const stB = makeStreamJson({ id: "stB", title: "Stream B", sessions: [sB] });

  const runner = createFakeRunner({
    sessions: { [sA.key]: sA, [sB.key]: sB },
    streams: { [stA.key]: stA, [stB.key]: stB },
    streamList: [stA, stB],
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });

  // Space 1 holds two enrolled tabs (Stream A and Stream B) plus a new unenrolled tab (tabNew).
  // Space 2 is a brand-new empty workspace with only an unenrolled tab (tabEmpty).
  const spaces = [
    {
      id: "sp1",
      machine: null,
      tabs: [
        { id: "tabA", active: false },
        { id: "tabB", active: true },
        { id: "tabNew", active: false },
      ],
    },
    { id: "sp2", machine: null, tabs: [{ id: "tabEmpty", active: true }] },
  ];
  const panes = [
    { pane: "pA", tab: "tabA", terminal: "tA", program: { agent: "pi" } },
    { pane: "pB", tab: "tabB", terminal: "tB", program: { agent: "pi" } },
    { pane: "pNew", tab: "tabNew", terminal: "tNew", program: { shell: "zsh" } },
    { pane: "pEmpty", tab: "tabEmpty", terminal: "tEmpty", program: { shell: "zsh" } },
  ];
  const terminalList = [
    { address: "tA", state: "live", labels: { [LABEL_SESSION]: sA.key, [LABEL_STREAM]: stA.key } },
    { address: "tB", state: "live", labels: { [LABEL_SESSION]: sB.key, [LABEL_STREAM]: stB.key } },
    { address: "tNew", state: "live", labels: {} },
    { address: "tEmpty", state: "live", labels: {} },
  ];
  const agents = [
    { address: "tA", agent: "pi" },
    { address: "tB", agent: "pi" },
  ];

  // Focus tabB (Stream B) with default preferredView === "session".
  await coord.observe({
    activeSpace: spaces[0],
    activeTabId: "tabB",
    visibleTerminals: ["tB"],
    focusedTerminal: "tB",
    terminalList,
    panes,
    agents,
  });
  assert.equal(coord.preferredView, "session");
  assert.equal(coord.lastFocusedStreamBySpace.get("sp1"), stB.key);

  // Open new unenrolled tab in sp1: falls back to last-focused Space Stream (Stream B), keeping preferredView === "session".
  await coord.observe({
    activeSpace: spaces[0],
    activeTabId: "tabNew",
    visibleTerminals: ["tNew"],
    focusedTerminal: "tNew",
    terminalList,
    panes,
    agents,
  });
  const sp1StreamKey = resolveSpaceStreamKey({
    space: spaces[0],
    panes,
    terminalList,
    agents,
    frames: coord.frames,
    lastFocusedStreamBySpace: coord.lastFocusedStreamBySpace,
  });
  assert.equal(sp1StreamKey, stB.key);
  const onNewTab = resolveSidebarView({
    preferredView: coord.preferredView,
    tabId: "tabNew",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    previewSessions: coord.previewSessions,
    frames: coord.frames,
    visibleTerminals: ["tNew"],
    focusedTerminal: "tNew",
    spaceStreamKey: sp1StreamKey,
  });
  assert.equal(onNewTab.view, "stream");
  assert.equal(onNewTab.stream.key, stB.key);
  assert.equal(onNewTab.isPreview, false);
  assert.equal(onNewTab.hasSessionAccess, false);
  assert.equal(onNewTab.hasStreamAccess, true);
  assert.equal(coord.preferredView, "session");

  // Switch back to tabA: snaps back to Session automatically because preferredView is still "session".
  await coord.observe({
    activeSpace: spaces[0],
    activeTabId: "tabA",
    visibleTerminals: ["tA"],
    focusedTerminal: "tA",
    terminalList,
    panes,
    agents,
  });
  const backOnTabA = resolveSidebarView({
    preferredView: coord.preferredView,
    tabId: "tabA",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    previewSessions: coord.previewSessions,
    frames: coord.frames,
    visibleTerminals: ["tA"],
    focusedTerminal: "tA",
    spaceStreamKey: resolveSpaceStreamKey({
      space: spaces[0],
      panes,
      terminalList,
      agents,
      frames: coord.frames,
      lastFocusedStreamBySpace: coord.lastFocusedStreamBySpace,
    }),
  });
  assert.equal(backOnTabA.view, "session");
  assert.equal(backOnTabA.session.key, sA.key);

  // Switch to brand-new empty Space sp2: both Session and Stream are inaccessible -> falls back to All.
  await coord.observe({
    activeSpace: spaces[1],
    activeTabId: "tabEmpty",
    visibleTerminals: ["tEmpty"],
    focusedTerminal: "tEmpty",
    terminalList,
    panes,
    agents,
  });
  const sp2StreamKey = resolveSpaceStreamKey({
    space: spaces[1],
    panes,
    terminalList,
    agents,
    frames: coord.frames,
    lastFocusedStreamBySpace: coord.lastFocusedStreamBySpace,
  });
  assert.equal(sp2StreamKey, null);
  const onEmptySpace = resolveSidebarView({
    preferredView: coord.preferredView,
    tabId: "tabEmpty",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    previewSessions: coord.previewSessions,
    frames: coord.frames,
    visibleTerminals: ["tEmpty"],
    focusedTerminal: "tEmpty",
    spaceStreamKey: sp2StreamKey,
  });
  assert.equal(onEmptySpace.view, "all");
  assert.equal(onEmptySpace.hasSessionAccess, false);
  assert.equal(onEmptySpace.hasStreamAccess, false);
  assert.equal(coord.preferredView, "session");

  // Manually switching to Stream or All stays put across tab switches.
  await coord.setView("all");
  await coord.observe({
    activeSpace: spaces[0],
    activeTabId: "tabA",
    visibleTerminals: ["tA"],
    focusedTerminal: "tA",
    terminalList,
    panes,
    agents,
  });
  const stayAll = resolveSidebarView({
    preferredView: coord.preferredView,
    tabId: "tabA",
    tabStreams: coord.tabStreams,
    streams: coord.streams,
    previewSessions: coord.previewSessions,
    frames: coord.frames,
    visibleTerminals: ["tA"],
    focusedTerminal: "tA",
  });
  assert.equal(stayAll.view, "all");
  assert.equal(stayAll.hasSessionAccess, true);
  assert.equal(stayAll.hasStreamAccess, true);
});

test("dropReference, setSessionStatus, convention-gated bulk actions, and only_active openWork", async () => {
  const sAct = makeSessionJson({ id: "s-act", stream_id: "st1", status: "active", directory: { path: "/w/1" } });
  const sWait = makeSessionJson({ id: "s-wait", stream_id: "st1", status: "waiting", directory: { path: "/w/2" } });
  const sPark = makeSessionJson({ id: "s-park", stream_id: "st1", status: "parked", directory: { path: "/w/3" } });
  const sDone = makeSessionJson({ id: "s-done", stream_id: "st1", status: "done", directory: { path: "/w/4" } });
  const st = makeStreamJson({
    id: "st1",
    title: "Multi",
    status: "active",
    sessions: [sAct, sWait, sPark, sDone],
  });

  const bulk = streamBulkActionState(st);
  assert.equal(bulk.canOpenActive, true);
  assert.equal(bulk.canParkAll, true);
  assert.equal(bulk.canCloseAll, true);
  assert.deepEqual(bulk.parkableSessions.map((s) => s.id), ["s-act", "s-wait"]);
  assert.deepEqual(bulk.closableSessions.map((s) => s.id), ["s-act", "s-wait", "s-park"]);

  const doneOnlyBulk = streamBulkActionState({ sessions: [sDone] });
  assert.equal(doneOnlyBulk.canOpenActive, false);
  assert.equal(doneOnlyBulk.canParkAll, false);
  assert.equal(doneOnlyBulk.canCloseAll, false);

  const editCalls = [];
  const runner = createFakeRunner({
    sessions: { [sAct.key]: sAct, [sWait.key]: sWait, [sPark.key]: sPark, [sDone.key]: sDone },
    streams: { [st.key]: st },
    sessionEdit: (args) => {
      editCalls.push(args);
      if (args.includes(sWait.key) && args.includes("closed")) {
        return { code: 1, stdout: JSON.stringify({ error: { message: "disk full" } }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  const coord = new StreamsCoordinator({ exec: runner.exec, newId: () => "id1" });
  coord.recordHostMachine("h1", null);
  coord.streams.set(st.key, st);

  // Drop reference
  const dropRes = await coord.dropReference(sAct.key, "docs/old.md");
  assert.equal(dropRes.ok, true);
  assert.deepEqual(editCalls.at(-1), ["session", "edit", sAct.key, "--drop-reference", "docs/old.md"]);

  // Set session status
  assert.deepEqual(SESSION_STATUSES, ["active", "waiting", "blocked", "parked", "done", "closed"]);
  const statRes = await coord.setSessionStatus(sAct.key, "blocked");
  assert.equal(statRes.ok, true);
  assert.deepEqual(editCalls.at(-1), ["session", "edit", sAct.key, "--status", "blocked"]);

  // Park all updates only active/waiting/blocked sessions
  const parkRes = await coord.parkAllSessions(st.key);
  assert.equal(parkRes.ok, true);
  assert.deepEqual(parkRes.updated, [sAct.key, sWait.key]);

  // Close all reports partial failure cleanly naming the failing Session
  const closeRes = await coord.closeAllSessions(st.key);
  assert.equal(closeRes.ok, false);
  assert.deepEqual(closeRes.updated, [sAct.key]);
  assert.equal(closeRes.failed, sWait.key);
  assert.match(coord.lastError, /disk full/);

  // Open all active sessions launches only `active` Pi Sessions (never `parked`)
  const openRes = await coord.openWork(
    { stream: st.key, sessions: { kind: "only_active" }, terminalList: [], panes: [], agents: [] },
    {}
  );
  assert.equal(openRes.ok, true);
  assert.deepEqual(openRes.launched.map((l) => l.session), [sAct.key]);

  // Empty Stream enables delete; non-empty disables it
  assert.equal(streamBulkActionState(coord.streams.get(st.key)).canDelete, false);
  assert.equal(streamBulkActionState({ key: "h1/empty", sessions: [] }).canDelete, true);

  const emptyCoord = new StreamsCoordinator({
    exec: async (_cmd, args) => {
      if (args[0] === "stream" && args[1] === "delete") {
        return { code: 0, stdout: JSON.stringify({ key: args[2], deleted: true }), stderr: "" };
      }
      return { code: 0, stdout: "{}", stderr: "" };
    },
  });
  emptyCoord.recordHostMachine("h1", null);
  emptyCoord.streams.set("h1/empty", { key: "h1/empty", id: "empty", title: "Empty", sessions: [] });
  emptyCoord.hostStreams = [{ key: "h1/empty", id: "empty", title: "Empty", status: "active" }];
  emptyCoord.previewStreamKey = "h1/empty";
  emptyCoord.preferredView = "stream";
  const delRes = await emptyCoord.deleteStream("h1/empty");
  assert.equal(delRes.ok, true);
  assert.equal(emptyCoord.streams.has("h1/empty"), false);
  assert.equal(emptyCoord.hostStreams.length, 0);
  assert.equal(emptyCoord.previewStreamKey, null);
  assert.equal(emptyCoord.preferredView, "all");
});

test("runningSessionTerminals matches real herdr-app AgentInfo and PaneInfo Program shapes", async () => {
  const s1 = makeSessionJson({ id: "s-real", stream_id: "st1", title: "Live Herdr Pi" });
  const st = makeStreamJson({
    id: "st1",
    title: "Real Stream",
    sessions: [{ key: s1.key, harness: "pi", title: s1.title, status: "active", state: s1.state }],
  });
  const runner = createFakeRunner({
    sessions: { [s1.key]: s1 },
    streams: { [st.key]: st },
  });
  const published = [];
  const coord = new StreamsCoordinator({
    exec: runner.exec,
    publishShared: (k, v) => published.push({ k, v }),
  });

  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t-pi", "t-claude", "t-zsh"],
    focusedTerminal: "t-pi",
    terminalList: [
      { address: "t-pi", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st.key } },
      { address: "t-claude", state: "live", labels: { [LABEL_SESSION]: "h1/pi/old-pi" } },
      { address: "t-zsh", state: "live", labels: { [LABEL_SESSION]: "h1/pi/old-shell" } },
    ],
    panes: [
      { pane: "p1", tab: "tab1", terminal: "t-pi", app: "pi", program: { key: "pi", name: "Pi", agent: true, shell: false } },
      { pane: "p2", tab: "tab1", terminal: "t-claude", app: "claude-code", program: { key: "claude", name: "Claude Code", agent: true, shell: false } },
      { pane: "p3", tab: "tab1", terminal: "t-zsh", app: null, program: { key: "zsh", name: "zsh", agent: false, shell: true } },
    ],
    agents: [
      { address: "t-pi", pane: "p1", app: "pi", program: { key: "pi", name: "Pi", agent: true, shell: false } },
      { address: "t-claude", pane: "p2", app: "claude-code", program: { key: "claude", name: "Claude Code", agent: true, shell: false } },
    ],
  });

  assert.equal(coord.frames.get("t-pi")?.kind, "enrolled");
  assert.equal(coord.frames.has("t-claude"), false);
  assert.equal(coord.frames.has("t-zsh"), false);
  assert.equal(coord.streams.get(st.key)?.sessions[0]?.stream, st.key);
  assert.deepEqual(published.at(-1)?.v?.["t-pi"], statusToTerminalMark("active"));
});

test("actionError parses hc stderr JSON, formats friendly missing-folder message, and survives refreshes", async () => {
  const parsedStderr = extractCliError(
    {
      code: 1,
      stdout: "",
      stderr: JSON.stringify({
        error: {
          kind: "spawn_failed",
          message: 'cannot start "streams": No such file or directory (os error 2)',
          hint: "check the program name and cwd",
        },
      }),
    },
    "fallback"
  );
  assert.equal(parsedStderr.kind, "spawn_failed");
  assert.equal(parsedStderr.hint, "check the program name and cwd");

  const sLive = makeSessionJson({ id: "s-live", stream_id: "st1", title: "Live Session" });
  const sGone = makeSessionJson({
    id: "s-gone",
    stream_id: "st1",
    title: "Session Refs",
    status: "waiting",
    directory: {
      path: "/Users/alice/.herdr/worktrees/streams/session-references",
      repository: { url: "git@github.com:org/streams.git", branch: "session-references" },
    },
  });
  const st = makeStreamJson({ id: "st1", title: "Streams", sessions: [sLive, sGone] });

  const baseRunner = createFakeRunner({
    sessions: { [sLive.key]: sLive, [sGone.key]: sGone },
    streams: { [st.key]: st },
    streamList: [st],
  });
  const toasts = [];
  const runCmds = [];
  const coord = new StreamsCoordinator({
    homeDir: "/Users/alice",
    exec: async (cmd, args) => {
      if (cmd === "hc" && args[0] === "run") {
        return {
          code: 1,
          stdout: "",
          stderr: JSON.stringify({
            error: {
              kind: "spawn_failed",
              message: 'cannot start "streams": No such file or directory (os error 2)',
              hint: "check the program name and cwd",
            },
          }),
        };
      }
      if (cmd === "test" && args[0] === "-d") {
        return { code: 1, stdout: "", stderr: "" };
      }
      return baseRunner.exec(cmd, args);
    },
    notifyToast: (n) => toasts.push(n),
  });

  const terminalList = [
    {
      address: "t1",
      state: "live",
      labels: { [LABEL_SESSION]: sLive.key, [LABEL_STREAM]: st.key, [LABEL_HERDR_SPACE]: "sp1" },
    },
  ];
  const panes = [{ pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } }];
  const agents = [{ address: "t1", program: { key: "pi", agent: true } }];

  const openRes = await coord.openWork(
    {
      stream: st.key,
      sessions: { kind: "selected", session_ids: [sGone.key] },
      terminalList,
      panes,
      agents,
    },
    { run: (c, a) => runCmds.push({ c, a }) }
  );

  assert.equal(openRes.ok, false);
  assert.equal(runCmds.length, 0); // did not switch Space on failed launch
  assert.equal(coord.actionError?.title, 'Folder missing for "Session Refs"');
  assert.match(coord.actionError?.message ?? "", /~\/\.herdr\/worktrees\/streams\/session-references no longer exists/);
  assert.match(coord.actionError?.message ?? "", /git@github\.com:org\/streams\.git/);
  assert.doesNotMatch(coord.actionError?.message ?? "", /"spawn_failed"/);
  assert.equal(coord.actionError?.copyLabel, "Copy command");
  assert.equal(coord.actionError?.copyText, `streams session edit ${sGone.key} --directory <path>`);
  assert.equal(toasts.length, 1);
  assert.equal(toasts[0].level, "error");
  assert.equal(toasts[0].title, 'Folder missing for "Session Refs"');

  // A subsequent successful background observe MUST NOT wipe actionError!
  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList,
    panes,
    agents,
    force: true,
  });
  assert.equal(coord.readError, null);
  assert.equal(coord.actionError?.title, 'Folder missing for "Session Refs"');

  // Explicit dismissal clears actionError.
  coord.clearActionError();
  assert.equal(coord.actionError, null);
  assert.equal(coord.lastError, null);
});

test("groupStreamSessions partitions by openness first and supports recent and alphabetical sorting", () => {
  const sessions = [
    { key: "k-done-open", status: "done", title: "Zeta Done Open" },
    { key: "k-act-open", status: "active", title: "Alpha Active Open" },
    { key: "k-park", status: "parked", title: "Zebra Parked" },
    { key: "k-blk", status: "blocked", title: "Beta Blocked" },
    { key: "k-done-1", status: "done", title: "Gamma Done" },
    { key: "k-closed", status: "closed", title: "Delta Closed" },
  ];

  // Default: recent (preserves CLI recency order within each group)
  const recentGroups = groupStreamSessions(sessions, {
    liveSessionKeys: new Set(["k-act-open", "k-done-open"]),
  });
  assert.deepEqual(
    recentGroups.map((g) => ({ id: g.id, defaultOpen: g.defaultOpen, keys: g.sessions.map((s) => s.key) })),
    [
      { id: "open", defaultOpen: true, keys: ["k-done-open", "k-act-open"] },
      { id: "not_open", defaultOpen: true, keys: ["k-park", "k-blk"] },
      { id: "done_closed", defaultOpen: false, keys: ["k-done-1", "k-closed"] },
    ]
  );

  // Alphabetical sorting within each group
  const alphaGroups = groupStreamSessions(sessions, {
    liveSessionKeys: new Set(["k-act-open", "k-done-open"]),
    sort: "alpha",
  });
  assert.deepEqual(
    alphaGroups.map((g) => ({ id: g.id, keys: g.sessions.map((s) => s.key) })),
    [
      { id: "open", keys: ["k-act-open", "k-done-open"] },
      { id: "not_open", keys: ["k-blk", "k-park"] },
      { id: "done_closed", keys: ["k-closed", "k-done-1"] },
    ]
  );
});

test("groupHostStreams partitions Open / Not open / Done & closed with recent and A–Z sorting", () => {
  const streams = [
    { key: "h1/s1", id: "s1", title: "Zebra Done Open", status: "done", last_activity_ms: 500 },
    { key: "h1/s2", id: "s2", title: "Alpha Active Open", status: "active", last_activity_ms: 900 },
    { key: "h1/s3", id: "s3", title: "Omega Parked", status: "parked", last_activity_ms: 800 },
    { key: "h1/s4", id: "s4", title: "Beta Waiting", status: "waiting", last_activity_ms: 300 },
    { key: "h1/s5", id: "s5", title: "Gamma Done", status: "done", last_activity_ms: 700 },
    { key: "h1/s6", id: "s6", title: "Delta Closed", status: "closed", last_activity_ms: 950 },
  ];
  const openCounts = new Map([
    ["h1/s1", 1],
    ["h1/s2", 2],
  ]);

  const recent = groupHostStreams(streams, { openCounts, sort: "recent" });
  assert.deepEqual(
    recent.map((g) => ({ id: g.id, defaultOpen: g.defaultOpen, ids: g.streams.map((s) => s.id) })),
    [
      { id: "open", defaultOpen: true, ids: ["s2", "s1"] },
      { id: "not_open", defaultOpen: true, ids: ["s3", "s4"] },
      { id: "done_closed", defaultOpen: false, ids: ["s6", "s5"] },
    ]
  );

  const alpha = groupHostStreams(streams, { openCounts, sort: "alpha" });
  assert.deepEqual(
    alpha.map((g) => ({ id: g.id, ids: g.streams.map((s) => s.id) })),
    [
      { id: "open", ids: ["s2", "s1"] },
      { id: "not_open", ids: ["s4", "s3"] },
      { id: "done_closed", ids: ["s6", "s5"] },
    ]
  );

  const searched = groupHostStreams(streams, { openCounts, query: "gamma" });
  assert.deepEqual(
    searched.filter((g) => g.streams.length > 0).map((g) => ({ id: g.id, ids: g.streams.map((s) => s.id) })),
    [{ id: "done_closed", ids: ["s5"] }]
  );
});

test("setSessionStatus performs targeted authoritative refresh across frames, streams, hostStreams, and terminal.marks", async () => {
  const s1 = makeSessionJson({ id: "s1", stream_id: "st1", title: "Herdr UI", status: "active" });
  const st1 = makeStreamJson({ id: "st1", title: "Streams", status: "active", sessions: [s1] });

  const calls = [];
  const published = [];
  let notifyCount = 0;
  const coord = new StreamsCoordinator({
    exec: async (cmd, args) => {
      calls.push([cmd, ...args]);
      if (cmd === "streams" && args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (cmd === "streams" && args[0] === "session" && args[1] === "edit") {
        const nextStatus = args[args.indexOf("--status") + 1];
        s1.status = nextStatus;
        st1.status = nextStatus;
        st1.active_sessions = nextStatus === "active" ? 1 : 0;
        st1.sessions[0].status = nextStatus;
        return { code: 0, stdout: "", stderr: "" };
      }
      if (cmd === "streams" && args[0] === "session" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(s1), stderr: "" };
      }
      if (cmd === "streams" && args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(st1), stderr: "" };
      }
      if (cmd === "streams" && args[0] === "stream" && args[1] === "list") {
        return { code: 0, stdout: JSON.stringify({ items: [st1], unreadable: [] }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
    publishShared: (k, v) => published.push({ k, v }),
    notify: () => {
      notifyCount += 1;
    },
  });

  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } }],
    agents: [{ address: "t1", program: { key: "pi", agent: true } }],
  });
  await coord.ensureHostStreams(true);
  calls.length = 0;
  notifyCount = 0;

  // Mutate status directly (without calling observe() afterwards!)
  const res = await coord.setSessionStatus(s1.key, "waiting", st1.key);
  assert.equal(res.ok, true);

  // Only 3 targeted CLI calls: session edit, session show, stream show (no stream list / session list scan!)
  assert.deepEqual(
    calls.map((c) => `${c[1]} ${c[2]}`),
    ["session edit", "session show", "stream show"]
  );
  // All caches and shared terminal.marks updated immediately in one tick:
  assert.equal(coord.frames.get("t1")?.session?.status, "waiting");
  assert.equal(coord.streams.get(st1.key)?.status, "waiting");
  assert.equal(coord.streams.get(st1.key)?.sessions[0]?.status, "waiting");
  assert.equal(coord.hostStreams[0]?.status, "waiting");
  assert.deepEqual(published.at(-1)?.v?.t1, statusToTerminalMark("waiting"));
  assert.ok(notifyCount > 0);
});

test("parseTerminalAddress, qualify/unqualify, recordHostId, and machine-scoped findSpaceForStream", () => {
  assert.deepEqual(parseTerminalAddress("t1"), { address: "t1", machine: null, localId: "t1" });
  assert.deepEqual(parseTerminalAddress("retriever/t8p"), {
    address: "retriever/t8p",
    machine: "retriever",
    localId: "t8p",
  });
  assert.deepEqual(parseTerminalAddress({ id: "t8p", machine: "retriever" }), {
    address: "retriever/t8p",
    machine: "retriever",
    localId: "t8p",
  });
  assert.equal(recordHostId("host-rem/pi/s1"), "host-rem");
  assert.equal(recordHostId("host-rem/st-1"), "host-rem");
  assert.equal(qualifyForMachine("sp1", null), "sp1");
  assert.equal(qualifyForMachine("sp1", "retriever"), "retriever/sp1");
  assert.equal(qualifyForMachine("retriever/sp1", "retriever"), "retriever/sp1");
  assert.equal(unqualifyForMachine("retriever/sp1", "retriever"), "sp1");
  assert.equal(unqualifyForMachine("sp1", "retriever"), "sp1");

  const terms = [
    {
      address: "t1",
      labels: { [LABEL_STREAM]: "h-local/st-1", [LABEL_HERDR_SPACE]: "sp-local" },
    },
    {
      address: "retriever/t8p",
      machine: "retriever",
      labels: { [LABEL_STREAM]: "h-rem/st-9", [LABEL_HERDR_SPACE]: "sp-rem" },
    },
  ];
  assert.equal(findSpaceForStream(terms, "h-local/st-1", null), "sp-local");
  assert.equal(findSpaceForStream(terms, "h-rem/st-9", null), null);
  assert.equal(findSpaceForStream(terms, "h-rem/st-9", "retriever"), "sp-rem");
});

test("remote space observation routes CLI calls with { on }, keeps config local, and uses portable remote bin", async () => {
  const sRem = makeSessionJson({
    host: "h-rem",
    id: "s-rem",
    stream_id: "st-rem",
    title: "Remote Worker",
    status: "active",
  });
  const stRem = makeStreamJson({
    host: "h-rem",
    id: "st-rem",
    title: "Remote Stream",
    sessions: [sRem],
  });
  const published = [];
  const runner = createFakeRunner({
    sessions: { [sRem.key]: sRem },
    streams: { [stRem.key]: stRem },
    onExec: (bin, args, opts) => {
      if (bin === "sh") {
        assert.equal(opts?.on, "retriever");
        return { code: 0, stdout: "/home/bits", stderr: "" };
      }
      return undefined;
    },
  });

  const coord = new StreamsCoordinator({
    exec: runner.exec,
    streamsBin: "/Users/wendell/.local/bin/streams",
    publishShared: (k, v) => published.push({ k, v }),
  });

  const res = await coord.observe({
    activeSpace: { id: "retriever/sp1", machine: "retriever" },
    activeTabId: "retriever/tab1",
    visibleTerminals: ["retriever/t8p"],
    focusedTerminal: "retriever/t8p",
    terminalList: [
      {
        address: "retriever/t8p",
        machine: "retriever",
        state: "live",
        labels: { [LABEL_SESSION]: sRem.key, [LABEL_SAVED]: "rev-1" },
      },
    ],
    panes: [
      {
        pane: "retriever/p1",
        tab: "retriever/tab1",
        terminal: "retriever/t8p",
        program: { key: "pi", agent: true },
      },
    ],
    agents: [{ address: "retriever/t8p", program: { key: "pi", agent: true } }],
    machineList: [
      { name: null, layout: "ready" },
      { name: "retriever", layout: "ready" },
    ],
  });

  assert.equal(res?.kind, "stream");
  assert.equal(coord.frames.get("retriever/t8p")?.session?.key, sRem.key);
  assert.equal(coord.tabStreams.get("retriever/tab1"), stRem.key);
  assert.equal(coord.machineForHost("h-rem"), "retriever");
  assert.deepEqual(published.at(-1)?.v?.["retriever/t8p"], statusToTerminalMark("active"));

  // Config is read once locally with Mac path; remote CLI calls use bare "streams" with { on: "retriever" }.
  const cfgCall = runner.calls.find((c) => c.args[0] === "config");
  assert.equal(cfgCall?.on ?? null, null);
  assert.equal(cfgCall?.bin, "/Users/wendell/.local/bin/streams");

  const showCall = runner.calls.find((c) => c.args[0] === "session" && c.args[1] === "show");
  assert.equal(showCall?.on, "retriever");
  assert.equal(showCall?.bin, "streams");

  // Label reconciliation uses local hc with full <machine>/<id> address.
  const hcLabel = runner.calls.find((c) => c.bin === "hc" && c.args[0] === "label");
  assert.equal(hcLabel?.on ?? null, null);
  assert.deepEqual(hcLabel?.args, ["label", "retriever/t8p", `${LABEL_STREAM}=${stRem.key}`]);

  // Remote $HOME was lazily fetched and cached.
  assert.equal(await coord.ensureHomeDir("retriever"), "/home/bits");
  assert.equal(coord.homeDirFor("retriever"), "/home/bits");
});

test("cross-machine terminal.marks skip offline machines and survive active remote read rejection", async () => {
  const sLocal = makeSessionJson({
    host: "h-loc",
    id: "s-loc",
    stream_id: "st-loc",
    title: "Local Agent",
    status: "waiting",
  });
  const sRem = makeSessionJson({
    host: "h-rem",
    id: "s-rem",
    stream_id: "st-rem",
    title: "Remote Agent",
    status: "active",
  });
  const published = [];
  const calls = [];
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      calls.push({ bin, args, on });
      if (args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (on === "retriever") {
        throw new Error("ssh transport reset");
      }
      if (on === "offline-box") {
        throw new Error("should not call offline machine");
      }
      if (on === null && args[0] === "session" && args[1] === "list") {
        return { code: 0, stdout: JSON.stringify({ items: [sLocal] }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
    publishShared: (k, v) => published.push({ k, v }),
  });

  await coord.observe({
    activeSpace: { id: "retriever/sp1", machine: "retriever" },
    activeTabId: "retriever/tab1",
    visibleTerminals: ["retriever/t8p"],
    focusedTerminal: "retriever/t8p",
    terminalList: [
      {
        address: "retriever/t8p",
        machine: "retriever",
        state: "live",
        labels: { [LABEL_SESSION]: sRem.key },
      },
      {
        address: "t1",
        state: "live",
        labels: { [LABEL_SESSION]: sLocal.key },
      },
      {
        address: "offline-box/t9",
        machine: "offline-box",
        state: "live",
        labels: { [LABEL_SESSION]: "h-off/pi/s-off" },
      },
    ],
    panes: [
      { pane: "retriever/p1", tab: "retriever/tab1", terminal: "retriever/t8p", program: { key: "pi", agent: true } },
      { pane: "p1", tab: "tab-loc", terminal: "t1", program: { key: "pi", agent: true } },
      { pane: "offline-box/p9", tab: "offline-box/tab9", terminal: "offline-box/t9", program: { key: "pi", agent: true } },
    ],
    agents: [
      { address: "retriever/t8p", program: { key: "pi", agent: true } },
      { address: "t1", program: { key: "pi", agent: true } },
      { address: "offline-box/t9", program: { key: "pi", agent: true } },
    ],
    machineList: [
      { name: null, layout: "ready" },
      { name: "retriever", layout: "ready" },
      { name: "offline-box", layout: "unavailable" },
    ],
  });

  // Active remote failure surfaced in readError without throwing...
  assert.match(coord.readError ?? "", /ssh transport reset/);
  // ...while offline-box was never called...
  assert.equal(calls.some((c) => c.on === "offline-box"), false);
  // ...and the healthy local agent STILL acquired its frame and terminal.marks entry!
  assert.equal(coord.frames.get("t1")?.session?.key, sLocal.key);
  assert.deepEqual(published.at(-1)?.v?.t1, statusToTerminalMark("waiting"));
});

test("hostScope toggle, per-machine cache isolation, and out-of-order deferred responses do not cross-contaminate", async () => {
  const stLocalOnly = makeStreamJson({ host: "h-loc", id: "st-loc", title: "Local Only" });
  const stRemOnly = makeStreamJson({ host: "h-rem", id: "st-rem", title: "Remote Only" });

  let resolveLocalList;
  const localListDeferred = new Promise((r) => {
    resolveLocalList = r;
  });

  const listCalls = [];
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      if (args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "list") {
        const isLocalFlag = args.includes("--local");
        listCalls.push({ on, isLocalFlag });
        if (on === null && isLocalFlag) {
          return localListDeferred;
        }
        if (on === "retriever" && isLocalFlag) {
          return {
            code: 0,
            stdout: JSON.stringify({ items: [stRemOnly], unreadable: [] }),
            stderr: "",
          };
        }
        if (on === "retriever" && !isLocalFlag) {
          return {
            code: 0,
            stdout: JSON.stringify({ items: [stRemOnly, stLocalOnly], unreadable: [] }),
            stderr: "",
          };
        }
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });

  // 1. Start slow local/This-host listing.
  coord.activeMachine = null;
  coord.hostScope = "local";
  const pendingLocal = coord.ensureHostStreams(true);

  // 2. Before local resolves, switch to retriever + All hosts and fetch immediately.
  coord.activeMachine = "retriever";
  await coord.setHostScope("all");
  assert.deepEqual(
    coord.hostStreams.map((s) => s.key),
    [stRemOnly.key, stLocalOnly.key]
  );

  // 3. Now resolve the delayed local/This-host response — it must NOT overwrite retriever/All-hosts!
  resolveLocalList({
    code: 0,
    stdout: JSON.stringify({ items: [stLocalOnly], unreadable: [] }),
    stderr: "",
  });
  await pendingLocal;

  assert.equal(coord.activeMachine, "retriever");
  assert.equal(coord.hostScope, "all");
  assert.deepEqual(
    coord.hostStreams.map((s) => s.key),
    [stRemOnly.key, stLocalOnly.key]
  );

  // 4. Switching back to local/This-host sees the cached local bucket immediately.
  coord.activeMachine = null;
  coord.hostScope = "local";
  assert.deepEqual(
    coord.hostStreams.map((s) => s.key),
    [stLocalOnly.key]
  );
  assert.equal(coord.machineForHost("h-loc"), null);

  // 5. Verify --local flag was passed for local scope and omitted for all scope.
  assert.deepEqual(listCalls, [
    { on: null, isLocalFlag: true },
    { on: "retriever", isLocalFlag: false },
  ]);
});

test("initial inspectStream and previewSession route to remote owner for unsynced remote-only records", async () => {
  const sRem = makeSessionJson({
    host: "h-rem",
    id: "s-rem-new",
    stream_id: "st-rem-new",
    title: "Fresh Remote Session",
  });
  const stRem = makeStreamJson({
    host: "h-rem",
    id: "st-rem-new",
    title: "Fresh Remote Stream",
    sessions: [sRem],
  });

  const calls = [];
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      calls.push({ bin, args, on });
      if (args[0] === "stream" && args[1] === "show") {
        // Exists ONLY on retriever, not yet synced to local Mac!
        if (on === "retriever" && args[2] === stRem.key) {
          return { code: 0, stdout: JSON.stringify(stRem), stderr: "" };
        }
        return {
          code: 1,
          stdout: JSON.stringify({ error: { kind: "not-found", message: "not found locally" } }),
          stderr: "",
        };
      }
      if (args[0] === "session" && args[1] === "show") {
        if (on === "retriever" && args[2] === sRem.key) {
          return { code: 0, stdout: JSON.stringify(sRem), stderr: "" };
        }
        return {
          code: 1,
          stdout: JSON.stringify({ error: { kind: "not-found", message: "not found locally" } }),
          stderr: "",
        };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });

  // Currently focused on local Mac space, but retriever's hostId was learned passively.
  coord.activeMachine = null;
  coord.recordHostMachine("h-rem", "retriever");

  const loadedStream = await coord.inspectStream(stRem.key);
  assert.equal(loadedStream?.key, stRem.key);
  const loadedSession = await coord.previewSession(sRem.key, stRem.key);
  assert.equal(loadedSession?.key, sRem.key);

  assert.deepEqual(
    calls.map((c) => ({ cmd: `${c.args[0]} ${c.args[1]}`, on: c.on })),
    [
      { cmd: "stream show", on: "retriever" },
      { cmd: "session show", on: "retriever" },
    ]
  );
});

test("mutations route to owning remote machine, fail fast for unmapped/offline hosts, and handle rejections", async () => {
  const sRem = makeSessionJson({ host: "h-rem", id: "s1", stream_id: "st1", status: "active" });
  const stRem = makeStreamJson({ host: "h-rem", id: "st1", sessions: [sRem] });

  const calls = [];
  let rejectRemote = false;
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      calls.push({ bin, args, on });
      if (on === "retriever" && rejectRemote) {
        throw new Error("remote unreachable");
      }
      if (args[0] === "session" && args[1] === "edit") {
        sRem.status = "parked";
        return { code: 0, stdout: "", stderr: "" };
      }
      if (args[0] === "session" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(sRem), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(stRem), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "delete") {
        return { code: 0, stdout: "", stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });

  coord.activeMachine = null;
  coord.recordHostMachine("h-loc", null);
  coord.recordHostMachine("h-rem", "retriever");
  coord._hasMachineSnapshot = true;
  coord.machineLayouts.set("retriever", "ready");

  // 1. Cross-host edit from Mac space routes to retriever.
  const okRes = await coord.setSessionStatus(sRem.key, "parked", stRem.key);
  assert.equal(okRes.ok, true);
  assert.equal(calls[0].on, "retriever");

  // 2. Unmapped host fails fast with clear boundary error (zero CLI calls).
  calls.length = 0;
  const unmappedRes = await coord.setSessionStatus("h-unknown/pi/s9", "parked");
  assert.equal(unmappedRes.ok, false);
  assert.match(unmappedRes.error, /not linked to an online machine/);
  assert.equal(calls.length, 0);

  // 3. Offline mapped host fails fast without calling exec.
  coord.machineLayouts.set("retriever", "unavailable");
  const offlineRes = await coord.deleteStream(stRem.key);
  assert.equal(offlineRes.ok, false);
  assert.match(offlineRes.error, /retriever is offline/);
  assert.equal(calls.length, 0);

  // 4. Transport rejection during mutation returns structured error instead of rejecting.
  coord.machineLayouts.set("retriever", "ready");
  rejectRemote = true;
  const rejRes = await coord.dropReference(sRem.key, "https://example.com", stRem.key);
  assert.equal(rejRes.ok, false);
  assert.match(rejRes.error, /remote unreachable/);
  assert.match(coord.actionError?.message ?? "", /remote unreachable/);
});

test("openWork routes remote resume via hc --on, qualifies space.select, and distinguishes remote test -d rejection from missing dir", async () => {
  const sRem = makeSessionJson({
    host: "h-rem",
    id: "s-rem",
    stream_id: "st-rem",
    title: "Remote Task",
    status: "parked",
    directory: { path: "/home/bits/code/streams", repository: null },
  });
  const stKey = "h-rem/st-rem";

  // Case 1: Successful remote resume into an existing remote space.
  {
    const calls = [];
    const ran = [];
    const coord = new StreamsCoordinator({
      exec: async (bin, args, opts) => {
        const on = opts?.on ?? null;
        calls.push({ bin, args, on });
        if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
        if (args[0] === "session" && args[1] === "list") {
          return { code: 0, stdout: JSON.stringify({ items: [sRem] }), stderr: "" };
        }
        return { code: 0, stdout: "", stderr: "" };
      },
      streamsBin: "/Users/wendell/.local/bin/streams",
      newId: () => "id1",
    });
    coord.activeMachine = "retriever";
    coord.recordHostMachine("h-rem", "retriever");

    const res = await coord.openWork(
      {
        stream: stKey,
        terminalList: [
          {
            address: "retriever/t8a",
            machine: "retriever",
            state: "live",
            labels: { [LABEL_STREAM]: stKey, [LABEL_HERDR_SPACE]: "sp-rem-1" },
          },
        ],
      },
      { run: (cmd, arg) => ran.push({ cmd, arg }) }
    );

    assert.equal(res.ok, true);
    const listCall = calls.find((c) => c.args[0] === "session" && c.args[1] === "list");
    assert.equal(listCall?.on, "retriever");
    assert.equal(listCall?.bin, "streams");

    const hcCall = calls.find((c) => c.bin === "hc");
    assert.equal(hcCall?.on, null);
    assert.ok(hcCall.args.includes("--on"));
    assert.equal(hcCall.args[hcCall.args.indexOf("--on") + 1], "retriever");
    assert.ok(hcCall.args.includes(`${LABEL_HERDR_SPACE}=sp-rem-1`));
    assert.ok(hcCall.args.includes(`${LABEL_HERDR_TAB}=streams-tab-id1`));
    assert.equal(hcCall.args.at(-4), "streams");
    assert.deepEqual(ran, [{ cmd: "space.select", arg: { id: "retriever/sp-rem-1" } }]);
  }

  // Case 2: Launch fails + remote `test -d` rejects (transport failure) -> must NOT claim folder is missing!
  {
    const coord = new StreamsCoordinator({
      exec: async (bin, args, opts) => {
        if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
        if (args[0] === "session" && args[1] === "list") {
          return { code: 0, stdout: JSON.stringify({ items: [sRem] }), stderr: "" };
        }
        if (bin === "hc") {
          return {
            code: 1,
            stdout: "",
            stderr: JSON.stringify({ error: { kind: "spawn_failed", message: "No such file or directory" } }),
          };
        }
        if (bin === "test") {
          assert.equal(opts?.on, "retriever");
          throw new Error("connection timed out");
        }
        return { code: 0, stdout: "", stderr: "" };
      },
    });
    coord.activeMachine = "retriever";
    coord.recordHostMachine("h-rem", "retriever");

    const res = await coord.openWork({ stream: stKey });
    assert.equal(res.ok, false);
    assert.doesNotMatch(coord.actionError?.title ?? "", /Folder missing/);
    assert.doesNotMatch(res.error ?? "", /no longer exists/);
  }

  // Case 3: Launch fails + remote `test -d` returns exit code 1 -> formats missing folder with cached remote $HOME (~).
  {
    const coord = new StreamsCoordinator({
      exec: async (bin, args, opts) => {
        if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
        if (args[0] === "session" && args[1] === "list") {
          return { code: 0, stdout: JSON.stringify({ items: [sRem] }), stderr: "" };
        }
        if (bin === "hc") {
          return {
            code: 1,
            stdout: "",
            stderr: JSON.stringify({ error: { kind: "spawn_failed", message: "No such file or directory" } }),
          };
        }
        if (bin === "test") {
          assert.equal(opts?.on, "retriever");
          return { code: 1, stdout: "", stderr: "" };
        }
        if (bin === "sh") {
          assert.equal(opts?.on, "retriever");
          return { code: 0, stdout: "/home/bits\n", stderr: "" };
        }
        return { code: 0, stdout: "", stderr: "" };
      },
    });
    coord.activeMachine = "retriever";
    coord.recordHostMachine("h-rem", "retriever");

    const res = await coord.openWork({ stream: stKey });
    assert.equal(res.ok, false);
    assert.match(coord.actionError?.title ?? "", /Folder missing/);
    assert.match(res.error ?? "", /~\/code\/streams no longer exists/);
  }
});

test("openReference routes document reads remotely, falls back to native cross-machine, and ensureHomeDir caches & recovers", async () => {
  const calls = [];
  const ran = [];
  let homeAttempts = 0;
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      calls.push({ bin, args, on });
      if (args[0] === "config") {
        return {
          code: 0,
          stdout: JSON.stringify({ herdr: { viewer: { mode: "editor", placement: "pane-right" } } }),
          stderr: "",
        };
      }
      if (args[0] === "session" && args[1] === "document" && args[2] === "read") {
        return { code: 0, stdout: "# Remote Design\nHello", stderr: "" };
      }
      if (bin === "sh") {
        homeAttempts += 1;
        if (homeAttempts === 1) throw new Error("temporary network blip");
        return { code: 0, stdout: "/home/bits\n", stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
    streamsBin: "/Users/wendell/.local/bin/streams",
  });

  coord.recordHostMachine("h-rem", "retriever");

  // 1. Cross-machine preview (activeSpace is local Mac, document owned by retriever, viewer mode is "editor"):
  //    automatically falls back to native viewer reading via { on: "retriever" }.
  coord.activeMachine = null;
  const crossRes = await coord.openReference(
    {
      kind: "document",
      session: "h-rem/pi/s1",
      path: "docs/plan.md",
      title: "plan.md",
      filename: "plan.md",
    },
    {
      activeSpaceId: "sp-local",
      activeTabId: "tab-local",
      targetPaneId: "p-local",
      run: (cmd, arg) => ran.push({ cmd, arg }),
    }
  );
  assert.equal(crossRes.ok, true);
  const readCall = calls.find((c) => c.args[0] === "session" && c.args[1] === "document");
  assert.equal(readCall?.on, "retriever");
  assert.equal(readCall?.bin, "streams");
  assert.equal(ran[0]?.cmd, "pane.split");
  assert.equal(ran[0]?.arg?.mod, "streams");
  assert.match(ran[0]?.arg?.props?.content ?? "", /Remote Design/);

  // 2. Same-machine remote preview uses launch_command with portable "streams" bin (not Mac path).
  ran.length = 0;
  coord.activeMachine = "retriever";
  const sameRes = await coord.openReference(
    {
      kind: "document",
      session: "h-rem/pi/s1",
      path: "docs/plan.md",
      title: "plan.md",
      filename: "plan.md",
    },
    {
      activeSpaceId: "retriever/sp1",
      activeTabId: "retriever/tab1",
      targetPaneId: "retriever/p1",
      run: (cmd, arg) => ran.push({ cmd, arg }),
    }
  );
  assert.equal(sameRes.ok, true);
  assert.equal(ran[0]?.cmd, "pane.split");
  assert.deepEqual(ran[0]?.arg?.argv, [
    "streams",
    "session",
    "document",
    "view",
    "h-rem/pi/s1",
    "docs/plan.md",
  ]);

  // 3. ensureHomeDir recovers after initial failure, then caches on subsequent calls.
  assert.equal(await coord.ensureHomeDir("retriever"), null);
  assert.equal(coord.homeDirFor("retriever"), null);
  const [h1, h2] = await Promise.all([
    coord.ensureHomeDir("retriever"),
    coord.ensureHomeDir("retriever"),
  ]);
  assert.equal(h1, "/home/bits");
  assert.equal(h2, "/home/bits");
  assert.equal(await coord.ensureHomeDir("retriever"), "/home/bits");
  assert.equal(homeAttempts, 2);
});

test("empty local host refuses owner-only actions on unmapped synced remote records without running locally", async () => {
  const sRem = makeSessionJson({
    host: "h-rem",
    id: "s-rem",
    stream_id: "st-rem",
    status: "parked",
    directory: { path: "/home/bits/code/streams", repository: null },
  });
  const stRem = makeStreamJson({
    host: "h-rem",
    id: "st-rem",
    title: "Synced Remote Stream",
    sessions: [sRem],
  });

  const calls = [];
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      calls.push({ bin, args, on });
      if (args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "list") {
        // Local host has ZERO local Streams, while All hosts returns the synced remote Stream.
        if (args.includes("--local")) {
          return { code: 0, stdout: JSON.stringify({ items: [], unreadable: [] }), stderr: "" };
        }
        return { code: 0, stdout: JSON.stringify({ items: [stRem], unreadable: [] }), stderr: "" };
      }
      throw new Error(`unexpected local execution for unmapped foreign owner: ${bin} ${args.join(" ")}`);
    },
  });

  // No pre-registered host IDs at all: local `--local` returns empty, then user toggles All hosts.
  await coord.ensureHostStreams(true);
  await coord.setHostScope("all");
  assert.deepEqual(
    coord.hostStreams.map((s) => s.key),
    [stRem.key]
  );
  calls.length = 0;

  // Every owner-only action must refuse cleanly with ZERO local hc run / mutation / document calls.
  const openRes = await coord.openWork({ stream: stRem.key });
  assert.equal(openRes.ok, false);
  assert.match(openRes.error, /not linked to an online machine/);

  const statusRes = await coord.setSessionStatus(sRem.key, "active", stRem.key);
  assert.equal(statusRes.ok, false);
  assert.match(statusRes.error, /not linked to an online machine/);

  const dropRes = await coord.dropReference(sRem.key, "docs/a.md", stRem.key);
  assert.equal(dropRes.ok, false);
  assert.match(dropRes.error, /not linked to an online machine/);

  const delRes = await coord.deleteStream(stRem.key);
  assert.equal(delRes.ok, false);
  assert.match(delRes.error, /not linked to an online machine/);

  const docRes = await coord.openReference({
    kind: "document",
    session: sRem.key,
    path: "docs/a.md",
    title: "a.md",
  });
  assert.equal(docRes.ok, false);
  assert.match(docRes.error, /not linked to an online machine/);

  // Only config get ran during openWork/openReference; zero local stream/session/hc mutations.
  assert.equal(
    calls.some((c) => c.bin === "hc" || c.args[0] === "session" || c.args[1] === "delete"),
    false
  );

  // Companion check: arriving on a local host in All hosts mode (before --local ever ran) lazily
  // probes `stream list --local --limit 1` once, learns local host ID, and allows local actions.
  const stLocal = makeStreamJson({ host: "h-loc", id: "st-loc", title: "Local Stream" });
  const sLocal = makeSessionJson({ host: "h-loc", id: "s-loc", stream_id: "st-loc", status: "active" });
  const probeCalls = [];
  const coord2 = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      probeCalls.push({ bin, args, on });
      if (args[0] === "stream" && args[1] === "list" && args.includes("--local")) {
        return { code: 0, stdout: JSON.stringify({ items: [stLocal], unreadable: [] }), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "edit") {
        sLocal.status = "parked";
        return { code: 0, stdout: "", stderr: "" };
      }
      if (args[0] === "session" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(sLocal), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(stLocal), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  coord2.hostScope = "all";
  const localEdit = await coord2.setSessionStatus(sLocal.key, "parked", stLocal.key);
  assert.equal(localEdit.ok, true);
  assert.equal(coord2.machineForHost("h-loc"), null);
  assert.equal(
    probeCalls.filter((c) => c.args[0] === "stream" && c.args[1] === "list").length,
    1
  );
  // Foreign host on coord2 is still refused cleanly with zero additional probe calls.
  const foreignEdit = await coord2.setSessionStatus(sRem.key, "parked", stRem.key);
  assert.equal(foreignEdit.ok, false);
  assert.match(foreignEdit.error, /not linked to an online machine/);
  assert.equal(
    probeCalls.filter((c) => c.args[0] === "stream" && c.args[1] === "list").length,
    1
  );
});

test("parkAllSessions and closeAllSessions surface errors when prerequisite remote stream show rejects", async () => {
  const coord = new StreamsCoordinator({
    exec: async (_bin, args, opts) => {
      if (opts?.on === "retriever" && args[0] === "stream" && args[1] === "show") {
        throw new Error("ssh connection timed out");
      }
      throw new Error(`unexpected call: ${args.join(" ")}`);
    },
  });
  coord.recordHostMachine("h-rem", "retriever");

  const parkRes = await coord.parkAllSessions("h-rem/st-1");
  assert.equal(parkRes.ok, false);
  assert.deepEqual(parkRes.updated, []);
  assert.match(parkRes.error ?? "", /ssh connection timed out/);
  assert.equal(coord.actionError?.title, "Could not park all sessions");
  assert.match(coord.actionError?.message ?? "", /ssh connection timed out/);

  coord.clearActionError();

  const closeRes = await coord.closeAllSessions("h-rem/st-1");
  assert.equal(closeRes.ok, false);
  assert.deepEqual(closeRes.updated, []);
  assert.match(closeRes.error ?? "", /ssh connection timed out/);
  assert.equal(coord.actionError?.title, "Could not close all sessions");
  assert.match(coord.actionError?.message ?? "", /ssh connection timed out/);
});


