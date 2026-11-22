import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { streamKeyOf, unreadableNotice, type StreamCli } from "./streams.ts";

/** Only initial enrollment offers membership selection. No native focus or workspace calls. */
export async function chooseInitial(
  ctx: ExtensionContext,
  cli: StreamCli,
  options: { streamId?: string; title?: string; reason?: string },
  onEnrolled?: (info: { streamKey: string; sessionKey: string; sessionTitle: string }) => void,
  signal?: AbortSignal
): Promise<{ status: string; stream_id?: string; session_key?: string }> {
  const native = ctx.sessionManager.getSessionId();
  const before = await cli.context(native, signal);
  if (before.session) {
    return {
      status: "already-bound; use streams session edit --stream to move a Session",
      stream_id: before.session.stream_id,
    };
  }
  if (!ctx.hasUI) throw new Error("Initial enrollment needs an interactive UI");

  let streamKey: string | undefined;
  let title = options.title;

  const listing = await cli.listStreams({ all: true, signal });
  const streams = listing.items;
  const unreadable = unreadableNotice(listing.unreadable);
  if (options.streamId) {
    const matched = streams.find(
      (s) => s.id === options.streamId || streamKeyOf(s) === options.streamId
    );
    if (!matched) {
      throw new Error(
        `Pass an exact catalog stream ID from streams stream list --json${unreadable ? `\n${unreadable}` : ""}`
      );
    }
    streamKey = streamKeyOf(matched);
  }

  const rows = streams.map((stream) => `${stream.id} · ${stream.title.text}`);
  const chosen = title
    ? "Create…"
    : await ctx.ui.select(
        `Initial work card attachment${options.reason ? `\n${options.reason}` : ""}${unreadable ? `\n${unreadable}` : ""}`,
        [
          ...(options.streamId ? rows.filter((row) => row.startsWith(`${options.streamId} ·`)) : rows),
          "Create…",
          "Cancel",
        ]
      );
  if (!chosen || chosen === "Cancel" || signal?.aborted) return { status: "cancelled" };

  if (chosen === "Create…") {
    title = await ctx.ui.editor("Stream title", title ?? "");
    if (!title?.trim()) return { status: "cancelled" };
    streamKey = undefined;
  } else {
    const selected = streams.find((s) => chosen === `${s.id} · ${s.title.text}`);
    if (!selected) return { status: "cancelled" };
    streamKey = streamKeyOf(selected);
  }

  const sessionTitle = await ctx.ui.editor("This session title", "Pi contribution");
  if (!sessionTitle?.trim() || signal?.aborted) return { status: "cancelled" };
  if (
    !(await ctx.ui.confirm(
      "Enroll this conversation?",
      `Stream: ${title ?? streamKey}\nSession: ${sessionTitle}`
    ))
  ) {
    return { status: "cancelled" };
  }
  if (ctx.sessionManager.getSessionId() !== native || signal?.aborted) {
    return { status: "cancelled" };
  }

  if (!streamKey) {
    const createdStream = await cli.createStream(
      { title: title!.trim(), titleMode: "manual" },
      signal
    );
    streamKey = createdStream.key;
  }
  const createdSession = await cli.createSession(
    native,
    {
      stream: streamKey,
      title: sessionTitle.trim(),
      titleMode: "manual",
      directory: ctx.cwd,
    },
    signal
  );

  try {
    await cli.writeSessionLabel(createdSession.key, signal, streamKey);
  } catch {
    // Label write failure is non-fatal
  }
  onEnrolled?.({
    streamKey,
    sessionKey: createdSession.key,
    sessionTitle: sessionTitle.trim(),
  });
  return {
    status: "enrolled",
    stream_id: streamKey.split("/").slice(1).join("/") || streamKey,
    session_key: createdSession.key,
  };
}
