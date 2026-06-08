# Single-Stream Agent Policy

Use this reference for a stream-specific AI session, a stream CMUX workspace, or a command with an explicit stream selector.

Read [core.md](core.md), [scratch.md](scratch.md), and [cmux-integration.md](cmux-integration.md) when CMUX is involved.

## Scope

Single-stream scope means one scratch file and its associated local workspace, AI sessions, links, and CMUX projection.

The current session is responsible for keeping its stream's scratch frontmatter and body accurate enough for a new human or AI session to resume.

## Commands

`status` means: read the scratch file and recap the currently recorded state. Do not check external systems unless asked.

`check` and `refresh` mean: inspect external or local state relevant to the stream, update scratch and CMUX if reality changed, and recap.

`update`, `checkpoint`, and `handoff` mean: make the current stream durable and resumable.

A checkpoint/handoff should:

1. Update frontmatter fields when current state changed.
2. Update `updated`.
3. Refresh `## Status` with current pickup context.
4. Add or revise decisions/open questions when important reasoning changed.
5. Update CMUX projection when open.
6. Recap current state and uncertainties.

`cleanup` performs final stream completion/archive/retro behavior. See [cleanup.md](cleanup.md).

`split off` creates a separate related stream without switching the current session. Use `streams start "Title"`, fill in frontmatter/body, and leave its `sessions` empty unless a dedicated session starts for it.

`open`, `switch`, and `resume` explicitly change the current stream. They create/select the CMUX workspace, read scratch, and resume the recorded session when one exists. When no session exists, start a fresh AI session and let that new session record itself in `sessions` once known.

## Recap Style

Single-stream recaps should use three short sections:

1. `Overall`: broad overview of the stream.
2. `Just completed`: what changed now.
3. `Still to do`: remaining concrete work, blockers, or verification.

End with one or two sentences stating the current/next step.

## Work Loop

1. Read the scratch file.
2. Work the current state.
3. Before replying, consider whether frontmatter, `## Status`, decisions, open questions, or CMUX projection changed.
4. If changed, update scratch/CMUX.
5. Recap current state and next transition.
