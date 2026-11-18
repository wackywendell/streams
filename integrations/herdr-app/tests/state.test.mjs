import test from "node:test";
import assert from "node:assert/strict";
import {
  resolveRole,
  buildViewSelections,
  observeNoticeToken,
  activeTabSession,
  resolveTabRenameTarget,
  effectiveSessionStatus,
  buildReferenceItems,
  filterHostStreams,
  unreadableNotice,
  hostStreamsEmptyMessage,
  formatRelativeAge,
  liveStreamIds,
  dispatchViewerActions,
  dispatchReferencePresentation,
  StreamsCoordinator,
} from "../state.js";

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
  assert.equal(resolveRole(undefined), "sidebar");
});

test("buildViewSelections reads the tab, focused agent first, then one frame per local terminal", () => {
  const ref = (key) => ({ adapter: "hc", endpoint: "core-1", key });
  const selections = buildViewSelections({
    endpoint: "core-1",
    visibleTerminals: ["t1", "t2", "t1", "remote/t3", ""],
    liveTerminals: ["t4", "t2"],
    focusedTerminal: "t2",
  });
  assert.deepEqual(selections, [
    { kind: "tab", terminals: [ref("t2"), ref("t1")] },
    { kind: "frame", terminal: ref("t1") },
    { kind: "frame", terminal: ref("t2") },
    { kind: "frame", terminal: ref("t4") },
  ]);

  assert.deepEqual(
    buildViewSelections({ endpoint: "core-1", visibleTerminals: [], focusedTerminal: "t9" }),
    [{ kind: "tab", terminals: [] }],
    "a focused terminal outside the tab is not part of its read"
  );
  assert.deepEqual(buildViewSelections({ endpoint: null, visibleTerminals: ["t1"] }), []);
});

test("observeNoticeToken combines only local terminal notice tokens", () => {
  const key = "streams.saved.abc";
  const token = observeNoticeToken(
    [
      { address: "t2", labels: { [key]: "u2" } },
      { address: "t1", labels: { [key]: "u1" } },
      { address: "remote/t3", labels: { [key]: "u3" } },
      { id: "t4", machine: null, labels: { [key]: "u4" } },
      { id: "remote/t5", machine: "remote", labels: { [key]: "u5" } },
    ],
    key
  );
  assert.equal(token, "t1:u1|t2:u2|t4:u4");
});

test("resolveTabRenameTarget mirrors the active tab's enrolled session title when tab_naming is enabled", () => {
  const frames = new Map([
    [
      "t1",
      {
        kind: "enrolled",
        session: { id: "s1", stream: "st1", title: "Herdr Design" },
      },
    ],
  ]);
  assert.deepEqual(
    resolveTabRenameTarget({
      activeTab: { id: "tb1", name: "Old Tab" },
      visibleTerminals: ["t1"],
      frames,
      tabNaming: true,
    }),
    {
      tabId: "tb1",
      currentName: "Old Tab",
      desiredName: "Herdr Design",
    }
  );
  assert.equal(
    resolveTabRenameTarget({
      activeTab: { id: "tb1", name: "Herdr Design" },
      visibleTerminals: ["t1"],
      frames,
      tabNaming: true,
    }),
    null
  );
  assert.equal(
    resolveTabRenameTarget({
      activeTab: { id: "tb1", name: "Old Tab" },
      visibleTerminals: ["t1"],
      frames,
      tabNaming: false,
    }),
    null
  );
});

test("buildReferenceItems formats PRs, issues, links, documents, and artifacts into one References list", () => {
  const items = buildReferenceItems(
    {
      id: "stream-1",
      links: [
        {
          key: { kind: "pr", url: "https://github.com/org/repo/pull/42" },
          name: "",
          description: "Add integration support",
        },
        {
          key: { kind: "issue", url: "https://github.com/org/repo/issues/17" },
          name: "Fix tab refresh",
          description: "Tracking bug",
        },
        {
          key: { kind: "link", url: "https://dd.slack.com/archives/C123/p456" },
          name: "Slack announcement",
          description: "Rollout notes",
        },
      ],
      documents: [
        {
          key: { repository: "origin", path: "docs/hc-herdr-app-plan.md" },
          name: "hc-herdr-app-plan.md",
          description: "Implementation plan",
        },
      ],
      artifacts: [
        {
          producer: "sess-1",
          entries: [
            {
              key: {
                host: "h1",
                stream: "stream-1",
                session: "sess-1",
                name: "report.md",
              },
              description: "Audit report",
            },
          ],
        },
      ],
    },
    { viewingSessionId: "sess-1" }
  );

  assert.deepEqual(
    items.map((item) => ({
      kind: item.kind,
      title: item.title,
      location: item.location,
      description: item.description,
      monoTitle: item.monoTitle,
      external: item.external,
    })),
    [
      {
        kind: "pr",
        title: "github.com/org/repo/pull/42",
        location: null,
        description: "Add integration support",
        monoTitle: true,
        external: true,
      },
      {
        kind: "issue",
        title: "Fix tab refresh",
        location: "github.com/org/repo/issues/17",
        description: "Tracking bug",
        monoTitle: false,
        external: true,
      },
      {
        kind: "link",
        title: "Slack announcement",
        location: "dd.slack.com/archives/C123/p456",
        description: "Rollout notes",
        monoTitle: false,
        external: true,
      },
      {
        kind: "document",
        title: "hc-herdr-app-plan.md",
        location: "docs/hc-herdr-app-plan.md",
        description: "Implementation plan",
        monoTitle: false,
        external: false,
      },
      {
        kind: "artifact",
        title: "report.md",
        location: null,
        description: "Audit report",
        monoTitle: true,
        external: false,
      },
    ]
  );
});

test("StreamsCoordinator resolves tab-local enrolled session and stream without space association, reacts to save notices, and discards stale reads", async () => {
  const calls = [];
  let pendingReadResolve = null;

  const coordinator = new StreamsCoordinator({
    streamsBin: "/usr/local/bin/streams",
    exec: async (cmd, args) => {
      calls.push({ cmd, args });
      if (args[1] === "describe") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: { catalog: "cat-1", host: "host-1" },
            endpoint: "core-1",
            notification_key: "streams.saved.key1",
          }),
          stderr: "",
        };
      }
      const req = JSON.parse(args[3]);
      if (pendingReadResolve) {
        const resolver = pendingReadResolve;
        pendingReadResolve = null;
        return await new Promise((resolve) => resolver(resolve, req));
      }
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: req.scope,
          endpoint: req.endpoint,
          verified_endpoint: req.endpoint,
          result: {
            ok: {
              kind: "read",
              notification_key: "streams.saved.key1",
              herdr_config: {
                tab_naming: true,
                viewer_mode: "editor",
                viewer_placement: "pane-right",
              },
              values: req.body.selections.map((sel) =>
                sel.kind === "tab"
                  ? {
                      ok: {
                        kind: "tab",
                        context: {
                          kind: "stream",
                          stream: {
                            id: "stream-1",
                            title: "Stream One",
                            status: "active",
                            goal: "Goal",
                            goal_short: null,
                            sessions: [
                              {
                                id: "sess-1",
                                title: "Session One",
                                status: "active",
                                harness: "pi",
                                state_short: "Working",
                              },
                            ],
                            documents: [],
                            artifacts: [],
                            links: [],
                            unreadable: [],
                          },
                        },
                      },
                    }
                  : {
                      ok: {
                        kind: "frame",
                        context: {
                          kind: "enrolled",
                          session: {
                            id: "sess-1",
                            stream: "stream-1",
                            title: "Session One",
                            status: "active",
                            recap: "Session recap",
                            state: "working",
                          },
                          companion: "shown",
                        },
                      },
                    }
              ),
            },
          },
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });

  const snap = {
    activeSpace: { id: "s1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [{ address: "t1", labels: {} }],
    focusedTerminal: "t1",
  };

  await coordinator.observe(snap);
  assert.equal(calls.length, 2, "describe + one tab read");
  assert.deepEqual(
    JSON.parse(calls[1].args[3]).body.selections.map((sel) => sel.kind),
    ["tab", "frame"]
  );
  assert.equal(coordinator.frames.get("t1")?.kind, "enrolled");
  assert.equal(coordinator.tabStreams.get("tab-1"), "stream-1");
  assert.equal(coordinator.streams.get("stream-1")?.title, "Stream One");
  assert.equal(
    activeTabSession({
      visibleTerminals: ["t1"],
      focusedTerminal: "t1",
      frames: coordinator.frames,
    })?.session?.id,
    "sess-1"
  );

  // Re-observing identical state makes zero subprocess calls
  await coordinator.observe(snap);
  assert.equal(calls.length, 2, "cached re-render does not spawn subprocesses");

  // Changing the save-notice label triggers one batched read (tab + frame together)
  const snapAfterSave = {
    ...snap,
    terminalList: [{ address: "t1", labels: { "streams.saved.key1": "uuid-2" } }],
  };
  await coordinator.observe(snapAfterSave);
  assert.equal(calls.length, 3, "save notice triggers one combined batched refresh");

  // Stale async read rejection: start a slow read, then trigger a newer read that finishes first
  let resolveSlow;
  pendingReadResolve = (resolve) => {
    resolveSlow = resolve;
  };
  const slowPromise = coordinator.observe({
    ...snapAfterSave,
    visibleTerminals: ["t1", "t2"],
  });
  await new Promise((r) => setTimeout(r, 10));
  const fastPromise = coordinator.observe({
    ...snapAfterSave,
    visibleTerminals: ["t1"],
    force: true,
  });
  await fastPromise;

  resolveSlow({
    code: 0,
    stdout: JSON.stringify({
      version: 1,
      scope: { catalog: "cat-1", host: "host-1" },
      endpoint: "core-1",
      verified_endpoint: "core-1",
      result: {
        ok: {
          kind: "read",
          notification_key: "streams.saved.key1",
          herdr_config: {
            tab_naming: false,
            viewer_mode: "less",
            viewer_placement: "tab",
          },
          values: [
            {
              ok: {
                kind: "tab",
                context: {
                  kind: "stream",
                  stream: {
                    id: "stream-stale",
                    title: "Stale Stream",
                    status: "active",
                    goal: null,
                    goal_short: null,
                    sessions: [],
                    documents: [],
                    artifacts: [],
                    links: [],
                    unreadable: [],
                  },
                },
              },
            },
            {
              ok: {
                kind: "frame",
                context: { kind: "unenrolled" },
              },
            },
            {
              ok: {
                kind: "frame",
                context: { kind: "unenrolled" },
              },
            },
          ],
        },
      },
      viewer_projection: [],
      warnings: [],
    }),
    stderr: "",
  });
  await slowPromise;

  assert.equal(
    coordinator.frames.get("t1")?.kind,
    "enrolled",
    "stale slow response must not overwrite newer frame"
  );
  assert.equal(coordinator.tabNaming, true, "stale tabNaming must not overwrite newer read");
  assert.equal(coordinator.tabStreams.get("tab-1"), "stream-1", "stale tab context is discarded");
});

test("StreamsCoordinator dispatches ViewerAction items once on fresh action replies", async () => {
  const ran = [];
  const opened = [];
  const coordinator = new StreamsCoordinator({
    exec: async (_cmd, args) => {
      if (args[1] === "describe") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: { catalog: "cat-1", host: "host-1" },
            endpoint: "core-1",
            notification_key: "streams.saved.key1",
          }),
          stderr: "",
        };
      }
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: { catalog: "cat-1", host: "host-1" },
          endpoint: "core-1",
          verified_endpoint: "core-1",
          result: {
            ok: {
              kind: "reference_opened",
              value: {
                known: [],
                result: {
                  kind: "needs_viewer",
                  value: [
                    { kind: "open_url", url: "https://example.org/issue/1" },
                    {
                      kind: "focus_terminal",
                      terminal: { adapter: "hc", endpoint: "core-1", key: "t9" },
                    },
                  ],
                },
              },
            },
          },
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });

  await coordinator.openReference(
    {
      kind: "link",
      stream: "stream-1",
      link: { kind: "issue", url: "https://example.org/issue/1" },
    },
    {
      run: (cmd, args) => ran.push({ cmd, args }),
      openUrl: (url) => opened.push(url),
    }
  );

  assert.deepEqual(opened, ["https://example.org/issue/1"]);
  assert.deepEqual(ran, [{ cmd: "pane.focus", args: { terminal: "t9" } }]);

  const extra = dispatchViewerActions(
    [
      { kind: "select_space", space: { adapter: "hc", endpoint: "core-1", key: "sp2" } },
      { kind: "focus_terminal", terminal: { adapter: "hc", endpoint: "core-1", key: "t10" } },
    ],
    {
      run: (cmd, args) => ran.push({ cmd, args }),
      openUrl: (url) => opened.push(url),
      focusTerminal: (termKey) => ran.push({ cmd: "custom.focus", args: { terminal: termKey } }),
    }
  );
  assert.deepEqual(extra, [
    { kind: "select_space", id: "sp2" },
    { kind: "focus_terminal", terminal: "t10" },
  ]);
  assert.deepEqual(ran.slice(-2), [
    { cmd: "space.select", args: { id: "sp2" } },
    { cmd: "custom.focus", args: { terminal: "t10" } },
  ]);
});

test("StreamsCoordinator.act surfaces WireError and reference_opened not_applied/uncertain outcomes", async () => {
  const coordinator = new StreamsCoordinator({
    exec: async (_cmd, args) => {
      if (args[1] === "describe") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: { catalog: "cat-1", host: "host-1" },
            endpoint: "core-1",
            notification_key: "streams.saved.key1",
          }),
          stderr: "",
        };
      }
      const req = JSON.parse(args[3]);
      const action = req.body.action;
      if (action.reference?.kind === "artifact") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: req.scope,
            endpoint: req.endpoint,
            verified_endpoint: req.endpoint,
            result: {
              ok: {
                kind: "reference_opened",
                value: {
                  known: [],
                  result: {
                    kind: "not_applied",
                    reason: "artifact is unavailable",
                  },
                },
              },
            },
            viewer_projection: [],
            warnings: [],
          }),
          stderr: "",
        };
      }
      if (action.reference?.kind === "document") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: req.scope,
            endpoint: req.endpoint,
            verified_endpoint: req.endpoint,
            result: {
              ok: {
                kind: "reference_opened",
                value: {
                  known: [],
                  result: {
                    kind: "uncertain",
                    reason: "viewer start was not confirmed",
                  },
                },
              },
            },
            viewer_projection: [],
            warnings: [],
          }),
          stderr: "",
        };
      }
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: req.scope,
          endpoint: req.endpoint,
          verified_endpoint: null,
          result: {
            error: {
              kind: "unavailable",
              message: "configured catalog is unavailable",
            },
          },
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });

  await coordinator.openReference({
    kind: "artifact",
    key: { host: "h1", stream: "stream-1", session: "sess-1", name: "report.md" },
  });
  assert.equal(coordinator.lastError, "artifact is unavailable");

  await coordinator.openReference(
    {
      kind: "document",
      stream: "stream-1",
      viewing_session: "sess-1",
      document: { repository: "origin", path: "README.md" },
    },
    { activeSpaceId: "sp1" }
  );
  assert.equal(coordinator.lastError, "viewer start was not confirmed");

  await coordinator.openWork({ stream: "stream-1" });
  assert.equal(coordinator.lastError, "configured catalog is unavailable");
});

test("dispatchReferencePresentation handles launch_command and show_content across placements", () => {
  const ran = [];
  const overlays = [];
  const run = (cmd, args) => ran.push({ cmd, args });
  const openOverlay = (spec) => overlays.push(spec);

  assert.deepEqual(
    dispatchReferencePresentation(
      {
        kind: "launch_command",
        title: "docs/spec.md",
        args: ["integration", "view-document", "--mode", "editor"],
        placement: "pane-right",
      },
      { activeSpaceId: "sp1", targetPaneId: "p1", streamsBin: "/bin/streams", run }
    ),
    { ok: true }
  );
  assert.deepEqual(ran.at(-1), {
    cmd: "pane.split",
    args: {
      direction: "right",
      pane: "p1",
      argv: ["/bin/streams", "integration", "view-document", "--mode", "editor"],
    },
  });

  assert.deepEqual(
    dispatchReferencePresentation(
      {
        kind: "launch_command",
        title: "docs/spec.md",
        args: ["integration", "view-document", "--mode", "hunk"],
        placement: "tab",
      },
      { activeSpaceId: "sp1", targetPaneId: "p1", tabNaming: true, streamsBin: "/bin/streams", run }
    ),
    { ok: true }
  );
  assert.deepEqual(ran.at(-1), {
    cmd: "tab.new",
    args: {
      space: "sp1",
      name: "docs/spec.md",
      argv: ["/bin/streams", "integration", "view-document", "--mode", "hunk"],
    },
  });

  assert.deepEqual(
    dispatchReferencePresentation(
      {
        kind: "show_content",
        title: "Spec",
        filename: "spec.md",
        content: "# Hi",
        truncated: true,
        placement: "floating",
      },
      { run, openOverlay }
    ),
    { ok: true }
  );
  assert.deepEqual(overlays[0], {
    kind: "window",
    key: "streams-viewer",
    width: 0.65,
    height: 0.75,
    mod: "streams",
    props: {
      kind: "reference-viewer",
      title: "Spec",
      filename: "spec.md",
      content: "# Hi",
      truncated: true,
    },
  });

  assert.deepEqual(
    dispatchReferencePresentation(
      {
        kind: "launch_command",
        title: "docs/spec.md",
        args: ["integration", "view-document"],
        placement: "pane-down",
      },
      { activeSpaceId: "sp1", targetPaneId: null, run }
    ),
    { ok: false, error: "No active pane to split for reference viewer." }
  );
});

test("StreamsCoordinator clears stale binding once on scope mismatch and recovers on re-describe", async () => {
  let describeCalls = 0;
  let readCalls = 0;
  const coordinator = new StreamsCoordinator({
    exec: async (_cmd, args) => {
      if (args[1] === "describe") {
        describeCalls += 1;
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: { catalog: `cat-${describeCalls}`, host: "host-1" },
            endpoint: "core-1",
            notification_key: "streams.saved.k",
          }),
          stderr: "",
        };
      }
      if (args[1] === "request") {
        readCalls += 1;
        const req = JSON.parse(args[3]);
        if (req.scope.catalog === "cat-1") {
          return {
            code: 0,
            stdout: JSON.stringify({
              version: 1,
              scope: req.scope,
              endpoint: "core-1",
              verified_endpoint: null,
              result: {
                error: {
                  kind: "unavailable",
                  message: "configured catalog and host do not match the requested scope",
                },
              },
              viewer_projection: [],
              warnings: [],
            }),
            stderr: "",
          };
        }
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: req.scope,
            endpoint: "core-1",
            verified_endpoint: "core-1",
            result: {
              ok: {
                kind: "read",
                notification_key: "streams.saved.k",
                herdr_config: { tab_naming: true, viewer_mode: "editor", viewer_placement: "pane-right" },
                values: [{ ok: { kind: "frame", context: { kind: "unenrolled" } } }],
              },
            },
            viewer_projection: [],
            warnings: [],
          }),
          stderr: "",
        };
      }
      throw new Error(`unexpected ${args.join(" ")}`);
    },
  });

  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
  });
  assert.equal(describeCalls, 1);
  assert.equal(readCalls, 1);
  assert.equal(coordinator.binding, null);
  assert.equal(coordinator.lastError, "configured catalog and host do not match the requested scope");

  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
  });
  assert.equal(describeCalls, 2);
  assert.equal(readCalls, 2);
  assert.equal(coordinator.binding?.scope?.catalog, "cat-2");
  assert.equal(coordinator.lastError, null);
});

test("effectiveSessionStatus marks exited terminals or non-agent panes offline", () => {
  assert.equal(
    effectiveSessionStatus(
      { status: "active" },
      { pane: { program: { agent: true } }, terminal: { state: "live" }, agent: { address: "t1" } }
    ),
    "active"
  );
  assert.equal(
    effectiveSessionStatus(
      { status: "active" },
      { pane: { program: { agent: true } }, terminal: { state: "exited" }, agent: null }
    ),
    "offline"
  );
  assert.equal(
    effectiveSessionStatus(
      { status: "waiting" },
      { pane: { program: { agent: false, shell: true } }, terminal: { state: "live" }, agent: null }
    ),
    "offline"
  );
  assert.equal(
    effectiveSessionStatus(
      { status: "done" },
      { pane: { program: { agent: false } }, terminal: { state: "exited" }, agent: null }
    ),
    "done"
  );
});

test("StreamsCoordinator binds pending viewer metadata to pane ids and deduplicates failed keys until stale or forced", async () => {
  let readCalls = 0;
  let failRead = true;
  const coordinator = new StreamsCoordinator({
    staleFallbackMs: 300_000,
    exec: async (_cmd, args) => {
      if (args[1] === "describe") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: { catalog: "cat-1", host: "host-1" },
            endpoint: "core-1",
            notification_key: "streams.saved.k",
          }),
          stderr: "",
        };
      }
      readCalls += 1;
      if (failRead) {
        return { code: 1, stdout: "", stderr: "temporary read failure" };
      }
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: { catalog: "cat-1", host: "host-1" },
          endpoint: "core-1",
          verified_endpoint: "core-1",
          result: {
            ok: {
              kind: "read",
              notification_key: "streams.saved.k",
              herdr_config: { tab_naming: true, viewer_mode: "native", viewer_placement: "pane-right" },
              values: [{ ok: { kind: "frame", context: { kind: "unenrolled" } } }],
            },
          },
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });

  coordinator.recordPendingViewer({
    tabId: "tab-1",
    title: "Design Doc",
    filename: "docs/design.md",
    truncated: true,
  });
  const bound = coordinator.resolveViewerPaneMeta("pane-9", "tab-1", [{ pane: "pane-9" }]);
  assert.deepEqual(bound, {
    tabId: "tab-1",
    title: "Design Doc",
    filename: "docs/design.md",
    truncated: true,
  });
  assert.equal(coordinator.resolveViewerPaneMeta("pane-9", "tab-1", [{ pane: "pane-9" }]), bound);
  assert.equal(coordinator.resolveViewerPaneMeta("pane-9", "tab-1", []), null);

  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
    now: 1000,
  });
  assert.equal(readCalls, 1);
  assert.equal(coordinator.lastError, "temporary read failure");

  // Re-observing the same failed key without force must not spam exec().
  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
    now: 2000,
  });
  assert.equal(readCalls, 1);

  // Explicit force recovers cleanly.
  failRead = false;
  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
    force: true,
    now: 3000,
  });
  assert.equal(readCalls, 2);
  assert.equal(coordinator.lastError, null);

  // Unchanged key within staleFallbackMs skips exec(); beyond staleFallbackMs refreshes.
  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
    now: 100_000,
  });
  assert.equal(readCalls, 2);

  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
    now: 400_000,
  });
  assert.equal(readCalls, 3);
});

test("StreamsCoordinator captures hostStreams on unenrolled tabs, inspects streams, and passes optional mode/placement on openReference", async () => {
  const actions = [];
  const coordinator = new StreamsCoordinator({
    exec: async (_cmd, args) => {
      if (args[1] === "describe") {
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: { catalog: "cat-1", host: "host-1" },
            endpoint: "core-1",
            notification_key: "streams.saved.k",
          }),
          stderr: "",
        };
      }
      const req = JSON.parse(args[3]);
      if (req.body.kind === "read") {
        const selKinds = req.body.selections.map((s) => s.kind);
        if (selKinds[0] === "saved_stream") {
          return {
            code: 0,
            stdout: JSON.stringify({
              version: 1,
              scope: req.scope,
              endpoint: "core-1",
              verified_endpoint: "core-1",
              result: {
                ok: {
                  kind: "read",
                  notification_key: "streams.saved.k",
                  herdr_config: { tab_naming: true, viewer_mode: "native", viewer_placement: "pane-right" },
                  values: [
                    {
                      ok: {
                        kind: "stream",
                        value: {
                          id: "st-paused",
                          title: "Paused Effort",
                          goal: "Goal",
                          goal_short: null,
                          status: "parked",
                          sessions: [],
                          documents: [],
                          links: [],
                          artifacts: [],
                        },
                      },
                    },
                  ],
                },
              },
              viewer_projection: [],
              warnings: [],
            }),
            stderr: "",
          };
        }
        return {
          code: 0,
          stdout: JSON.stringify({
            version: 1,
            scope: req.scope,
            endpoint: "core-1",
            verified_endpoint: "core-1",
            result: {
              ok: {
                kind: "read",
                notification_key: "streams.saved.k",
                herdr_config: { tab_naming: true, viewer_mode: "native", viewer_placement: "pane-right" },
                values: [
                  {
                    ok: {
                      kind: "tab",
                      context: {
                        kind: "unassociated",
                        streams: {
                          streams: [
                            {
                              id: "st-active",
                              title: "Active Effort",
                              goal_short: "Keep going",
                              status: "active",
                              active_sessions: 1,
                              total_sessions: 2,
                            },
                          ],
                          unreadable: [],
                        },
                      },
                    },
                  },
                  { ok: { kind: "frame", context: { kind: "unenrolled" } } },
                ],
              },
            },
            viewer_projection: [],
            warnings: [],
          }),
          stderr: "",
        };
      }
      actions.push(req.body.action);
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: req.scope,
          endpoint: "core-1",
          verified_endpoint: "core-1",
          result: {
            ok: {
              kind: "reference_opened",
              value: { known: [], result: { kind: "applied" } },
            },
          },
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });

  await coordinator.observe({
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
  });
  assert.equal(coordinator.hostStreams.length, 1);
  assert.equal(coordinator.hostStreams[0].id, "st-active");
  assert.equal(coordinator.tabStreams.get("tab-1"), null);

  await coordinator.inspectStream("st-paused");
  assert.equal(coordinator.inspectedStreamId, "st-paused");
  assert.equal(coordinator.streams.get("st-paused")?.title, "Paused Effort");

  const docRef = {
    kind: "document",
    stream: "st-active",
    document: { repository: "source", path: "README.md" },
    viewing_session: "s1",
  };
  await coordinator.openReference(docRef, { activeSpaceId: "sp-1" });
  assert.equal("mode" in actions[0], false);
  assert.equal("placement" in actions[0], false);

  await coordinator.openReference(docRef, {
    activeSpaceId: "sp-1",
    mode: "editor",
    placement: "pane-right",
  });
  assert.equal(actions[1].mode, "editor");
  assert.equal(actions[1].placement, "pane-right");
});

test("a tab read shows the tab's Stream, or the host's readable Streams with unreadable records, and keeps a failure after the rest of the read succeeds", async () => {
  const unreadable = {
    kind: "stream",
    host: "h",
    path: "hosts/h/streams/x.yaml",
    cause: "unknown field `archived`",
    repair: "Run /streams reset in a conversation on that Stream, or delete or fix the file.",
  };
  const listing = (streams) => ({ ok: { kind: "tab", context: { kind: "unassociated", streams } } });
  let tab = listing({ streams: [{ id: "st", title: "T", status: "active" }], unreadable: [unreadable] });
  let spawns = 0;
  const coordinator = new StreamsCoordinator({
    exec: async (_cmd, args) => {
      if (args[1] === "describe") {
        return {
          code: 0,
          stdout: JSON.stringify({ version: 1, scope: { catalog: "cat-1", host: "host-1" }, endpoint: "core-1" }),
          stderr: "",
        };
      }
      const req = JSON.parse(args[3]);
      spawns += 1;
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: req.scope,
          endpoint: "core-1",
          verified_endpoint: "core-1",
          result: {
            ok: {
              kind: "read",
              values: req.body.selections.map((sel) =>
                sel.kind === "tab" ? tab : { ok: { kind: "frame", context: { kind: "not_applicable" } } }
              ),
            },
          },
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });
  const snapshot = {
    activeSpace: { id: "sp-1", machine: null },
    activeTabId: "tab-1",
    visibleTerminals: ["t1"],
    terminalList: [],
    focusedTerminal: "t1",
  };
  await coordinator.observe(snapshot);
  assert.equal(coordinator.hostStreamsError, null);
  assert.deepEqual(coordinator.hostStreams.map((summary) => summary.id), ["st"]);
  assert.deepEqual(coordinator.hostUnreadable, [unreadable]);
  assert.deepEqual(unreadableNotice(coordinator.hostUnreadable), {
    title: "1 unreadable record is not shown",
    records: [{ label: "hosts/h/streams/x.yaml: unknown field `archived`", repair: unreadable.repair }],
  });

  // A shell routed to a Stream (by its work label or space) shows that Stream from the same
  // spawn, with its unreadable records; the host listing keeps its last value.
  const view = { id: "st", title: "T", status: "active", sessions: [], unreadable: [unreadable] };
  tab = { ok: { kind: "tab", context: { kind: "stream", stream: view } } };
  const before = spawns;
  await coordinator.observe({ ...snapshot, force: true });
  assert.equal(spawns - before, 1, "one spawn per tab refresh");
  assert.equal(coordinator.tabStreams.get("tab-1"), "st");
  assert.deepEqual(coordinator.streams.get("st"), view);
  assert.deepEqual(coordinator.hostStreams.map((summary) => summary.id), ["st"]);

  // Any other shape is a mismatch and never an empty catalog.
  for (const value of [
    { ok: { kind: "host_streams", value: { streams: [], unreadable: [] } } },
    listing([{ id: "st", title: "T", status: "active" }]),
    listing({ streams: [] }),
    { ok: { kind: "tab", context: { kind: "stream" } } },
    null,
  ]) {
    tab = value;
    await coordinator.observe({ ...snapshot, force: true });
    assert.equal(coordinator.tabStreams.get("tab-1"), null);
    assert.deepEqual(coordinator.hostStreams, []);
    assert.deepEqual(coordinator.hostUnreadable, []);
    assert.equal(
      coordinator.hostStreamsError,
      "unexpected tab context from streams; reinstall the CLI and mod together"
    );
  }

  tab = { error: { kind: "unavailable", message: "catalog is not initialized" } };
  await coordinator.observe({ ...snapshot, force: true });
  assert.deepEqual(coordinator.hostStreams, []);
  assert.deepEqual(coordinator.hostUnreadable, []);
  assert.equal(coordinator.hostStreamsError, "catalog is not initialized");

  tab = listing({ streams: [], unreadable: [] });
  await coordinator.observe({ ...snapshot, force: true });
  assert.equal(coordinator.hostStreamsError, null);
  assert.deepEqual(coordinator.hostStreams, []);
  assert.equal(unreadableNotice(coordinator.hostUnreadable), null);
});

test("hostStreamsEmptyMessage says why the host list is empty, in a fixed order", () => {
  const stream = { id: "st", title: "T", status: "active" };
  const record = { kind: "stream", host: "h", path: "p", cause: "c", repair: "r" };
  const message = (fields) =>
    hostStreamsEmptyMessage({ error: null, streams: [], shown: [], unreadable: [], ...fields });
  assert.equal(message({ error: "catalog is not initialized", unreadable: [record] }), "Streams are unavailable: catalog is not initialized");
  assert.equal(message({ streams: [stream], unreadable: [record] }), "No matching Streams.");
  assert.equal(message({ unreadable: [record] }), "No readable Streams on this host.");
  assert.equal(message({}), "No Streams on this host.");
  assert.equal(message({ streams: [stream], shown: [stream], unreadable: [record] }), null);
});

test("unreadableNotice names each record with its own repair", () => {
  assert.equal(unreadableNotice([]), null);
  assert.equal(unreadableNotice(undefined), null);
  const notice = unreadableNotice([
    { kind: "session", host: "h", path: "hosts/h/conversations/a.yaml", cause: "bad", repair: "Run /streams reset in that conversation, or delete or fix the file." },
    { kind: "host", host: "h", path: "hosts/h/host.yaml", cause: "worse", repair: "Delete or fix the file." },
  ]);
  assert.equal(notice.title, "2 unreadable records are not shown");
  assert.deepEqual(notice.records, [
    { label: "hosts/h/conversations/a.yaml: bad", repair: "Run /streams reset in that conversation, or delete or fix the file." },
    { label: "hosts/h/host.yaml: worse", repair: "Delete or fix the file." },
  ]);
});

test("filterHostStreams supports 5 status tabs, recent/alpha sorting, and text search", () => {
  const streams = [
    { id: "1", title: "Zebra", goal_short: "Alpha task", status: "active", last_updated_ms: 1000 },
    { id: "2", title: "Beta", goal_short: "Wait review", status: "waiting", last_updated_ms: 3000 },
    { id: "3", title: "Alpha", goal_short: "Stuck CI", status: "blocked", last_updated_ms: 2000 },
    { id: "4", title: "Later", goal_short: "Hold", status: "parked", last_updated_ms: 4000 },
    { id: "5", title: "Shipped", goal_short: "Finished", status: "done", last_updated_ms: 5000 },
    { id: "6", title: "Dropped", goal_short: "Abandoned", status: "closed", last_updated_ms: 6000 },
  ];

  assert.deepEqual(
    filterHostStreams(streams, { filter: "in_progress", sort: "recent" }).map((s) => s.id),
    ["2", "3", "1"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "in_progress", sort: "alpha" }).map((s) => s.id),
    ["3", "2", "1"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "parked" }).map((s) => s.id),
    ["4"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "done" }).map((s) => s.id),
    ["5"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "closed" }).map((s) => s.id),
    ["6"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "all", sort: "recent" }).map((s) => s.id),
    ["6", "5", "4", "2", "3", "1"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "all", query: "review" }).map((s) => s.id),
    ["2"]
  );
  assert.deepEqual(
    filterHostStreams(streams, { filter: "in_progress", query: "alpha" }).map((s) => s.id),
    ["3", "1"]
  );
});

test("formatRelativeAge and liveStreamIds format ages and detect live open tabs", () => {
  const now = Date.UTC(2026, 10, 17, 12, 0, 0);
  assert.equal(formatRelativeAge(0, now), "");
  assert.equal(formatRelativeAge(now - 20_000, now), "now");
  assert.equal(formatRelativeAge(now - 12 * 60_000, now), "12m");
  assert.equal(formatRelativeAge(now - 3 * 3_600_000, now), "3h");
  assert.equal(formatRelativeAge(now - 5 * 86_400_000, now), "5d");
  assert.equal(formatRelativeAge(Date.UTC(2026, 9, 10, 12, 0, 0), now), "Oct 10");

  const frames = new Map([
    ["t1", { kind: "enrolled", session: { stream: "st-live" } }],
    ["t2", { kind: "enrolled", session: { stream: "st-exited" } }],
    ["t3", { kind: "unenrolled" }],
  ]);
  const live = liveStreamIds({
    panes: [
      { terminal: "t1", program: { agent: true } },
      { terminal: "t2", program: { agent: true } },
      { terminal: "t3", program: { agent: true } },
    ],
    terminalList: [
      { address: "t1", state: "live" },
      { address: "t2", state: "exited" },
    ],
    frames,
  });
  assert.deepEqual([...live], ["st-live"]);
  assert.equal(effectiveSessionStatus({ status: "closed" }, { terminal: { state: "exited" } }), "closed");

  const sels = buildViewSelections({
    endpoint: "core-1",
    visibleTerminals: ["t-shell"],
    liveTerminals: ["t1", "t-shell"],
  });
  assert.deepEqual(
    sels.map((s) => (s.kind === "tab" ? "tab" : s.terminal.key)),
    ["tab", "t-shell", "t1"]
  );
});
