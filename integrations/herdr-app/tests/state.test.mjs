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

test("buildViewSelections batches optional known stream and deduplicates local visible terminals", () => {
  const selections = buildViewSelections({
    endpoint: "core-1",
    knownStreamId: "stream-1",
    visibleTerminals: ["t1", "t2", "t1", "remote/t3", ""],
  });
  assert.deepEqual(selections, [
    {
      kind: "saved_stream",
      stream: "stream-1",
    },
    {
      kind: "frame",
      terminal: { adapter: "hc", endpoint: "core-1", key: "t1" },
    },
    {
      kind: "frame",
      terminal: { adapter: "hc", endpoint: "core-1", key: "t2" },
    },
  ]);
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
                sel.kind === "saved_stream"
                  ? {
                      ok: {
                        kind: "stream",
                        value: {
                          id: sel.stream,
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
  assert.equal(calls.length, 3, "describe + initial frame read + follow-up saved_stream read");
  assert.equal(coordinator.frames.get("t1")?.kind, "enrolled");
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
  assert.equal(calls.length, 3, "cached re-render does not spawn subprocesses");

  // Changing the save-notice label triggers one batched read (saved_stream + frame together)
  const snapAfterSave = {
    ...snap,
    terminalList: [{ address: "t1", labels: { "streams.saved.key1": "uuid-2" } }],
  };
  await coordinator.observe(snapAfterSave);
  assert.equal(calls.length, 4, "save notice triggers one combined batched refresh");

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
                kind: "stream",
                value: {
                  id: "stream-stale",
                  title: "Stale Stream",
                  status: "active",
                  goal: null,
                  goal_short: null,
                  sessions: [],
                  documents: [],
                  artifacts: [],
                  links: [],
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
