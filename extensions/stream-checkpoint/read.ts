import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, formatSize, truncateHead } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { latestBinding, readPending } from "./core.ts";
import { StreamsCliError, type StreamCli } from "./streams.ts";

const parameters = Type.Object({
	stream: Type.Optional(Type.String({ description: "Stream slug or title. Defaults to the stream bound to this session." })),
	offset: Type.Optional(Type.Number({ description: "Line number to start reading from (1-indexed)" })),
	limit: Type.Optional(Type.Number({ description: "Maximum number of lines to read" })),
	draft: Type.Optional(Type.Boolean({ description: "Read this session's pending, unapplied checkpoint draft instead of the note. Default false." })),
});

/** Select lines exactly as Pi's read tool does, so offset/limit continuation notices behave the same. */
export function selectLines(text: string, slug: string, offset?: number, limit?: number): string {
	const lines = text.split("\n");
	const start = offset ? Math.max(0, offset - 1) : 0;
	if (start >= lines.length) throw new Error(`Offset ${offset} is beyond end of file (${lines.length} lines total)`);
	const end = limit === undefined ? lines.length : Math.min(start + limit, lines.length);
	const truncation = truncateHead(lines.slice(start, end).join("\n"));
	if (truncation.firstLineExceedsLimit) {
		return `[Line ${start + 1} is ${formatSize(Buffer.byteLength(lines[start], "utf-8"))}, exceeds ${formatSize(DEFAULT_MAX_BYTES)} limit. Use bash: streams read ${slug} | sed -n '${start + 1}p' | head -c ${DEFAULT_MAX_BYTES}]`;
	}
	if (truncation.truncated) {
		const last = start + truncation.outputLines;
		const limitNote = truncation.truncatedBy === "lines" ? "" : ` (${formatSize(DEFAULT_MAX_BYTES)} limit)`;
		return `${truncation.content}\n\n[Showing lines ${start + 1}-${last} of ${lines.length}${limitNote}. Use offset=${last + 1} to continue.]`;
	}
	if (end < lines.length) return `${truncation.content}\n\n[${lines.length - end} more lines in file. Use offset=${end + 1} to continue.]`;
	return truncation.content;
}

export function streamReadTool(agentDir: string, cliFor: (ctx: ExtensionContext) => Pick<StreamCli, "show" | "read">): ToolDefinition<typeof parameters, { slug: string; draftRevision?: number }> {
	return {
		name: "stream_read",
		label: "Read stream",
		description: `Read a stream's scratch note without a path-access prompt. Defaults to the stream bound to this session by /checkpoint. Returns the stream's slug, title, status, state and updated date, then the note's lines. An invalid note is still shown, with its problems. Output is truncated to ${DEFAULT_MAX_LINES} lines or ${DEFAULT_MAX_BYTES / 1024}KB; use offset/limit to continue.`,
		promptSnippet: "Read this session's stream scratch note, or another stream by ref",
		promptGuidelines: ["Use stream_read rather than read to view stream scratch notes."],
		parameters,
		annotations: { readOnlyHint: true },
		async execute(_id, { stream: ref, offset, limit, draft }, _signal, _onUpdate, ctx) {
			const cli = cliFor(ctx);
			const ownerId = ctx.sessionManager.getSessionId();
			const binding = ref?.trim() ? undefined : latestBinding(ctx.sessionManager.getEntries(), ownerId);
			if (!ref?.trim() && !binding) throw new Error("No stream is bound to this session. Pass `stream` (slug or title); run `streams list` to find one.");
			let slug = binding?.slug ?? ref!.trim();
			const header: string[] = [];
			try {
				const stream = await cli.show(slug);
				if (binding && stream.path !== binding.path) throw new Error(`Bound stream ${binding.slug} was renamed or removed; pass \`stream\` explicitly.`);
				slug = stream.slug;
				header.push(`Stream: ${stream.slug}`, `Title: ${stream.title}`, `Status: ${stream.status}`, `State: ${stream.state}`);
				if (typeof stream.updated === "string") header.push(`Updated: ${stream.updated}`);
			} catch (error) {
				// `streams read` still shows an invalid note, so the agent can see what to fix.
				if (!(error instanceof StreamsCliError && error.kind === "invalid-stream")) throw error;
				header.push(`Stream: ${slug}`);
			}
			let text: string;
			let draftRevision: number | undefined;
			if (draft) {
				const pending = readPending(agentDir, ownerId, slug)?.draft;
				if (!pending) throw new Error(`No pending checkpoint draft for ${slug} in this session.`);
				text = pending.proposedContent;
				draftRevision = pending.revision;
				header.push(`Draft: revision ${pending.revision}, pending review and not applied. Metadata above is from the applied note.`);
			} else {
				const note = await cli.read(slug);
				text = note.content;
				if (note.findings) header.push(`Invalid note:\n${note.findings}\nTo fix it, pipe a corrected note to \`streams update ${slug} --if-match ${note.sha256}\`.`);
			}
			return {
				content: [{ type: "text", text: `${header.join("\n")}\n\n${selectLines(text, slug, offset, limit)}` }],
				details: { slug, ...(draftRevision ? { draftRevision } : {}) },
			};
		},
	};
}
