import test from "node:test";
import assert from "node:assert/strict";
import {
  resolveRole,
  buildViewSelections,
  observeNoticeToken,
  dispatchViewerActions,
  StreamsCoordinator,
} from "../state.js";

test("resolveRole selects popup, frame, and sidebar roles deterministically", () => {
  assert.equal(resolveRole({ role: "popup" }), "popup");
  assert.equal(resolveRole({ role: "frame" }), "frame");
  assert.equal(resolveRole({ role: "sidebar", pane: "p1" }), "sidebar");
  assert.equal(resolveRole({ kind: "session-popup" }), "popup");
  assert.equal(resolveRole({ pane: "p1", terminal: "t1" }), "frame");
  assert.equal(resolveRole({ terminal: "t1" }), "frame");
  assert.equal(resolveRole({}), "sidebar");
  assert.equal(resolveRole(undefined), "sidebar");
});

test("buildViewSelections batches active local sidebar and deduplicates visible terminals", () => {
  const selections = buildViewSelections({
    endpoint: "core-1",
    activeSpace: { id: "s1", machine: null },
    visibleTerminals: ["t1", "t2", "t1", "remote/t3", ""],
    viewingChoice: { kind: "follow", last_agent: null },
  });
  assert.deepEqual(selections, [
    {
      kind: "sidebar",
      space: { adapter: "hc", endpoint: "core-1", key: "s1" },
      viewing: { kind: "follow", last_agent: null },
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
      { id: "t2", machine: null, labels: { [key]: "u2" } },
      { id: "t1", machine: null, labels: { [key]: "u1" } },
      { id: "remote/t3", machine: "remote", labels: { [key]: "u3" } },
    ],
    key
  );
  assert.equal(token, "t1:u1|t2:u2");
});

test("StreamsCoordinator deduplicates describe and read calls, reacts to save notices, and discards stale reads", async () => {
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
              values: req.body.selections.map((sel) =>
                sel.kind === "sidebar"
                  ? {
                      ok: {
                        kind: "sidebar",
                        value: {
                          kind: "associated",
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
                          },
                          viewing_session: { kind: "selected", session: "sess-1" },
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
    visibleTerminals: ["t1"],
    terminalList: [{ id: "t1", machine: null, labels: {} }],
    focusedTerminal: "t1",
  };

  await coordinator.observe(snap);
  assert.equal(calls.length, 2, "one describe call + one batched read call");
  assert.equal(coordinator.sidebar?.kind, "associated");
  assert.equal(coordinator.frames.get("t1")?.kind, "enrolled");
  assert.equal(coordinator.streamTitle("stream-1"), "Stream One");

  // Re-observing identical state makes zero subprocess calls
  await coordinator.observe(snap);
  assert.equal(calls.length, 2, "cached re-render does not spawn subprocesses");

  // Changing the save-notice label triggers exactly one new batched read
  const snapAfterSave = {
    ...snap,
    terminalList: [{ id: "t1", machine: null, labels: { "streams.saved.key1": "uuid-2" } }],
  };
  await coordinator.observe(snapAfterSave);
  assert.equal(calls.length, 3, "save notice triggers one batched refresh");

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
          values: [{ ok: { kind: "sidebar", value: { kind: "unassociated" } } }],
        },
      },
      viewer_projection: [],
      warnings: [],
    }),
    stderr: "",
  });
  await slowPromise;
  assert.equal(
    coordinator.sidebar?.kind,
    "associated",
    "stale read reply must not overwrite newer state"
  );
});

test("StreamsCoordinator dispatches ViewerAction items once on fresh action replies", async () => {
  const ran = [];
  const openedUrls = [];
  const actionsSent = [];
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
      assert.equal(req.body.kind, "act");
      actionsSent.push(req.body.action);
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
                known: [{ adapter: "hc", endpoint: "core-1", key: "t-view" }],
                result: {
                  kind: "needs_viewer",
                  value: [
                    {
                      kind: "select_space",
                      space: { adapter: "hc", endpoint: "core-1", key: "s1" },
                    },
                    {
                      kind: "focus_terminal",
                      terminal: { adapter: "hc", endpoint: "core-1", key: "t-view" },
                    },
                    {
                      kind: "open_url",
                      url: "https://example.com/pr/1",
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
      kind: "artifact",
      key: { host: "host-1", stream: "stream-1", session: "sess-1", name: "out.md" },
    },
    {
      activeSpaceId: "s1",
      run: (cmd, args) => ran.push({ cmd, args }),
      openUrl: (u) => openedUrls.push(u),
    }
  );

  await coordinator.openReference(
    {
      kind: "document",
      stream: "stream-1",
      viewing_session: "sess-1",
      document: { repository: "repo-1", path: "README.md" },
    },
    {
      activeSpaceId: "s1",
      run: (cmd, args) => ran.push({ cmd, args }),
      openUrl: (u) => openedUrls.push(u),
    }
  );

  await coordinator.openReference(
    {
      kind: "link",
      stream: "stream-1",
      link: { kind: "pr", url: "https://example.com/pr/1" },
    },
    {
      activeSpaceId: "s1",
      run: (cmd, args) => ran.push({ cmd, args }),
      openUrl: (u) => openedUrls.push(u),
    }
  );

  assert.deepEqual(actionsSent[0].reference, {
    kind: "artifact",
    key: { host: "host-1", stream: "stream-1", session: "sess-1", name: "out.md" },
  });
  assert.deepEqual(actionsSent[1], {
    kind: "open_reference",
    reference: {
      kind: "document",
      stream: "stream-1",
      viewing_session: "sess-1",
      document: { repository: "repo-1", path: "README.md" },
    },
    context: { kind: "explicit" },
    at: { adapter: "hc", endpoint: "core-1", key: "s1" },
  });
  assert.deepEqual(actionsSent[2], {
    kind: "open_reference",
    reference: {
      kind: "link",
      stream: "stream-1",
      link: { kind: "pr", url: "https://example.com/pr/1" },
    },
    context: { kind: "explicit" },
    at: null,
  });

  assert.deepEqual(ran.slice(0, 2), [
    { cmd: "space.select", args: { id: "s1" } },
    { cmd: "pane.focus", args: { terminal: "t-view" } },
  ]);
  assert.equal(openedUrls.length, 3);
  assert.equal(openedUrls[0], "https://example.com/pr/1");

  // Direct dispatch helper check
  const direct = dispatchViewerActions([], {
    run: (cmd, args) => ran.push({ cmd, args }),
    openUrl: (u) => openedUrls.push(u),
  });
  assert.deepEqual(direct, []);
});

test("StreamsCoordinator.act surfaces WireError and reference_opened not_applied/uncertain outcomes", async () => {
  let nextResult;
  let notifications = 0;
  const coordinator = new StreamsCoordinator({
    notify: () => {
      notifications += 1;
    },
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
      return {
        code: 0,
        stdout: JSON.stringify({
          version: 1,
          scope: req.scope,
          endpoint: req.endpoint,
          verified_endpoint: req.endpoint,
          result: nextResult,
          viewer_projection: [],
          warnings: [],
        }),
        stderr: "",
      };
    },
  });

  nextResult = {
    error: { kind: "unavailable", message: "catalog locked" },
  };
  await coordinator.openWork({ stream: "stream-1" });
  assert.equal(coordinator.lastError, "catalog locked");
  const countAfterWireError = notifications;
  assert.ok(countAfterWireError >= 1);

  nextResult = {
    ok: {
      kind: "reference_opened",
      value: {
        known: [],
        result: { kind: "not_applied", reason: "checkout not registered" },
      },
    },
  };
  await coordinator.openReference({
    kind: "document",
    stream: "stream-1",
    viewing_session: "sess-1",
    document: { repository: "repo-1", path: "README.md" },
  });
  assert.equal(coordinator.lastError, "checkout not registered");
  assert.equal(notifications, countAfterWireError + 1);

  nextResult = {
    ok: {
      kind: "reference_opened",
      value: {
        known: [],
        result: { kind: "uncertain", reason: "terminal timed out" },
      },
    },
  };
  await coordinator.openReference({
    kind: "link",
    stream: "stream-1",
    link: { kind: "pr", url: "https://example.com/pr/1" },
  });
  assert.equal(coordinator.lastError, "terminal timed out");
  assert.equal(notifications, countAfterWireError + 2);
});
