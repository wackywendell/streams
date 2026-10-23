# Pi child protocol, version 1

Status: proposed contract between the Pi extension and the long-lived Rust child described in [Bounded, event-driven work in every Streams process](bounded-work-and-card-events.md). Both sides should implement exactly this; any change bumps the version and updates this file.

The protocol carries today's CLI calls over one pipe, so each side can move independently. A request is the argument vector the extension already passes to `streams`, and a response is what the CLI would have printed. The new parts are the handshake, cancellation, and child events that replace polling and retry timers.

## Launch and lifetime

The extension starts one child per Pi session:

```
streams serve --session-id <Pi session id> --plugin-root <work-cards plugin root>
```

It uses the same binary selection, environment and working directory as its current native calls. Stdin carries requests, and stdout carries responses and events. Stderr is human-readable diagnostics that the extension may log but never parses.

When Pi switches to another session, the extension shuts down the old child and starts a new one. To shut down, the extension closes the child's stdin. The child finishes in-flight requests within their deadlines and exits with status 0.

Any other exit is a crash. The extension rejects in-flight requests, warns the user, and restarts the child once per Pi session. If the restarted child also exits, the extension warns again and stops Streams for that Pi session until reload. A restart is not a session start, so the extension must not request an automatic open after a restart.

## Framing

Each message is one JSON object on one line, terminated by `\n`, with a `type` field. A message of an unknown type, invalid JSON, or a missing field is a protocol error. The receiver treats it as a crash of the other side: the extension kills and handles the child as above, and the child exits with a nonzero status.

## Handshake

The child's first line is:

```json
{"type":"ready","protocol":1}
```

The extension sends no requests until it receives this line. It treats any other first line, a different version, or no line within 10 seconds as a failed start, handled like a crash. Immediately after `ready`, the child sends its first `scope` event.

## Requests and responses

```json
{"type":"request","id":7,"argv":["agent","context","--session-id","…","--harness","pi","--json"],"input":{…}}
{"type":"response","id":7,"code":0,"value":{…},"notices":[]}
```

- **`id`** is a positive integer, unique for the child's lifetime.
- **`argv`** is the argument vector after `streams`, exactly as the extension passes it today, including `--json` and, for native calls, `--session-id`, `--plugin-root` and `--expect`.
- **`input`** is present only when the CLI call reads JSON from stdin today.
- **`code`** is the CLI exit status: 0 for success, 1 for an error, and 2 for an accepted save with warnings.
- **`value`** is the JSON the CLI prints on stdout.
- **`notices`** are the lines the CLI would print on stderr.

The extension can therefore map a response to its existing `{code, stdout, stderr}` handling without changing how it interprets results.

The child accepts only the commands the extension uses:

- `agent context`, `agent capture-inputs`, `agent preview`, `agent enroll` and `agent apply`
- `list`
- `workspace capture`
- `sync`
- `native observe`, `native open`, `native close`, `native project` and `native capture`

Any other command gets code 1 with error kind `unsupported-command`. A native request whose `--session-id` differs from the child's launch session gets code 1 with error kind `session-mismatch`. `debug`, `replay` and the prompt lab keep running as separate processes; they are rare and user-invoked.

The child runs requests concurrently and responds in completion order, so callers match responses by `id`. Locking is the same as for separate CLI processes. If a request changes scope, the child emits that scope before the response. A response fences the scope reads caused by that request; independent external events may still arrive later.

## Cancellation

```json
{"type":"cancel","id":7}
```

The extension sends this when a request's signal aborts or its deadline passes, then rejects the request locally. The child cancels the operation if it is still running, and still sends a response, which the extension discards. A cancel for an unknown or finished `id` is ignored. A timeout does not kill the child.

## Events

The child sends events without a request.

```json
{"type":"event","event":"scope","value":{…}}
{"type":"event","event":"scope","value":null,"reason":"…"}
{"type":"event","event":"warning","message":"…"}
```

**`scope`** carries the child's current owner observation. Its `value` uses the same schema as `native observe --json`. `value` is `null`, with a human-readable `reason`, when there is no observation, for example when the session is not enrolled or its workspace is unbound. The child sends a `scope` event after `ready`, and again whenever the observation changes. That happens when:

- the owner pane, tab or workspace moves
- the workspace binding changes
- the Session's membership or title changes
- enrollment becomes ready

The child sends nothing when the observation is unchanged.

The extension should adopt each `scope` value as its current expectation and run its usual update. A changed scope is a valid move, not a reason to require a reload. A request that races a move may fail with error kind `native-context-changed`. The extension should then wait for the next `scope` event, rather than fencing permanently. It retries the operation only if the user requested it; automatic work is simply dropped. These events replace the extension's 15-second native retry timer.

**`warning`** is a problem the user should see. The extension shows it the same way it shows today's native warnings. When the child cannot establish a required subscription, it sends a `warning` and then exits with a nonzero status.

## Ownership

The Rust side owns:

- `streams serve` and in-process dispatch
- the Herdr socket client and event subscriptions
- `scope` events and move reconciliation
- event-driven cards and card self-close
- the automatic-open rule

The Pi side owns:

- the child's lifecycle, including handshake, restart and shutdown
- routing all of the commands above through the child
- adopting `scope` events
- removing timers and per-event process starts from the extension

Until `streams serve` exists, the Pi side can test against a stand-in child. The stand-in implements this protocol by running one CLI process per request, and emits a `scope` event after `ready` by calling `native observe`.
