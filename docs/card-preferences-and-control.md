# ADR: Streams owns card preferences and lifecycle

Status: accepted; implemented in Rust and Pi, not activated. This changes the automatic-opening and close behaviour in [observe and act](native-observe-and-act.md). Its observation, naming and duplicate-exclusion rules still apply.

## One owner for card behaviour

A running Pi conversation can become attached after startup. The child already observes the new Session, but currently opens cards only at Pi startup. Opening on every observation would fix attachment while also undoing deliberate closes. Making Pi persist a preference would require a card's `q` action to travel through the child to Pi and back again.

Streams will own card preferences as well as native actions. The Pi extension will own Pi's interface and report generation. It will submit reports, open/close commands and preference edits to the child; it will not store another copy of card preferences or decide when the native card needs updating. Its settings screen will read and edit the same Streams-owned values. Only the Rust child will access card configuration: it owns the paths, file format, reads and writes. Pi and the card will use the same typed application interface over their different connections (Pi's existing stdio connection and the card's local socket). Neither will read or write the preference files directly. There is no preference reconciliation between Pi and Streams.

The card remains a separate terminal process. The child must outlive a closed card, and Herdr launches a process in each card terminal. Merging their operating-system processes would require a new terminal transport. Instead, the card will send explicit close intent directly to the child over a private local socket. This is a narrow action channel, not a general remote-control service. The endpoint will be supplied in launch inputs, never discovered by scanning processes or panes.

## Preferences, not launch history

The global automatic-opening default will live in Streams config (`cards.auto_open`), defaulting to `automatic`. A conversation override will be `inherit`, `automatic` or `manual`, defaulting to `inherit`. Streams will store each override in its catalog-scoped host-local state, keyed by the conversation's Session identity, including before attachment. It will survive child restarts, Pi reloads and resumed conversations, without following the Pi transcript branch or synchronizing to another host.

An explicit override wins; otherwise the global default applies. Each conversation has its own file so independent children do not rewrite a shared session map. The file describes only the preference. It contains no pane addresses, observed presence, attempted launches or pending actions.

The old `workspace_startup` key in Pi's `work-cards.json` will be replaced by the Streams setting, not read as a fallback. Activation must remove that retired key and set the desired Streams default explicitly. Other native presentation settings are outside this decision.

## Opening and closing

Automatic opening will run at Pi startup and when an already-observing child's bound Session changes, only when the effective preference is automatic. A child restart establishes its binding baseline without treating that as a new Pi startup. Ordinary catalog/native refreshes continue projecting and following existing cards, but never reopen a missing card.

An explicit `/streams pane` will save the conversation override as `automatic`, then open or reuse its card. Both `/streams pane close` and a card's `q` will save `manual`. Explicit close will close the observed card; `q` will notify the child and exit its own terminal UI. These actions change only that conversation's preference, not the global default or catalog membership.

Herdr closure, a signal, a crash and observed owner loss are not `q` intent and will not change the preference. They still leave the card absent until a subsequent automatic-opening trigger or explicit open. No presence-to-closure inference or reciprocal marker is needed.

The child will save the preference before completing a close request. The card's `q` request will have a bounded wait and no retry. If the child is unavailable or saving fails, the card will still exit and print a warning that manual mode may not have been saved. A successful preference save is not undone by a later native-action failure. Such failures must remain visible rather than pretending the entire operation rolled back.

The socket will be scoped to the same conversation as the child, with a stable endpoint across child restarts so an existing card can still submit `q`. A process lock will exclude simultaneous endpoint owners; stale socket replacement will happen only under that lock. This is endpoint lifetime exclusion, not a launch journal or a new daemon.

## Show the boundaries in tests

Scripted fake-Herdr scenarios will cover outside attachment, unchanged startup behaviour, manual/global/inherited policies, unrelated attachments, binding-change opening and repeated refreshes. They will demonstrate that explicit opening re-enables automatic mode, explicit close and `q` persist manual mode, and restart/resume uses the saved preference. Real terminal tests will distinguish `q` from signals and verify that unavailable control does not prevent quitting. Preference and endpoint tests will cover separate conversations, malformed input, failed saves and endpoint ownership/restart without live catalog or Herdr access.
