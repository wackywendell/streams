import { readFileSync } from "node:fs";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, formatSize, truncateHead } from "@earendil-works/pi-coding-agent";
import type { ExtensionContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { latestBinding, readPending } from "./core.ts";
import type { StreamCli } from "./streams.ts";

const parameters = Type.Object({
	stream: Type.Optional(Type.String({ description: "Stream slug, path, or title. Defaults to the stream bound to this session." })),
	offset: Type.Optional(Type.Number({ description: "Line number to start reading from (1-indexed)" })),
	limit: Type.Optional(Type.Number({ description: "Maximum number of lines to read" })),
	draft: Type.Optional(Type.Boolean({ description: "Read this session's pending, unapplied checkpoint draft instead of the note. Default false." })),
});

/** Select lines exactly as Pi's read tool does, so offset/limit continuation notices behave the same. */
export function selectLines(text: string, path: string, offset?: number, limit?: number): string {
	const lines = text.split("\n");
	const start = offset ? Math.max(0, offset - 1) : 0;
	if (start >= lines.length) throw new Error(`Offset ${offset} is beyond end of file (${lines.length} lines total)`);
	const end = limit === undefined ? lines.length : Math.min(start + limit, lines.length);
	const truncation = truncateHead(lines.slice(start, end).join("\n"));
	if (truncation.firstLineExceedsLimit) {
		return `[Line ${start + 1} is ${formatSize(Buffer.byteLength(lines[start], "utf-8"))}, exceeds ${formatSize(DEFAULT_MAX_BYTES)} limit. Use bash: sed -n '${start + 1}p' ${path} | head -c ${DEFAULT_MAX_BYTES}]`;
	}
	if (truncation.truncated) {
		const last = start + truncation.outputLines;
		const limitNote = truncation.truncatedBy === "lines" ? "" : ` (${formatSize(DEFAULT_MAX_BYTES)} limit)`;
		return `${truncation.content}\n\n[Showing lines ${start + 1}-${last} of ${lines.length}${limitNote}. Use offset=${last + 1} to continue.]`;
	}
	if (end < lines.length) return `${truncation.content}\n\n[${lines.length - end} more lines in file. Use offset=${end + 1} to continue.]`;
	return truncation.content;
}

export function streamReadTool(agentDir: string, cliFor: (ctx: ExtensionContext) => Pick<StreamCli, "show">): ToolDefinition<typeof parameters, { slug: string; path: string; draftRevision?: number }> {
	return {
		name: "stream_read",
		label: "Read stream",
		description: `Read a stream's scratch note without a path-access prompt. Defaults to the stream bound to this session by /checkpoint. Returns the resolved path, title, status, state and updated date, then the note's lines. Output is truncated to ${DEFAULT_MAX_LINES} lines or ${DEFAULT_MAX_BYTES / 1024}KB; use offset/limit to continue.`,
		promptSnippet: "Read this session's stream scratch note, or another stream by ref",
		promptGuidelines: ["Use stream_read rather than read to view stream scratch notes."],
		parameters,
		annotations: { readOnlyHint: true },
		async execute(_id, { stream: ref, offset, limit, draft }, _signal, _onUpdate, ctx) {
			const cli = cliFor(ctx);
			const ownerId = ctx.sessionManager.getSessionId();
			let stream: Awaited<ReturnType<StreamCli["show"]>>;
			if (ref?.trim()) stream = await cli.show(ref.trim());
			else {
				const binding = latestBinding(ctx.sessionManager.getEntries(), ownerId);
				if (!binding) throw new Error("No stream is bound to this session. Pass `stream` (slug, path, or title); run `streams list` to find one.");
				stream = await cli.show(binding.slug);
				if (stream.path !== binding.path) throw new Error(`Bound stream ${binding.slug} moved from ${binding.path} to ${stream.path}; pass \`stream\` explicitly.`);
			}
			const header = [`Stream: ${stream.slug}`, `Path: ${stream.path}`, `Title: ${stream.title}`, `Status: ${stream.status}`, `State: ${stream.state}`];
			if (typeof stream.updated === "string") header.push(`Updated: ${stream.updated}`);
			let text: string;
			let draftRevision: number | undefined;
			if (draft) {
				const pending = readPending(agentDir, ownerId, stream.slug)?.draft;
				if (!pending) throw new Error(`No pending checkpoint draft for ${stream.slug} in this session.`);
				text = pending.proposedContent;
				draftRevision = pending.revision;
				header.push(`Draft: revision ${pending.revision}, pending review and not applied. Metadata above is from the applied note.`);
			} else text = readFileSync(stream.path, "utf8");
			return {
				content: [{ type: "text", text: `${header.join("\n")}\n\n${selectLines(text, stream.path, offset, limit)}` }],
				details: { slug: stream.slug, path: stream.path, ...(draftRevision ? { draftRevision } : {}) },
			};
		},
	};
}
