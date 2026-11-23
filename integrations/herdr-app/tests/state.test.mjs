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
  normalizeSession,
  normalizeStream,
  resolveRole,
  parseTerminalAddress,
  recordHostId,
  qualifyForMachine,
  unqualifyForMachine,
  observeNoticeToken,
  indexLive,
  resolveSidebarView,
  resolveTabRenameTarget,
  effectiveSessionStatus,
  buildReferenceItems,
  prepareNativeContent,
  groupHostStreams,
  formatHostStreamMeta,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
  findLivePaneForStream,
  findLivePaneForSession,
  sessionForFramePane,
  buildMoveCandidates,
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
  const sess = coord.sessions.get(s1.key);
  assert.equal(sess?.key, s1.key);
  assert.equal(sess?.stream, st.key);
  assert.equal(sess?.detail, true);
  assert.equal(coord.streams.get(st.key)?.key, st.key);
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
  const live1 = indexLive({ ...common, focusedTerminal: "t1" }, { sessions: coord.sessions });
  const view1 = resolveSidebarView({
    sessions: coord.sessions,
    streams: coord.streams,
    live: live1,
  });
  assert.equal(view1.view, "session");
  assert.equal(view1.session.key, s1.key);
  assert.equal(buildReferenceItems(view1.session).length, 0);

  await coord.observe({ ...common, focusedTerminal: "t2" });
  const live2 = indexLive({ ...common, focusedTerminal: "t2" }, { sessions: coord.sessions });
  const view2 = resolveSidebarView({
    sessions: coord.sessions,
    streams: coord.streams,
    live: live2,
  });
  assert.equal(view2.view, "session");
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
  assert.equal(coord.staleSessionKeys.has("h1/pi/deleted"), true);
  assert.equal(coord.sessions.has("h1/pi/deleted"), false);
  assert.equal(coord.getStreamList().items.length, 1);

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
  assert.equal(coord.sessions.has("h1/pi/old-pi"), false);
});

test("sidebar_shows_the_stream_id", async () => {
  const s1 = makeSessionJson({ id: "s1", stream_id: "8a9b0c1d-stream" });
  const st = makeStreamJson({ id: "8a9b0c1d-stream", title: "Visible ID", sessions: [s1] });
  const runner = createFakeRunner({
    sessions: { [s1.key]: s1 },
    streams: { [st.key]: st },
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });
  const snap = {
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st.key } },
    ],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { agent: "pi" } }],
    agents: [{ address: "t1", agent: "pi" }],
  };
  await coord.observe(snap);

  const live = indexLive(snap, { sessions: coord.sessions });
  const view = resolveSidebarView({
    sessions: coord.sessions,
    streams: coord.streams,
    live,
  });
  assert.equal(view.view, "session");
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
  assert.match(coord.actionError?.message ?? "", /s-nodir/);
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

test("resolveTabRenameTarget, effectiveSessionStatus, unreadableNotice, formatRelativeAge, and indexLive", () => {
  const sessions = new Map([
    ["h1/pi/s1", { key: "h1/pi/s1", stream: "h1/st1", title: "CLI Work" }],
  ]);
  const runningByTerm = new Map([["t1", "h1/pi/s1"]]);
  assert.deepEqual(
    resolveTabRenameTarget({
      activeTab: { id: "tb1", name: "Old" },
      visibleTerminals: ["t1"],
      runningByTerm,
      sessions,
      tabNaming: true,
    }),
    { tabId: "tb1", currentName: "Old", desiredName: "CLI Work" }
  );

  assert.equal(effectiveSessionStatus({ status: "active" }, { terminal: { state: "exited" } }), "offline");
  assert.equal(effectiveSessionStatus({ status: "done" }, { terminal: { state: "exited" } }), "done");

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

  const live = indexLive(
    {
      visibleTerminals: ["t1"],
      focusedTerminal: "t1",
      terminalList: [
        { address: "t1", state: "live", labels: { [LABEL_SESSION]: "h1/pi/s1", [LABEL_STREAM]: "h1/st1" } },
      ],
      panes: [{ pane: "p1", tab: "tb1", terminal: "t1", program: { agent: "pi" } }],
      agents: [{ address: "t1", agent: "pi" }],
    },
    { sessions }
  );
  assert.equal(live.activeTerm, "t1");
  assert.equal(live.activeSessionKey, "h1/pi/s1");
  assert.equal(live.openCountByStream.get("h1/st1"), 1);

  const dispatches = [];
  const res = dispatchReferencePresentation(
    { kind: "launch_command", placement: "tab", title: "Doc", args: ["artifact", "view", "k"] },
    { activeSpaceId: "sp1", run: (cmd, args) => dispatches.push({ cmd, args }) }
  );
  assert.equal(res.ok, true);
  assert.equal(dispatches[0].cmd, "tab.new");
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
  assert.equal(coord.sessions.has("h1/pi/deleted"), false);
  // Off-tab omission must not permanently blacklist the key in staleSessionKeys.
  assert.equal(coord.staleSessionKeys.has("h1/pi/deleted"), false);

  // When t2 exits externally, next observe drops it from shared terminal marks.
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

  assert.deepEqual(Object.keys(published.at(-1).val), ["t1"]);
});

test("findLivePaneForStream prefers target Space active Tab and indexLive counts open terminals", () => {
  const sessions = new Map([
    ["h1/pi/s1", { key: "h1/pi/s1", stream: "h1/st1", status: "done" }],
    ["h1/pi/s2", { key: "h1/pi/s2", stream: "h1/st1", status: "active" }],
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

  const live = indexLive({ panes, terminalList, agents }, { sessions });
  assert.equal(live.openCountByStream.get("h1/st1"), 2);

  const preferred = findLivePaneForStream({ streamKey: "h1/st1", spaces, panes, terminalList, agents, sessions });
  assert.deepEqual(preferred, { pane: "p2", terminal: "t2", tab: "tab-active" });

  const sessPane = findLivePaneForSession({ sessionKey: "h1/pi/s1", panes, terminalList, agents });
  assert.deepEqual(sessPane, { pane: "p1", terminal: "t1", tab: "tab-inactive" });

  const shared = buildSharedTerminalMarks({ sessions, running: new Map([["t1", "h1/pi/s1"]]) });
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
  const liveOnT1 = indexLive(
    { visibleTerminals: ["t1"], focusedTerminal: "t1", panes, terminalList, agents },
    { sessions: coord.sessions }
  );
  const streamPreview = resolveSidebarView({
    preferredView: coord.preferredView,
    sessions: coord.sessions,
    streams: coord.streams,
    live: liveOnT1,
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
    sessions: coord.sessions,
    streams: coord.streams,
    live: liveOnT1,
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
  const liveNewTab = indexLive(
    {
      activeSpace: spaces[0],
      visibleTerminals: ["tNew"],
      focusedTerminal: "tNew",
      panes,
      terminalList,
      agents,
    },
    {
      sessions: coord.sessions,
      lastFocusedStreamBySpace: coord.lastFocusedStreamBySpace,
    }
  );
  assert.equal(liveNewTab.spaceStreamKey, stB.key);
  const onNewTab = resolveSidebarView({
    preferredView: coord.preferredView,
    sessions: coord.sessions,
    streams: coord.streams,
    live: liveNewTab,
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
    sessions: coord.sessions,
    streams: coord.streams,
    live: indexLive(
      {
        activeSpace: spaces[0],
        visibleTerminals: ["tA"],
        focusedTerminal: "tA",
        panes,
        terminalList,
        agents,
      },
      {
        sessions: coord.sessions,
        lastFocusedStreamBySpace: coord.lastFocusedStreamBySpace,
      }
    ),
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
  const liveEmptySpace = indexLive(
    {
      activeSpace: spaces[1],
      visibleTerminals: ["tEmpty"],
      focusedTerminal: "tEmpty",
      panes,
      terminalList,
      agents,
    },
    {
      sessions: coord.sessions,
      lastFocusedStreamBySpace: coord.lastFocusedStreamBySpace,
    }
  );
  assert.equal(liveEmptySpace.spaceStreamKey, null);
  const onEmptySpace = resolveSidebarView({
    preferredView: coord.preferredView,
    sessions: coord.sessions,
    streams: coord.streams,
    live: liveEmptySpace,
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
    sessions: coord.sessions,
    streams: coord.streams,
    live: indexLive(
      {
        activeSpace: spaces[0],
        visibleTerminals: ["tA"],
        focusedTerminal: "tA",
        panes,
        terminalList,
        agents,
      },
      { sessions: coord.sessions }
    ),
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
  assert.match(coord.actionError?.message ?? "", /disk full/);

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
  emptyCoord._upsertStream({ key: "h1/empty", id: "empty", title: "Empty", status: "active", sessions: [] }, { detail: true });
  emptyCoord._getListEntry().keys = ["h1/empty"];
  emptyCoord.previewStreamKey = "h1/empty";
  emptyCoord.preferredView = "stream";
  const delRes = await emptyCoord.deleteStream("h1/empty");
  assert.equal(delRes.ok, true);
  assert.equal(emptyCoord.streams.has("h1/empty"), false);
  assert.equal(emptyCoord.getStreamList().items.length, 0);
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

  assert.equal(coord.sessions.get(s1.key)?.key, s1.key);
  assert.equal(coord.sessions.has("h1/pi/old-pi"), false);
  assert.equal(coord.sessions.has("h1/pi/old-shell"), false);
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
  assert.equal(coord.readError, null);
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

test("setSessionStatus performs targeted authoritative refresh across sessions, streams, lists, and terminal.marks", async () => {
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
  // Normalized store and shared terminal.marks updated immediately in one tick:
  assert.equal(coord.sessions.get(s1.key)?.status, "waiting");
  assert.equal(coord.streams.get(st1.key)?.status, "waiting");
  assert.equal(coord.streams.get(st1.key)?.sessions[0]?.status, "waiting");
  assert.equal(coord.getStreamList().items[0]?.status, "waiting");
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
  assert.equal(coord.sessions.get(sRem.key)?.key, sRem.key);
  assert.equal(coord.sessions.get(sRem.key)?.stream, stRem.key);
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
  // ...and the healthy local agent STILL acquired its session and terminal.marks entry!
  assert.equal(coord.sessions.get(sLocal.key)?.key, sLocal.key);
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
  const identityCalls = [];
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      if (args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (args[0] === "catalog" && args[1] === "label") {
        identityCalls.push({ on });
        if (on === "retriever") {
          return { code: 0, stdout: JSON.stringify({ host_id: "h-rem", label: "retriever" }), stderr: "" };
        }
        return { code: 0, stdout: JSON.stringify({ host_id: "h-loc", label: "local" }), stderr: "" };
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
  await Promise.all(coord._identityProbes.values());
  assert.deepEqual(
    coord.getStreamList().items.map((s) => s.key),
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
    coord.getStreamList().items.map((s) => s.key),
    [stRemOnly.key, stLocalOnly.key]
  );

  // 4. Switching back to local/This-host sees the cached local bucket immediately.
  coord.activeMachine = null;
  coord.hostScope = "local";
  assert.deepEqual(
    coord.getStreamList().items.map((s) => s.key),
    [stLocalOnly.key]
  );
  assert.equal(coord.machineForHost("h-loc"), null);
  assert.equal(coord.machineForHost("h-rem"), "retriever");

  // 5. Verify --local flag was passed for local scope, omitted for all scope,
  //    and the unchecked remote ran its one-time `catalog label --json` identity query.
  assert.deepEqual(listCalls, [
    { on: null, isLocalFlag: true },
    { on: "retriever", isLocalFlag: false },
  ]);
  assert.deepEqual(identityCalls, [{ on: "retriever" }]);
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
      if (args[0] === "catalog" && args[1] === "label") {
        return { code: 0, stdout: JSON.stringify({ host_id: "h-loc", label: "mac" }), stderr: "" };
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
  await Promise.all(coord._identityProbes.values());
  assert.equal(coord.machineForHost("h-loc"), null);
  assert.deepEqual(
    coord.getStreamList().items.map((s) => s.key),
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
  // queries `catalog label --json` once, learns local host ID, and allows local actions.
  const stLocal = makeStreamJson({ host: "h-loc", id: "st-loc", title: "Local Stream" });
  const sLocal = makeSessionJson({ host: "h-loc", id: "s-loc", stream_id: "st-loc", status: "active" });
  const probeCalls = [];
  const coord2 = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      const on = opts?.on ?? null;
      probeCalls.push({ bin, args, on });
      if (args[0] === "catalog" && args[1] === "label") {
        return { code: 0, stdout: JSON.stringify({ host_id: "h-loc", label: "mac" }), stderr: "" };
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
    probeCalls.filter((c) => c.args[0] === "catalog" && c.args[1] === "label").length,
    1
  );
  // Foreign host on coord2 is still refused cleanly with zero additional identity calls.
  const foreignEdit = await coord2.setSessionStatus(sRem.key, "parked", stRem.key);
  assert.equal(foreignEdit.ok, false);
  assert.match(foreignEdit.error, /not linked to an online machine/);
  assert.equal(
    probeCalls.filter((c) => c.args[0] === "catalog" && c.args[1] === "label").length,
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

test("foreignHostBadge hides active host and cold boot, distinguishes online/loading/offline/unmapped, and probes unseeded All hosts", async () => {
  const coord = new StreamsCoordinator({ exec: async () => ({ code: 0, stdout: "", stderr: "" }) });
  coord._hasMachineSnapshot = true;
  coord.machineLayouts = new Map([
    ["retriever", "ready"],
    ["buildbox", "loading"],
  ]);

  // Cold boot (no host mapping discovered yet): never flash a false-positive foreign chip.
  assert.equal(coord.foreignHostBadge("h-local/s1", null), null);
  assert.equal(coord.foreignHostBadge("/invalid", null), null);
  assert.equal(coord.foreignHostBadge("", null), null);

  // Once the active host's identity is known, unmapped hosts report as unmapped.
  coord.recordHostMachine("0abb3ca7-local", null);
  assert.deepEqual(coord.foreignHostBadge("abcdef123456/s1", null), {
    label: "abcdef12",
    state: "unmapped",
    tooltip:
      "Host abcdef12 is not linked yet — visit its space or open a terminal there (read-only)",
  });

  // Map ready remote and connecting remote hosts.
  coord.recordHostMachine("62435806-rem", "retriever");
  coord.recordHostMachine("99999999-box", "buildbox");

  // Same host returns null from both local and remote active spaces (supports record, key, or bare hostId).
  assert.equal(
    coord.foreignHostBadge({ key: "0abb3ca7-local/s1", host_id: "0abb3ca7-local" }, null),
    null
  );
  assert.equal(coord.foreignHostBadge("62435806-rem", "retriever"), null);

  // Viewing remote records from local space: online vs. loading vs. offline vs. unmapped.
  assert.deepEqual(
    coord.foreignHostBadge({ key: "62435806-rem/s2", host_id: "62435806-rem" }, null),
    {
      label: "retriever",
      state: "online",
      tooltip: "On retriever",
    }
  );
  assert.deepEqual(coord.foreignHostBadge("99999999-box", null), {
    label: "buildbox",
    state: "loading",
    tooltip: "Connecting to buildbox — read-only until ready",
  });
  coord.machineLayouts.set("buildbox", "unavailable");
  assert.deepEqual(coord.foreignHostBadge("99999999-box/s3", null), {
    label: "buildbox",
    state: "offline",
    tooltip: "buildbox is offline — read-only (writes and resume unavailable)",
  });
  assert.deepEqual(coord.foreignHostBadge("deadbeef-1234", null), {
    label: "deadbeef",
    state: "unmapped",
    tooltip:
      "Host deadbeef is not linked yet — visit its space or open a terminal there (read-only)",
  });

  // Viewing local record from remote space.
  assert.deepEqual(
    coord.foreignHostBadge({ key: "0abb3ca7-local/s1", host_id: "0abb3ca7-local" }, "retriever"),
    {
      label: "local",
      state: "online",
      tooltip: "On local",
    }
  );

  // Regression: reaching a remote All hosts view without preseeding hostToMachine queries
  // `catalog label --json` once, suppresses the active remote's own badge, reports foreign
  // unmapped hosts, and does not re-query.
  const stRem = makeStreamJson({ host: "62435806-rem", id: "st-r", title: "Remote Streams" });
  const stOther = makeStreamJson({ host: "deadbeef-9999", id: "st-o", title: "Other Host Stream" });
  let probeCount = 0;
  const unseeded = new StreamsCoordinator({
    exec: async (_bin, args, opts) => {
      const on = opts?.on ?? null;
      if (args[0] === "catalog" && args[1] === "label" && on === "retriever") {
        probeCount += 1;
        return {
          code: 0,
          stdout: JSON.stringify({ host_id: "62435806-rem", label: "retriever" }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "list") {
        if (on === "retriever" && !args.includes("--local")) {
          return {
            code: 0,
            stdout: JSON.stringify({ items: [stRem, stOther], unreadable: [] }),
            stderr: "",
          };
        }
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  unseeded._hasMachineSnapshot = true;
  unseeded.machineLayouts.set("retriever", "ready");
  unseeded.activeMachine = "retriever";
  await unseeded.setHostScope("all");
  await Promise.all(unseeded._identityProbes.values());

  assert.equal(probeCount, 1);
  assert.equal(unseeded.machineForHost("62435806-rem"), "retriever");
  assert.equal(unseeded.foreignHostBadge(stRem), null);
  assert.deepEqual(unseeded.foreignHostBadge(stOther), {
    label: "deadbeef",
    state: "unmapped",
    tooltip:
      "Host deadbeef is not linked yet — visit its space or open a terminal there (read-only)",
  });

  // Subsequent forced refresh in All hosts does not re-run the owner probe.
  await unseeded.ensureHostStreams(true);
  assert.equal(probeCount, 1);
});

test("formatHostStreamMeta promotes goal, suppresses zero/redundant counts, and hides empty line 2", () => {
  const remoteBadge = { label: "retriever", state: "online", tooltip: "On retriever" };

  assert.deepEqual(
    formatHostStreamMeta({
      groupId: "open",
      openCount: 2,
      activeSessions: 5,
      goalShort: "  Workspace setup  ",
      hostBadge: remoteBadge,
    }),
    {
      goalText: "Workspace setup",
      countText: "2 open",
      hostBadge: remoteBadge,
      hasSecondLine: true,
    }
  );

  // Not open: never emits "none open" or "0 active"; shows active count only when > 0.
  assert.deepEqual(
    formatHostStreamMeta({
      groupId: "not_open",
      openCount: 0,
      activeSessions: 7,
      goalShort: "Spanwide cost doc",
      hostBadge: null,
    }),
    {
      goalText: "Spanwide cost doc",
      countText: "7 active",
      hostBadge: null,
      hasSecondLine: true,
    }
  );

  assert.deepEqual(
    formatHostStreamMeta({
      groupId: "not_open",
      openCount: 0,
      activeSessions: 0,
      goalShort: "Panel composition",
      hostBadge: null,
    }),
    {
      goalText: "Panel composition",
      countText: "",
      hostBadge: null,
      hasSecondLine: true,
    }
  );

  // Done & closed omits counts even if activeSessions/openCount were non-zero.
  assert.deepEqual(
    formatHostStreamMeta({
      groupId: "done_closed",
      openCount: 0,
      activeSessions: 2,
      goalShort: "",
      hostBadge: null,
    }),
    {
      goalText: "",
      countText: "",
      hostBadge: null,
      hasSecondLine: false,
    }
  );

  // Missing goal still keeps line 2 visible when a host badge or count exists.
  assert.deepEqual(
    formatHostStreamMeta({
      groupId: "done_closed",
      openCount: 0,
      activeSessions: 0,
      goalShort: null,
      hostBadge: remoteBadge,
    }),
    {
      goalText: "",
      countText: "",
      hostBadge: remoteBadge,
      hasSecondLine: true,
    }
  );
});

test("ensureMachineIdentity uses catalog label --json (including empty catalogs), recovers on retry, preserves deletions during pending queries, and deduplicates concurrent calls", async () => {
  const stRem = makeStreamJson({ host: "62435806-rem", id: "st-r", title: "Remote Streams" });
  const sRem = makeSessionJson({
    host: "62435806-rem",
    id: "s-r",
    stream_id: "st-r",
    status: "active",
  });

  // (a) Identity query fails (transportError) while All hosts list succeeds:
  //     rows still populate and the machine stays unchecked for later recovery.
  let failProbe = true;
  let probeCalls = 0;
  const coordFail = new StreamsCoordinator({
    exec: async (_bin, args, opts) => {
      const on = opts?.on ?? null;
      if (args[0] === "catalog" && args[1] === "label" && on === "retriever") {
        probeCalls += 1;
        if (failProbe) {
          throw new Error("ssh probe timed out");
        }
        return {
          code: 0,
          stdout: JSON.stringify({ host_id: stRem.host_id, label: "retriever" }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "list" && on === "retriever" && !args.includes("--local")) {
        return { code: 0, stdout: JSON.stringify({ items: [stRem], unreadable: [] }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  coordFail._hasMachineSnapshot = true;
  coordFail.machineLayouts.set("retriever", "ready");
  coordFail.activeMachine = "retriever";

  await coordFail.setHostScope("all");
  await Promise.all(coordFail._identityProbes.values());
  assert.equal(probeCalls, 1);
  assert.deepEqual(
    coordFail.getStreamList().items.map((s) => s.key),
    [stRem.key]
  );
  assert.equal(coordFail.machineForHost(stRem.host_id), undefined);
  assert.equal(coordFail.foreignHostBadge(stRem), null);

  // Once the transient error clears, a subsequent refresh learns ownership and reports unmapped hosts.
  failProbe = false;
  await coordFail.ensureHostStreams(true);
  await Promise.all(coordFail._identityProbes.values());
  assert.equal(probeCalls, 2);
  assert.equal(coordFail.machineForHost(stRem.host_id), "retriever");
  assert.equal(coordFail.foreignHostBadge(stRem), null);
  assert.equal(coordFail.foreignHostBadge("deadbeef-9999/st-o")?.state, "unmapped");

  // (b) Even on a machine with ZERO local Streams, `catalog label --json` immediately returns
  //     its authoritative host_id on the very first call, notifies views, and never re-queries.
  const stSynced = makeStreamJson({ host: "0abb3ca7-mac", id: "st-mac", title: "Synced Mac" });
  const stKnownForeign = makeStreamJson({
    host: "11112222-box",
    id: "st-box",
    title: "Known Foreign",
  });
  let emptyProbeCalls = 0;
  let emptyNotifyCount = 0;
  let resolveInitialIdentity;
  const initialIdentityPromise = new Promise((r) => {
    resolveInitialIdentity = r;
  });
  const coordEmpty = new StreamsCoordinator({
    notify: () => {
      emptyNotifyCount += 1;
    },
    exec: async (_bin, args, opts) => {
      const on = opts?.on ?? null;
      if (args[0] === "catalog" && args[1] === "label" && on === "retriever") {
        emptyProbeCalls += 1;
        return initialIdentityPromise;
      }
      if (args[0] === "stream" && args[1] === "list" && on === "retriever" && !args.includes("--local")) {
        return {
          code: 0,
          stdout: JSON.stringify({ items: [stSynced, stKnownForeign], unreadable: [] }),
          stderr: "",
        };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  coordEmpty._hasMachineSnapshot = true;
  coordEmpty.machineLayouts.set("retriever", "ready");
  coordEmpty.machineLayouts.set("buildbox", "ready");
  coordEmpty.recordHostMachine(stKnownForeign.host_id, "buildbox");
  coordEmpty.activeMachine = "retriever";

  await coordEmpty.setHostScope("all");
  assert.equal(coordEmpty.foreignHostBadge(stSynced), null);
  assert.equal(coordEmpty.foreignHostBadge(stKnownForeign)?.label, "buildbox");

  const notifiesBeforeResolve = emptyNotifyCount;
  resolveInitialIdentity({
    code: 0,
    stdout: JSON.stringify({ host_id: stRem.host_id, label: null }),
    stderr: "",
  });
  await Promise.all(coordEmpty._identityProbes.values());
  assert.equal(emptyProbeCalls, 1);
  assert.equal(coordEmpty.machineForHost(stRem.host_id), "retriever");
  assert.equal(emptyNotifyCount, notifiesBeforeResolve + 1);
  assert.equal(coordEmpty.foreignHostBadge(stRem), null);
  assert.equal(coordEmpty.foreignHostBadge(stSynced)?.state, "unmapped");

  const foreignDel = await coordEmpty.deleteStream(stSynced.key);
  assert.equal(foreignDel.ok, false);
  await coordEmpty.ensureHostStreams(true);
  assert.equal(emptyProbeCalls, 1);

  // (c) Through observe(): a slow identity query must NOT hold the sidebar loading guard once the
  //     All hosts list resolves, deleting a Stream while the query is pending must NOT be undone
  //     when the query settles, and concurrent ensureMachineIdentity calls share one exec.
  const stDel = makeStreamJson({ host: "0abb3ca7-mac", id: "st-del", title: "Empty Local Stream" });
  let resolveSlowProbe;
  const slowProbePromise = new Promise((r) => {
    resolveSlowProbe = r;
  });
  let concurrentProbes = 0;
  let notifyCount = 0;
  const coordRace = new StreamsCoordinator({
    notify: () => {
      notifyCount += 1;
    },
    exec: async (_bin, args, opts) => {
      const on = opts?.on ?? null;
      if (args[0] === "catalog" && args[1] === "label" && on === "retriever") {
        concurrentProbes += 1;
        return slowProbePromise;
      }
      if (args[0] === "stream" && args[1] === "list" && on === "retriever" && !args.includes("--local")) {
        return {
          code: 0,
          stdout: JSON.stringify({ items: [stRem, stDel], unreadable: [] }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "delete") {
        return { code: 0, stdout: "", stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  coordRace.recordHostMachine(stDel.host_id, null);
  coordRace.hostScope = "all";

  const remSpace = { id: "retriever/sp-race", name: "Race Remote", machine: "retriever" };
  const obsRes = await coordRace.observe({
    activeSpace: remSpace,
    activeTabId: "retriever/tb-race",
    visibleTerminals: [],
    terminalList: [],
    machineList: [
      { name: null, layout: "ready" },
      { name: "retriever", layout: "ready" },
    ],
  });
  assert.equal(obsRes?.kind, "listing");
  assert.notEqual(coordRace.lastCompletedKey, null);
  assert.equal(coordRace.inFlightKey, null);
  assert.deepEqual(
    coordRace.getStreamList().items.map((s) => s.key),
    [stRem.key, stDel.key]
  );
  assert.equal(coordRace.foreignHostBadge(stRem), null);
  assert.deepEqual(coordRace.foreignHostBadge(stDel), {
    label: "local",
    state: "online",
    tooltip: "On local",
  });

  const concurrentWriteCheck = coordRace.ensureWriteMachineForKey(sRem.key);
  const delRes = await coordRace.deleteStream(stDel.key);
  assert.equal(delRes.ok, true);
  assert.deepEqual(
    coordRace.getStreamList().items.map((s) => s.key),
    [stRem.key]
  );
  assert.equal(concurrentProbes, 1);

  const notifiesBeforeProbeResolve = notifyCount;
  resolveSlowProbe({
    code: 0,
    stdout: JSON.stringify({ host_id: stRem.host_id, label: "retriever" }),
    stderr: "",
  });

  const writeTarget = await concurrentWriteCheck;
  await Promise.all(coordRace._identityProbes.values());
  assert.deepEqual(writeTarget, { ok: true, machine: "retriever" });
  assert.deepEqual(
    coordRace.getStreamList().items.map((s) => s.key),
    [stRem.key]
  );
  assert.equal(coordRace.foreignHostBadge(stRem), null);
  assert.equal(coordRace.foreignHostBadge("deadbeef-9999/st-x")?.state, "unmapped");
  assert.equal(notifyCount, notifiesBeforeProbeResolve + 1);
  assert.equal(concurrentProbes, 1);

  // (d) Deleting a Stream during an in-flight observe() list bumps bucket.seq and leaves
  //     lastCompletedKey cleared so a repeat observe() with the same snapshot refetches cleanly.
  const stKept = makeStreamJson({ host: "0abb3ca7-mac", id: "st-kept", title: "Kept Local Stream" });
  let resolveSlowList;
  const slowListPromise = new Promise((r) => {
    resolveSlowList = r;
  });
  let resolveListStarted;
  const listStarted = new Promise((r) => {
    resolveListStarted = r;
  });
  let listCalls = 0;
  const coordListDel = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "stream" && args[1] === "list") {
        listCalls += 1;
        if (listCalls === 1) {
          resolveListStarted();
          return slowListPromise;
        }
        return {
          code: 0,
          stdout: JSON.stringify({ items: [stKept], unreadable: [] }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "delete") {
        return { code: 0, stdout: "", stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  coordListDel.recordHostMachine(stDel.host_id, null);
  const localSpace = { id: "sp-local-del", name: "Local", machine: null };
  const localSnap = {
    activeSpace: localSpace,
    activeTabId: "tb-local-del",
    visibleTerminals: [],
    terminalList: [],
    machineList: [{ name: null, layout: "ready" }],
  };

  const staleObserve = coordListDel.observe(localSnap);
  await listStarted;
  const delDuringList = await coordListDel.deleteStream(stDel.key);
  assert.equal(delDuringList.ok, true);

  resolveSlowList({
    code: 0,
    stdout: JSON.stringify({ items: [stDel, stKept], unreadable: [] }),
    stderr: "",
  });
  const staleRes = await staleObserve;
  assert.equal(staleRes, null);
  assert.equal(coordListDel.lastCompletedKey, null);
  // Wait for the automatic invalidation refetch kicked off by deleteStream.
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(listCalls, 2);
  assert.deepEqual(
    coordListDel.getStreamList().items.map((s) => s.key),
    [stKept.key]
  );

  // A repeat observe() with the exact same snapshot completes cleanly without resurrecting stDel.
  const refetched = await coordListDel.observe(localSnap);
  assert.equal(refetched?.kind, "listing");
  assert.notEqual(coordListDel.lastCompletedKey, null);
  assert.deepEqual(
    coordListDel.getStreamList().items.map((s) => s.key),
    [stKept.key]
  );
});

test("pending mutation map tracks in-flight status, bulk stream, and resume actions and clears on error", async () => {
  let resolveEdit;
  const editPromise = new Promise((r) => {
    resolveEdit = r;
  });
  let notifyCount = 0;
  const coord = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "session" && args[1] === "edit") {
        return editPromise;
      }
      if (args[0] === "stream" && args[1] === "delete") {
        return { code: 1, stdout: "", stderr: "cannot delete" };
      }
      return { code: 0, stdout: "{}", stderr: "" };
    },
    notify: () => {
      notifyCount += 1;
    },
  });
  coord.configLoaded = true;
  coord.recordHostMachine("h1", null);

  const pendingCall = coord.setSessionStatus("h1/pi/sess1", "parked", "h1/st1");
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(coord.isPending("status:h1/pi/sess1"), true);
  assert.ok(notifyCount >= 1);

  resolveEdit({ code: 1, stdout: "", stderr: "boom" });
  const res = await pendingCall;
  assert.equal(res.ok, false);
  assert.equal(coord.isPending("status:h1/pi/sess1"), false);

  const delRes = await coord.deleteStream("h1/st1");
  assert.equal(delRes.ok, false);
  assert.equal(coord.isPending("stream:h1/st1"), false);
});
test("renameSession and renameStream route to owning host, validate non-empty titles, and refresh caches", async () => {
  const calls = [];
  let sessionTitle = "Old session";
  let streamTitle = "Old stream";
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      calls.push({ bin, args, on: opts?.on ?? null });
      if (args[0] === "session" && args[1] === "edit") {
        sessionTitle = args[args.indexOf("--title") + 1];
        return { code: 0, stdout: "{}", stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "edit") {
        streamTitle = args[args.indexOf("--title") + 1];
        return { code: 0, stdout: "{}", stderr: "" };
      }
      if (args[0] === "session" && args[1] === "show") {
        return {
          code: 0,
          stdout: JSON.stringify(
            makeSessionJson({ host: "h-rem", id: "s1", stream_id: "st1", title: sessionTitle })
          ),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return {
          code: 0,
          stdout: JSON.stringify(
            makeStreamJson({
              host: "h-rem",
              id: "st1",
              title: streamTitle,
              sessions: [
                makeSessionJson({ host: "h-rem", id: "s1", stream_id: "st1", title: sessionTitle }),
              ],
            })
          ),
          stderr: "",
        };
      }
      return { code: 0, stdout: "{}", stderr: "" };
    },
  });
  coord.configLoaded = true;
  coord._hasMachineSnapshot = true;
  coord.machineLayouts = new Map([["retriever", "ready"]]);
  coord.recordHostMachine("h-rem", "retriever");
  coord._upsertStream(
    {
      key: "h-rem/st1",
      host_id: "h-rem",
      id: "st1",
      title: "Old stream",
      status: "active",
      sessions: [],
    },
    { detail: true }
  );
  coord._upsertSession(
    {
      key: "h-rem/pi/s1",
      host_id: "h-rem",
      id: "s1",
      stream: "h-rem/st1",
      title: "Old session",
      status: "active",
    },
    { detail: true }
  );

  // Empty title rejected without CLI call.
  const emptyRes = await coord.renameSession("h-rem/pi/s1", "   ", "h-rem/st1");
  assert.equal(emptyRes.ok, false);
  assert.equal(calls.length, 0);

  // Remote session rename routes with { on: "retriever" } and refreshes session + stream caches.
  const sessRes = await coord.renameSession("h-rem/pi/s1", "  New session  ", "h-rem/st1");
  assert.equal(sessRes.ok, true);
  assert.equal(sessRes.title, "New session");
  assert.equal(coord.isPending("rename:h-rem/pi/s1"), false);
  assert.equal(coord.sessions.get("h-rem/pi/s1").title, "New session");
  assert.equal(calls[0].on, "retriever");
  assert.deepEqual(calls[0].args, ["session", "edit", "h-rem/pi/s1", "--title", "New session"]);

  // Remote stream rename routes with { on: "retriever" } and updates cached stream title.
  const stRes = await coord.renameStream("h-rem/st1", "Renamed stream");
  assert.equal(stRes.ok, true);
  assert.equal(stRes.title, "Renamed stream");
  assert.equal(coord.isPending("rename:h-rem/st1"), false);
  assert.equal(coord.streams.get("h-rem/st1").title, "Renamed stream");

  // Offline remote refuses rename cleanly without invoking CLI.
  coord.machineLayouts.set("retriever", "unavailable");
  const prevCalls = calls.length;
  const offlineRes = await coord.renameStream("h-rem/st1", "Blocked rename");
  assert.equal(offlineRes.ok, false);
  assert.equal(calls.length, prevCalls);
});

test("buildMoveCandidates and moveSessionToStream filter same-host targets, update catalog and label, and refresh both Streams", async () => {
  const cands = buildMoveCandidates({
    sourceStreamKey: "h1/st1",
    hostId: "h1",
    streams: [
      { key: "h1/st1", host_id: "h1", title: "Source", status: "active", last_activity_ms: 100 },
      { key: "h1/st-done", host_id: "h1", title: "Archived", status: "done", last_activity_ms: 90 },
      { key: "h1/st-open", host_id: "h1", title: "Open Work", status: "active", last_activity_ms: 50 },
      { key: "h1/st-idle", host_id: "h1", title: "Idle Work", status: "active", last_activity_ms: 80 },
      { key: "h2/st-other", host_id: "h2", title: "Other Host", status: "active", last_activity_ms: 200 },
    ],
    openCounts: new Map([["h1/st-open", 2]]),
  });
  // Excludes source and other-host Streams; orders open active > idle active > archived.
  assert.deepEqual(
    cands.map((c) => c.stream.key),
    ["h1/st-open", "h1/st-idle", "h1/st-done"]
  );
  assert.equal(cands[0].openCount, 2);
  assert.equal(cands[2].isArchived, true);

  const terminalList = [
    { address: "t1", state: "live", labels: { [LABEL_SESSION]: "h1/pi/s1", [LABEL_STREAM]: "h1/st1" } },
  ];
  const panes = [{ pane: "p1", terminal: "t1", tab: "tb1", program: { key: "pi", agent: true } }];
  const agents = [{ address: "t1", app: "pi", program: { key: "pi", agent: true } }];

  const execCalls = [];
  let failLabel = false;
  const coord = new StreamsCoordinator({
    exec: async (bin, args, opts) => {
      execCalls.push({ bin, args, on: opts?.on ?? null });
      if (bin === "hc" && args[0] === "label" && failLabel) {
        return { code: 1, stdout: "", stderr: "hc unavailable" };
      }
      if (args[0] === "session" && args[1] === "show") {
        return {
          code: 0,
          stdout: JSON.stringify(makeSessionJson({ host: "h1", id: "s1", stream_id: "st-open" })),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "show") {
        const id = args[2].split("/")[1];
        return {
          code: 0,
          stdout: JSON.stringify(makeStreamJson({ host: "h1", id, sessions: [] })),
          stderr: "",
        };
      }
      return { code: 0, stdout: "{}", stderr: "" };
    },
  });
  coord.configLoaded = true;
  coord.recordHostMachine("h1", null);

  // Cross-host move rejected cleanly.
  const crossHost = await coord.moveSessionToStream({
    sessionKey: "h1/pi/s1",
    sourceStreamKey: "h1/st1",
    targetStreamKey: "h2/st9",
  });
  assert.equal(crossHost.ok, false);
  assert.equal(execCalls.length, 0);

  // Live session move updates catalog, relabels terminal, and refreshes both source and target streams.
  const moved = await coord.moveSessionToStream({
    sessionKey: "h1/pi/s1",
    sourceStreamKey: "h1/st1",
    targetStreamKey: "h1/st-open",
    terminalList,
    panes,
    agents,
  });
  assert.equal(moved.ok, true);
  assert.equal(coord.actionError, null);
  assert.equal(coord.isPending("move:h1/pi/s1"), false);
  assert.deepEqual(execCalls[0].args, ["session", "edit", "h1/pi/s1", "--stream", "h1/st-open"]);
  assert.deepEqual(execCalls[1].args, ["label", "t1", "streams.stream=h1/st-open"]);
  assert.equal(coord.sessions.get("h1/pi/s1")?.stream, "h1/st-open");
  assert.equal(coord.streams.has("h1/st1"), true);
  assert.equal(coord.streams.has("h1/st-open"), true);

  // Failed hc label surfaces a warning while still completing the catalog move.
  failLabel = true;
  const warned = await coord.moveSessionToStream({
    sessionKey: "h1/pi/s1",
    sourceStreamKey: "h1/st1",
    targetStreamKey: "h1/st-idle",
    terminalList,
    panes,
    agents,
  });
  assert.equal(warned.ok, true);
  assert.equal(coord.actionError?.level, "warning");
  assert.match(coord.actionError?.title ?? "", /label unchanged/i);
});

test("refreshAfterMutation advances readGeneration so stale observations cannot revert fresh edits, and pending guards reject duplicates", async () => {
  let releaseOldObserve;
  const oldObserveGate = new Promise((r) => {
    releaseOldObserve = r;
  });
  let notifyOldShowStarted;
  const oldShowStarted = new Promise((r) => {
    notifyOldShowStarted = r;
  });
  let currentTitle = "Old Title";
  let showCount = 0;

  let releaseSecondOpen;
  const openGate = new Promise((r) => {
    releaseSecondOpen = r;
  });
  let openListCalls = 0;

  const coord = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "stream" && args[1] === "show") {
        showCount += 1;
        if (showCount === 1) {
          notifyOldShowStarted();
          await oldObserveGate;
          return { code: 0, stdout: JSON.stringify(makeStreamJson({ host: "h1", id: "st1", title: "Old Title", sessions: [] })), stderr: "" };
        }
        return { code: 0, stdout: JSON.stringify(makeStreamJson({ host: "h1", id: "st1", title: currentTitle, sessions: [] })), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "edit") {
        currentTitle = args[args.indexOf("--title") + 1];
        return { code: 0, stdout: "{}", stderr: "" };
      }
      if (args[0] === "session" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Sess" })), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "list" && args.includes("--stream")) {
        openListCalls += 1;
        await openGate;
        return { code: 0, stdout: JSON.stringify({ items: [] }), stderr: "" };
      }
      return { code: 0, stdout: "{}", stderr: "" };
    },
  });
  coord.configLoaded = true;
  coord.recordHostMachine("h1", null);

  // Start a slow observe reading Old Title.
  const slowObs = coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tb1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [{ address: "t1", state: "live", labels: { [LABEL_SESSION]: "h1/pi/s1", [LABEL_STREAM]: "h1/st1" } }],
    panes: [{ pane: "p1", terminal: "t1", space: "sp1", tab: "tb1", program: { agent: "pi" } }],
    agents: [{ address: "t1", agent: "pi" }],
    force: true,
  });
  await oldShowStarted;

  // While slowObs is blocked on pre-edit stream show, rename stream to "New Title".
  const renameRes = await coord.renameStream("h1/st1", "New Title");
  assert.equal(renameRes.ok, true);
  assert.equal(coord.streams.get("h1/st1").title, "New Title");

  // Release stale observation: it must NOT revert the title back to "Old Title"!
  releaseOldObserve();
  await slowObs;
  assert.equal(coord.streams.get("h1/st1").title, "New Title");

  // Duplicate concurrent openWork on same selected session is rejected cleanly.
  const open1 = coord.openWork({
    stream: "h1/st1",
    sessions: { kind: "selected", session_ids: ["h1/pi/s1"] },
  });
  const open2 = await coord.openWork({
    stream: "h1/st1",
    sessions: { kind: "selected", session_ids: ["h1/pi/s1"] },
  });
  assert.equal(open2.ok, false);
  assert.equal(open2.duplicate, true);
  assert.equal(coord.isPending("resume:h1/pi/s1"), true);
  releaseSecondOpen();
  await open1;
  assert.equal(coord.isPending("resume:h1/pi/s1"), false);
  assert.equal(openListCalls, 1);
});

test("post-mutation refresh preserves lastCompletedAt and clears superseded inFlightKey", async () => {
  let releaseSlowShow;
  const slowShowGate = new Promise((r) => {
    releaseSlowShow = r;
  });
  let notifySlowShowStarted;
  const slowShowStarted = new Promise((r) => {
    notifySlowShowStarted = r;
  });
  let streamShowCalls = 0;

  const s1 = makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Alpha", status: "active" });
  const st1 = makeStreamJson({ host: "h1", id: "st1", title: "Work", status: "active", sessions: [s1] });
  let currentStatus = "active";

  const coord = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "show") {
        return {
          code: 0,
          stdout: JSON.stringify({ ...s1, status: currentStatus }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "show") {
        streamShowCalls += 1;
        if (streamShowCalls === 2) {
          notifySlowShowStarted();
          await slowShowGate;
        }
        return {
          code: 0,
          stdout: JSON.stringify({
            ...st1,
            status: currentStatus,
            sessions: [{ ...s1, status: currentStatus }],
          }),
          stderr: "",
        };
      }
      if (args[0] === "session" && args[1] === "edit") {
        currentStatus = args[args.indexOf("--status") + 1];
        return { code: 0, stdout: "{}", stderr: "" };
      }
      return { code: 0, stdout: JSON.stringify({ items: [st1], unreadable: [] }), stderr: "" };
    },
  });

  const snap = {
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [{ address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } }],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } }],
    agents: [{ address: "t1", app: "pi", program: { key: "pi", agent: true } }],
  };

  assert.equal(coord.lastCompletedAt, 0);
  await coord.observe(snap);
  assert.ok(coord.lastCompletedAt > 0);
  assert.equal(coord.inFlightKey, null);

  // Start a slow background refresh, then trigger a mutation while it is in flight.
  const slowRefresh = coord.observe({ ...snap, force: true });
  await slowShowStarted;
  assert.notEqual(coord.inFlightKey, null);

  const editRes = await coord.setSessionStatus(s1.key, "waiting", st1.key);
  assert.equal(editRes.ok, true);
  // refreshAfterMutation cleared inFlightKey so the sidebar is never stuck on Loading Streams…,
  // and lastCompletedAt remains > 0 even though lastCompletedKey was invalidated.
  assert.equal(coord.inFlightKey, null);
  assert.equal(coord.lastCompletedKey, null);
  assert.ok(coord.lastCompletedAt > 0);

  // Releasing the superseded refresh must leave inFlightKey null and keep the updated status.
  releaseSlowShow();
  await slowRefresh;
  assert.equal(coord.inFlightKey, null);
  assert.equal(coord.sessions.get(s1.key)?.status, "waiting");
});

test("unvisited tab and uncached enrolled tab resolve Session synchronously without view flip", async () => {
  const s1 = makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Tab One" });
  const s2 = makeSessionJson({ host: "h1", id: "s2", stream_id: "st2", title: "Tab Two" });
  const st1 = makeStreamJson({ host: "h1", id: "st1", title: "Stream One", sessions: [s1] });
  const st2 = makeStreamJson({ host: "h1", id: "st2", title: "Stream Two", sessions: [s2] });

  const runner = createFakeRunner({
    sessions: { [s1.key]: s1, [s2.key]: s2 },
    streams: { [st1.key]: st1, [st2.key]: st2 },
    streamList: [st1, st2],
  });
  const coord = new StreamsCoordinator({ exec: runner.exec });

  const terminalList = [
    { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } },
    { address: "t2", state: "live", labels: { [LABEL_SESSION]: s2.key, [LABEL_STREAM]: st2.key } },
  ];
  const panes = [
    { pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } },
    { pane: "p2", tab: "tab2", terminal: "t2", program: { key: "pi", agent: true } },
  ];
  const agents = [
    { address: "t1", app: "pi", program: { key: "pi", agent: true } },
    { address: "t2", app: "pi", program: { key: "pi", agent: true } },
  ];

  // Even BEFORE any CLI call (empty sessions/streams maps), an enrolled tab resolves to Session
  // synchronously from Herdr labels (never flipping to Stream or All!).
  const coldLive = indexLive(
    { activeSpace: { id: "sp1", machine: null }, activeTabId: "tab1", visibleTerminals: ["t1"], focusedTerminal: "t1", terminalList, panes, agents },
    { sessions: coord.sessions }
  );
  const coldView = resolveSidebarView({
    preferredView: "session",
    sessions: coord.sessions,
    streams: coord.streams,
    live: coldLive,
  });
  assert.equal(coldView.view, "session");
  assert.equal(coldView.isEnrolled, true);
  assert.equal(coldView.sessionKey, s1.key);
  assert.equal(coldView.session, null);

  // Observe tab1 only — off-tab refresh populates sessions for t2.
  await coord.observe({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList,
    panes,
    agents,
  });
  assert.equal(coord.sessions.get(s2.key)?.key, s2.key);

  // Immediately upon switching to tab2, resolveSidebarView renders Session directly from normalized store.
  const v = resolveSidebarView({
    preferredView: "session",
    sessions: coord.sessions,
    streams: coord.streams,
    live: indexLive(
      { activeSpace: { id: "sp1", machine: null }, activeTabId: "tab2", visibleTerminals: ["t2"], focusedTerminal: "t2", terminalList, panes, agents },
      { sessions: coord.sessions }
    ),
  });
  assert.equal(v.view, "session");
  assert.equal(v.isEnrolled, true);
  assert.equal(v.session.key, s2.key);
  // streamTitle resolves from normalized streams map before stream show completes.
  assert.equal(coord.streamTitle(st2.key), "Stream Two");

  // Moving the active tab's session updates sessions and streams so Session view stays synced.
  const moveRes = await coord.moveSessionToStream({
    sessionKey: s1.key,
    sourceStreamKey: st1.key,
    targetStreamKey: st2.key,
    terminalList,
    panes,
    agents,
  });
  assert.equal(moveRes.ok, true);
  s1.stream_id = "st2";
  await coord.refreshAfterMutation({ sessionKey: s1.key, streamKey: st2.key });
  const afterMove = resolveSidebarView({
    preferredView: "session",
    sessions: coord.sessions,
    streams: coord.streams,
    live: indexLive(
      { activeSpace: { id: "sp1", machine: null }, activeTabId: "tab1", visibleTerminals: ["t1"], focusedTerminal: "t1", terminalList, panes, agents },
      { sessions: coord.sessions }
    ),
  });
  assert.equal(afterMove.view, "session");
  assert.equal(afterMove.stream?.key, st2.key);
});

test("invalid session show JSON preserves cached enrollment instead of flipping tab to unenrolled", async () => {
  const s1 = makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Live Session" });
  const st1 = makeStreamJson({ host: "h1", id: "st1", title: "Live Stream", sessions: [s1] });
  let corruptNextShow = false;

  const coord = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "config") {
        return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "show") {
        if (corruptNextShow) {
          return { code: 0, stdout: "", stderr: "" };
        }
        return { code: 0, stdout: JSON.stringify(s1), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(st1), stderr: "" };
      }
      return { code: 0, stdout: JSON.stringify({ items: [st1], unreadable: [] }), stderr: "" };
    },
  });

  const snap = {
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [{ address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key } }],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } }],
    agents: [{ address: "t1", app: "pi", program: { key: "pi", agent: true } }],
  };

  await coord.observe(snap);
  assert.equal(coord.sessions.get(s1.key)?.stream, st1.key);

  corruptNextShow = true;
  await coord.observe({ ...snap, force: true });
  assert.match(coord.readError ?? "", /invalid session show JSON/);
  assert.equal(coord.sessions.get(s1.key)?.key, s1.key);
});

test("transient hostStreams failure preserves cached items and hostStreamsEmptyMessage handles unloaded state", async () => {
  assert.equal(
    hostStreamsEmptyMessage({ error: null, streams: [], shown: [], unreadable: [], loaded: false }),
    "Loading Streams…"
  );

  const st1 = makeStreamJson({ host: "h1", id: "st1", title: "Cached Stream" });
  let failList = false;
  const coord = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "stream" && args[1] === "list") {
        if (failList) {
          return { code: 1, stdout: "", stderr: "temporary timeout" };
        }
        return { code: 0, stdout: JSON.stringify({ items: [st1], unreadable: [] }), stderr: "" };
      }
      return { code: 0, stdout: "{}", stderr: "" };
    },
  });

  await coord.ensureHostStreams(true);
  assert.equal(coord.getStreamList().items.length, 1);
  assert.equal(coord.getStreamList().error, null);

  failList = true;
  await coord.ensureHostStreams(true);
  const listAfterFail = coord.getStreamList();
  assert.equal(listAfterFail.items.length, 1);
  assert.equal(listAfterFail.items[0].key, st1.key);
  assert.match(listAfterFail.error ?? "", /temporary timeout/);
  assert.equal(
    hostStreamsEmptyMessage({
      error: listAfterFail.error,
      streams: listAfterFail.items,
      shown: listAfterFail.items,
      unreadable: [],
      loaded: listAfterFail.loaded,
    }),
    null
  );
});

test("remote machine reconnect preserves cached remote sessions, syncContext clears previews synchronously, and summary upserts preserve detail", async () => {
  const sRem = makeSessionJson({
    host: "h-rem",
    id: "s1",
    stream_id: "st1",
    title: "Remote Work",
    recap: "Detailed recap",
  });
  const stRem = makeStreamJson({
    host: "h-rem",
    id: "st1",
    title: "Remote Stream",
    goal: "Full goal",
    sessions: [sRem],
  });
  const runner = createFakeRunner({
    sessions: { [sRem.key]: sRem },
    streams: { [stRem.key]: stRem },
    streamList: [stRem],
  });

  const prefChanges = [];
  const coord = new StreamsCoordinator({
    exec: runner.exec,
    initialPreferredView: "stream",
    initialHostScope: "all",
    onPreferenceChange: (k, v) => prefChanges.push([k, v]),
  });
  assert.equal(coord.preferredView, "stream");
  assert.equal(coord.hostScope, "all");

  await coord.setView("all");
  await coord.setView("all");
  await coord.setHostScope("local");
  assert.deepEqual(prefChanges, [
    ["preferredView", "all"],
    ["hostScope", "local"],
  ]);

  await coord.observe({
    activeSpace: { id: "retriever/sp1", machine: "retriever" },
    activeTabId: "retriever/tab1",
    visibleTerminals: ["retriever/t1"],
    focusedTerminal: "retriever/t1",
    terminalList: [
      {
        address: "retriever/t1",
        machine: "retriever",
        state: "live",
        labels: { [LABEL_SESSION]: sRem.key, [LABEL_STREAM]: stRem.key },
      },
    ],
    panes: [{ pane: "retriever/p1", tab: "retriever/tab1", terminal: "retriever/t1", program: { key: "pi", agent: true } }],
    agents: [{ address: "retriever/t1", app: "pi", program: { key: "pi", agent: true } }],
    machineList: [
      { name: null, layout: "ready" },
      { name: "retriever", layout: "ready" },
    ],
  });
  assert.equal(coord.sessions.get(sRem.key)?.detail, true);
  assert.equal(coord.streams.get(stRem.key)?.detail, true);

  // Normalized summary upserts (as returned by `session list --fields key,stream_id,title,status,state`)
  // must preserve detail fields (`recap`, `directory`, `updated_at_ms`, `goal`, `sessions`).
  const listSummary = normalizeSession({
    key: sRem.key,
    stream_id: "st1",
    title: "Updated Title",
    status: "waiting",
    state: "Waiting on CI",
  });
  coord._upsertSession(listSummary, { detail: false });
  assert.equal(coord.sessions.get(sRem.key)?.title, "Updated Title");
  assert.equal(coord.sessions.get(sRem.key)?.state, "Waiting on CI");
  assert.equal(coord.sessions.get(sRem.key)?.state_short, "Waiting on CI");
  assert.equal(coord.sessions.get(sRem.key)?.recap, "Detailed recap");
  assert.equal(coord.sessions.get(sRem.key)?.updated_at_ms, sRem.updated_at_ms);
  assert.deepEqual(coord.sessions.get(sRem.key)?.directory, sRem.directory);
  assert.equal(coord.sessions.get(sRem.key)?.detail, true);
  // getStreamSessions resolves live status/title directly from `sessions`.
  assert.equal(coord.getStreamSessions(stRem.key)[0]?.status, "waiting");
  assert.equal(coord.getStreamSessions(stRem.key)[0]?.title, "Updated Title");

  const streamSummary = normalizeStream({
    key: stRem.key,
    title: "Updated Stream",
    status: "waiting",
    active_sessions: 1,
    total_sessions: 1,
    last_activity_ms: 0,
  });
  coord._upsertStream(streamSummary, { detail: false });
  assert.equal(coord.streams.get(stRem.key)?.title, "Updated Stream");
  assert.equal(coord.streams.get(stRem.key)?.goal, "Full goal");
  assert.equal(coord.streams.get(stRem.key)?.sessions.length, 1);

  // syncContext preserves previews on same Space/Tab, and clears them on Space/Tab change.
  coord.previewStreamKey = "h-rem/other";
  coord.previewSessionKey = "h-rem/pi/other";
  const sameLive = indexLive(
    { activeSpace: { id: "retriever/sp1", machine: "retriever" }, activeTabId: "retriever/tab1", visibleTerminals: [] },
    { sessions: coord.sessions }
  );
  coord.syncContext(sameLive);
  assert.equal(coord.previewStreamKey, "h-rem/other");
  assert.equal(coord.previewSessionKey, "h-rem/pi/other");

  const nextLive = indexLive(
    { activeSpace: { id: "sp-local", machine: null }, activeTabId: "local-tab", visibleTerminals: [] },
    { sessions: coord.sessions }
  );
  coord.syncContext(nextLive);
  assert.equal(coord.previewStreamKey, null);
  assert.equal(coord.previewSessionKey, null);
  assert.equal(coord.activeMachine, null);

  // Temporarily disconnecting retriever keeps cached remote sessions intact.
  await coord.observe({
    activeSpace: { id: "sp-local", machine: null },
    activeTabId: "local-tab",
    visibleTerminals: [],
    focusedTerminal: null,
    terminalList: [],
    panes: [],
    agents: [],
    machineList: [
      { name: null, layout: "ready" },
      { name: "retriever", layout: "loading" },
    ],
  });
  assert.equal(coord.sessions.get(sRem.key)?.key, sRem.key);
});

test("Stage 1 edge cases: session survives stream show failure, force clears stale keys, in-flight list refetches on mutation, offline active remote sets list error", async () => {
  // 1. Successful session show followed by failed stream show preserves Session detail.
  const s1 = makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Saved Sess" });
  let failStreamShow = true;
  let sessionNotFound = false;
  const coord1 = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      if (args[0] === "session" && args[1] === "show") {
        if (sessionNotFound) {
          return {
            code: 1,
            stdout: "",
            stderr: JSON.stringify({ error: { kind: "not-found", message: "missing" } }),
          };
        }
        return { code: 0, stdout: JSON.stringify(s1), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show" && failStreamShow) {
        return { code: 1, stdout: "", stderr: "stream unreadable" };
      }
      return { code: 0, stdout: JSON.stringify({ items: [], unreadable: [] }), stderr: "" };
    },
  });
  const snap1 = {
    activeSpace: { id: "sp1", machine: null },
    activeTabId: "tab1",
    visibleTerminals: ["t1"],
    focusedTerminal: "t1",
    terminalList: [{ address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: "h1/st1" } }],
    panes: [{ pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } }],
    agents: [{ address: "t1", app: "pi", program: { key: "pi", agent: true } }],
  };
  await coord1.observe(snap1);
  assert.equal(coord1.sessions.get(s1.key)?.key, s1.key);
  assert.equal(coord1.sessions.get(s1.key)?.detail, true);
  assert.match(coord1.readError ?? "", /stream unreadable/);

  // 2. Stale key marked by session show not-found recovers on force refresh.
  sessionNotFound = true;
  coord1.sessions.clear();
  await coord1.observe({ ...snap1, force: true });
  assert.equal(coord1.staleSessionKeys.has(s1.key), true);
  sessionNotFound = false;
  failStreamShow = false;
  await coord1.observe({ ...snap1, force: true });
  assert.equal(coord1.staleSessionKeys.has(s1.key), false);
  assert.equal(coord1.sessions.get(s1.key)?.key, s1.key);

  // 3. Label-only enrolled tab (uncached session, streams.stream label present) resolves Stream access.
  const coldLive = indexLive(snap1, { sessions: new Map() });
  const coldStreamView = resolveSidebarView({
    preferredView: "stream",
    sessions: new Map(),
    streams: new Map(),
    live: coldLive,
  });
  assert.equal(coldStreamView.view, "stream");
  assert.equal(coldStreamView.currentStreamKey, "h1/st1");
  assert.equal(coldStreamView.streamKey, "h1/st1");

  // 4. Mutation while setView("all") list fetch is in flight refetches so loaded does not deadlock.
  let releaseFirstList;
  const firstListGate = new Promise((r) => {
    releaseFirstList = r;
  });
  let listCalls = 0;
  const st2 = makeStreamJson({ host: "h1", id: "st2", title: "Ready Stream" });
  const coord2 = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "stream" && args[1] === "list") {
        listCalls += 1;
        if (listCalls === 1) return firstListGate;
        return { code: 0, stdout: JSON.stringify({ items: [st2], unreadable: [] }), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "edit") return { code: 0, stdout: "{}", stderr: "" };
      return { code: 0, stdout: JSON.stringify(st2), stderr: "" };
    },
  });
  coord2.recordHostMachine("h1", null);
  const firstFetch = coord2.setView("all");
  await coord2.setSessionStatus("h1/pi/s1", "waiting", st2.key);
  releaseFirstList({ code: 0, stdout: JSON.stringify({ items: [], unreadable: [] }), stderr: "" });
  await firstFetch;
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(listCalls, 2);
  assert.equal(coord2.getStreamList().loaded, true);
  assert.equal(coord2.getStreamList().items[0]?.key, st2.key);

  // 5. Offline active remote sets bucket error instead of leaving loaded:false, error:null forever.
  const coord3 = new StreamsCoordinator({
    exec: async () => ({ code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" }),
  });
  await coord3.observe({
    activeSpace: { id: "offline/sp1", machine: "offline" },
    activeTabId: "offline/tab1",
    visibleTerminals: [],
    terminalList: [],
    panes: [],
    agents: [],
    machineList: [
      { name: null, layout: "ready" },
      { name: "offline", layout: "unavailable" },
    ],
  });
  assert.match(coord3.getStreamList("offline", "local").error ?? "", /offline is unavailable/);

  // 6. contextSynced: true prevents a late observe() with a stale Space/Tab from clobbering a new preview.
  coord3.previewStreamKey = "h1/preview";
  coord3.lastSpaceTabKey = "sp-new:tab-new";
  await coord3.observe({
    activeSpace: { id: "sp-old", machine: null },
    activeTabId: "tab-old",
    visibleTerminals: [],
    terminalList: [],
    panes: [],
    agents: [],
    contextSynced: true,
  });
  assert.equal(coord3.previewStreamKey, "h1/preview");
});

test("Stage 2: scoped fetchers skip unchanged scopes, role-scoped notify wakes frames only on frame changes, and sessionForFramePane resolves O(1)", async () => {
  const s1 = makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Active Sess", status: "active" });
  const s2 = makeSessionJson({ host: "h1", id: "s2", stream_id: "st2", title: "Off-tab Sess", status: "active" });
  const st1 = makeStreamJson({ host: "h1", id: "st1", title: "Stream One", sessions: [s1] });
  const st2 = makeStreamJson({ host: "h1", id: "st2", title: "Stream Two", sessions: [s2] });

  const execLog = [];
  const notifyScopes = [];
  const coord = new StreamsCoordinator({
    notify: (scope) => notifyScopes.push(scope),
    exec: async (_bin, args) => {
      execLog.push(args.slice(0, 3).join(" "));
      if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      if (args[0] === "session" && args[1] === "show") {
        const key = args[2];
        const sess = key === s1.key ? s1 : s2;
        return { code: 0, stdout: JSON.stringify(sess), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        const key = args[2];
        const st = key === st1.key ? st1 : st2;
        return { code: 0, stdout: JSON.stringify(st), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "list") {
        return {
          code: 0,
          stdout: JSON.stringify({
            items: [{ key: s2.key, stream_id: "st2", title: s2.title, status: s2.status, state: s2.state }],
          }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "list") {
        return { code: 0, stdout: JSON.stringify({ items: [st1, st2], unreadable: [] }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });

  const makeSnap = ({ activeTab = "tab1", t1Saved = "1", t2Saved = "1" } = {}) => ({
    activeSpace: { id: "sp1", machine: null },
    activeTabId: activeTab,
    visibleTerminals: activeTab === "tab1" ? ["t1"] : activeTab === "tab2" ? ["t2"] : [],
    focusedTerminal: activeTab === "tab1" ? "t1" : activeTab === "tab2" ? "t2" : null,
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, [LABEL_STREAM]: st1.key, [LABEL_SAVED]: t1Saved } },
      { address: "t2", state: "live", labels: { [LABEL_SESSION]: s2.key, [LABEL_STREAM]: st2.key, [LABEL_SAVED]: t2Saved } },
    ],
    panes: [
      { pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } },
      { pane: "p2", tab: "tab2", terminal: "t2", program: { key: "pi", agent: true } },
    ],
    agents: [
      { address: "t1", app: "pi", program: { key: "pi", agent: true } },
      { address: "t2", app: "pi", program: { key: "pi", agent: true } },
    ],
  });

  // Initial load: populates both active (t1) and off-tab (t2) -> wakes "all" because frame sig changed.
  const snap1 = makeSnap({ activeTab: "tab1", t1Saved: "1", t2Saved: "1" });
  const peek1 = coord.peekObservationKey(snap1);
  assert.equal(typeof peek1, "string");
  await coord.observe(snap1);
  assert.equal(notifyScopes.includes("all"), true);

  // Off-tab save on t2 while active tab t1 is unchanged:
  // `_syncActiveTarget` MUST SKIP `session show` / `stream show` for t1; ONLY `session list` runs for t2!
  execLog.length = 0;
  notifyScopes.length = 0;
  s2.status = "waiting";
  await coord.observe(makeSnap({ activeTab: "tab1", t1Saved: "1", t2Saved: "2" }));
  assert.equal(execLog.some((c) => c.startsWith("session show")), false);
  assert.equal(execLog.some((c) => c.startsWith("stream show")), false);
  assert.equal(execLog.some((c) => c.startsWith("session list")), true);
  assert.equal(coord.sessions.get(s2.key)?.status, "waiting");
  // Frame sig changed (t2 status active -> waiting) -> wakes "all".
  assert.deepEqual(notifyScopes, ["all"]);

  // Sidebar-only UI actions MUST wake ONLY "sidebar", never "all".
  notifyScopes.length = 0;
  coord.reportActionError("Oops", "minor error");
  coord.clearActionError();
  assert.deepEqual(notifyScopes, ["sidebar", "sidebar"]);

  // Switching to unenrolled tab3 loads list once; switching to unenrolled tab4 reuses loaded list (0 stream list calls!).
  execLog.length = 0;
  await coord.observe(makeSnap({ activeTab: "tab3", t1Saved: "1", t2Saved: "2" }));
  execLog.length = 0;
  await coord.observe(makeSnap({ activeTab: "tab4", t1Saved: "1", t2Saved: "2" }));
  assert.equal(execLog.some((c) => c.startsWith("stream list")), false);

  // sessionForFramePane resolves enrolled Pi panes directly (including content.terminal) and ignores non-Pi shells.
  const resolved = sessionForFramePane({
    pane: { pane: "p1", tab: "tab1", content: { terminal: "t1" }, program: { key: "pi", agent: true } },
    terminalList: snap1.terminalList,
    agents: snap1.agents,
    staleSessionKeys: coord.staleSessionKeys,
    sessions: coord.sessions,
  });
  assert.equal(resolved?.sessionKey, s1.key);
  assert.equal(resolved?.session?.title, "Active Sess");
  assert.equal(
    sessionForFramePane({
      pane: { pane: "p-zsh", tab: "tab1", terminal: "t1", program: { key: "zsh", shell: true, agent: false } },
      terminalList: snap1.terminalList,
      agents: [],
      staleSessionKeys: coord.staleSessionKeys,
      sessions: coord.sessions,
    }),
    null
  );
});

test("Stage 2 review fixes: sibling save, non-Pi shell save, cold Space fallback, reconnect refetch, homeDir & stale frame wakeups, and tab-switch stability", async () => {
  // 1. Same-stream sibling save refreshes sibling AND active Stream instead of reverting from stale cache.
  const s1 = makeSessionJson({ host: "h1", id: "s1", stream_id: "st1", title: "Lead", status: "active" });
  const s2 = makeSessionJson({ host: "h1", id: "s2", stream_id: "st1", title: "Worker", status: "active" });
  const st1 = makeStreamJson({ host: "h1", id: "st1", title: "Shared Stream", sessions: [s1, s2] });

  const calls = [];
  const scopes = [];
  let failSessionShow = false;
  let sessionNotFound = false;
  const coord = new StreamsCoordinator({
    notify: (scope) => scopes.push(scope),
    exec: async (_bin, args, opts) => {
      calls.push({ on: opts?.on ?? null, cmd: args.slice(0, 3).join(" ") });
      if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      if (args[0] === "-c") return { code: 0, stdout: "/home/bits", stderr: "" };
      if (args[0] === "session" && args[1] === "show") {
        if (sessionNotFound) {
          return { code: 1, stdout: "", stderr: JSON.stringify({ error: { kind: "not-found", message: "gone" } }) };
        }
        if (failSessionShow) return { code: 1, stdout: "", stderr: "transient err" };
        return { code: 0, stdout: JSON.stringify(args[2] === s1.key ? s1 : s2), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify({ ...st1, sessions: sessionNotFound ? [s2] : [s1, s2] }), stderr: "" };
      }
      if (args[0] === "session" && args[1] === "list") {
        const list = sessionNotFound ? [s2] : [s1, s2];
        return {
          code: 0,
          stdout: JSON.stringify({
            items: list.map((s) => ({ key: s.key, stream_id: "st1", title: s.title, status: s.status, state: s.state })),
          }),
          stderr: "",
        };
      }
      if (args[0] === "stream" && args[1] === "list") {
        return { code: 0, stdout: JSON.stringify({ items: [st1], unreadable: [] }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });

  const mkSnap = ({ tab = "tab1", t1s = "1", t2s = "1", shSaved = null, streamLabel = st1.key } = {}) => ({
    activeSpace: { id: "sp1", machine: null, tabs: [{ id: "tab1" }, { id: "tab2" }, { id: "tab-unenrolled" }] },
    activeTabId: tab,
    visibleTerminals: tab === "tab1" ? ["t1"] : tab === "tab2" ? ["t2"] : [],
    focusedTerminal: tab === "tab1" ? "t1" : tab === "tab2" ? "t2" : null,
    terminalList: [
      { address: "t1", state: "live", labels: { [LABEL_SESSION]: s1.key, ...(streamLabel ? { [LABEL_STREAM]: streamLabel } : {}), [LABEL_SAVED]: t1s } },
      { address: "t2", state: "live", labels: { [LABEL_SESSION]: s2.key, ...(streamLabel ? { [LABEL_STREAM]: streamLabel } : {}), [LABEL_SAVED]: t2s } },
      ...(shSaved ? [{ address: "t-sh", state: "live", labels: { [LABEL_SAVED]: shSaved } }] : []),
    ],
    panes: [
      { pane: "p1", tab: "tab1", terminal: "t1", program: { key: "pi", agent: true } },
      { pane: "p2", tab: "tab2", terminal: "t2", program: { key: "pi", agent: true } },
    ],
    agents: [
      { address: "t1", app: "pi", program: { key: "pi", agent: true } },
      { address: "t2", app: "pi", program: { key: "pi", agent: true } },
    ],
  });

  await coord.observe(mkSnap());
  assert.equal(coord.sessions.get(s2.key)?.status, "active");

  // Sibling t2 changes status to waiting and saves: must update s2 (not revert!).
  s2.status = "waiting";
  calls.length = 0;
  await coord.observe(mkSnap({ t2s: "2" }));
  assert.equal(coord.sessions.get(s2.key)?.status, "waiting");
  assert.equal(calls.some((c) => c.cmd.startsWith("stream show")), true);

  // Switching Tab 1 -> Tab 2 -> Tab 1 without any saves runs ZERO off-tab session list calls (no ping-pong).
  calls.length = 0;
  await coord.observe(mkSnap({ tab: "tab2", t2s: "2" }));
  await coord.observe(mkSnap({ tab: "tab1", t2s: "2" }));
  assert.equal(calls.some((c) => c.cmd.startsWith("session list")), false);

  // 2. Non-Pi shell terminal `streams.saved` refreshes active target even when Pi tokens are unchanged!
  s1.title = "Edited from shell";
  calls.length = 0;
  await coord.observe(mkSnap({ t2s: "2", shSaved: "cli-99" }));
  assert.equal(calls.some((c) => c.cmd.startsWith("session show")), true);
  assert.equal(coord.sessions.get(s1.key)?.title, "Edited from shell");

  // 3. ensureHomeDir wakes "all" so remote pane headers shorten ~/ paths.
  scopes.length = 0;
  const homeDir = await coord.ensureHomeDir("retriever");
  assert.equal(homeDir, "/home/bits");
  assert.deepEqual(scopes, ["all"]);

  // 4. Marking a previously cached live session stale via `not-found` wakes "all" (frames clear pill).
  scopes.length = 0;
  sessionNotFound = true;
  await coord.observe({ ...mkSnap({ t2s: "2", shSaved: "cli-99" }), force: true });
  assert.equal(coord.staleSessionKeys.has(s1.key), true);
  assert.equal(scopes.includes("all"), true);

  // 5. Cold-boot unenrolled tab with off-tab Pi terminal lacking `streams.stream` label still fetches Space fallback Stream detail.
  sessionNotFound = false;
  const coordCold = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      if (args[0] === "session" && args[1] === "list") {
        return { code: 0, stdout: JSON.stringify({ items: [{ key: s2.key, stream_id: "st1", title: s2.title, status: "active" }] }), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(st1), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "list") {
        return { code: 0, stdout: JSON.stringify({ items: [st1], unreadable: [] }), stderr: "" };
      }
      return { code: 0, stdout: "", stderr: "" };
    },
  });
  await coordCold.observe(mkSnap({ tab: "tab-unenrolled", streamLabel: null }));
  assert.equal(coordCold.streams.get(st1.key)?.detail, true);

  // 6. Remote machine disconnect -> reconnect clears cached active/off-tab sigs so reconnect refetches.
  const sRem = makeSessionJson({ host: "h-rem", id: "sr1", stream_id: "str1", title: "Before" });
  const stRem = makeStreamJson({ host: "h-rem", id: "str1", title: "Remote", sessions: [sRem] });
  let remShowCalls = 0;
  const coordRem = new StreamsCoordinator({
    exec: async (_bin, args) => {
      if (args[0] === "config") return { code: 0, stdout: JSON.stringify({ herdr: {} }), stderr: "" };
      if (args[0] === "session" && args[1] === "show") {
        remShowCalls += 1;
        return { code: 0, stdout: JSON.stringify(sRem), stderr: "" };
      }
      if (args[0] === "stream" && args[1] === "show") {
        return { code: 0, stdout: JSON.stringify(stRem), stderr: "" };
      }
      return { code: 0, stdout: JSON.stringify({ items: [], unreadable: [] }), stderr: "" };
    },
  });
  const remSnap = (layout) => ({
    activeSpace: { id: "retriever/sp1", machine: "retriever" },
    activeTabId: "retriever/tab1",
    visibleTerminals: ["retriever/t1"],
    focusedTerminal: "retriever/t1",
    terminalList: [{ address: "retriever/t1", machine: "retriever", state: "live", labels: { [LABEL_SESSION]: sRem.key, [LABEL_STREAM]: stRem.key, [LABEL_SAVED]: "1" } }],
    panes: [{ pane: "retriever/p1", tab: "retriever/tab1", terminal: "retriever/t1", program: { key: "pi", agent: true } }],
    agents: [{ address: "retriever/t1", app: "pi", program: { key: "pi", agent: true } }],
    machineList: [
      { name: null, layout: "ready" },
      { name: "retriever", layout },
    ],
  });
  await coordRem.observe(remSnap("ready"));
  assert.equal(remShowCalls, 1);
  await coordRem.observe(remSnap("unavailable"));
  sRem.title = "After Reconnect";
  await coordRem.observe(remSnap("ready"));
  assert.equal(remShowCalls, 2);
  assert.equal(coordRem.sessions.get(sRem.key)?.title, "After Reconnect");
});
