import { existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import type { KeyId } from "@earendil-works/pi-tui";
import { StreamsCliError, type Note, type StreamCli } from "./streams.ts";
import type { Annotation, AnnotationEditor, Feedback } from "./review.ts";
import { isSeq, parseDocument } from "yaml";

export const BINDING_TYPE = "stream-checkpoint-binding";

export interface Binding {
	ownerId: string;
	slug: string;
	/** The note's path when bound, compared only to notice a renamed stream; Pi reads notes through the CLI. */
	path: string;
}

export interface Draft {
	slug: string;
	revision: number;
	baseContent: string;
	proposedContent: string;
	summary?: string;
}

export interface CheckpointConfig {
	focusShortcut: KeyId;
	/** Register the main-agent checkpoint tool. */
	agentTools: boolean;
	selectorModel?: { provider: string; id: string };
	drafterModel?: { provider: string; id: string };
}

export function loadConfig(agentDir: string): CheckpointConfig {
	const path = join(agentDir, "checkpoint.json");
	if (!existsSync(path)) return { focusShortcut: "alt+p", agentTools: true };
	const config = JSON.parse(readFileSync(path, "utf8"));
	if (config === null || typeof config !== "object" || Array.isArray(config)) throw new Error("checkpoint.json must be an object");
	if (config.cmux !== undefined) throw new Error("checkpoint.json: cmux moved to streams; use streams config set cmux.enabled true (or false), then remove cmux from checkpoint.json");
	if (config.focusShortcut !== undefined && (typeof config.focusShortcut !== "string" || !config.focusShortcut.trim())) throw new Error("checkpoint.json: focusShortcut must be a nonempty Pi keybinding");
	if (config.agentTools !== undefined && typeof config.agentTools !== "boolean") throw new Error("checkpoint.json: agentTools must be true or false");
	for (const key of ["selectorModel", "drafterModel"] as const) {
		const model = config[key];
		if (model !== undefined && (model === null || typeof model !== "object" || Array.isArray(model) || typeof model.provider !== "string" || !model.provider.trim() || typeof model.id !== "string" || !model.id.trim())) {
			throw new Error(`checkpoint.json: ${key} must contain nonempty provider and id strings`);
		}
	}
	return { focusShortcut: (config.focusShortcut ?? "alt+p") as KeyId, agentTools: config.agentTools ?? true,
		...(config.selectorModel ? { selectorModel: config.selectorModel } : {}),
		...(config.drafterModel ? { drafterModel: config.drafterModel } : {}) };
}

export function latestBinding(entries: readonly { type: string; customType?: string; data?: unknown }[], ownerId: string): Binding | undefined {
	for (let i = entries.length - 1; i >= 0; i--) {
		const entry = entries[i];
		if (entry.type !== "custom" || entry.customType !== BINDING_TYPE) continue;
		const data = entry.data as Partial<Binding> | undefined;
		if (data?.ownerId === ownerId && typeof data.slug === "string" && typeof data.path === "string") return data as Binding;
	}
	return undefined;
}

export async function selectStream(ref: string, binding: Binding | undefined, cli: Pick<StreamCli, "list" | "show">): Promise<Awaited<ReturnType<StreamCli["list"]>>[number] | undefined> {
	const streams = await cli.list();
	if (ref.trim()) return cli.show(ref.trim());
	return binding ? streams.find((stream) => stream.slug === binding.slug && stream.path === binding.path) : undefined;
}

/**
 * The stream whose slug is exactly `slug`, for tools that must not guess. A title, prefix or unknown
 * slug fails, naming the streams it matched or the slugs containing it; other failures, such as a broken note, pass through.
 */
export async function exactStream(slug: string, cli: Pick<StreamCli, "list" | "show">): Promise<Awaited<ReturnType<StreamCli["show"]>>> {
	let candidates: string[] = [];
	try {
		const stream = await cli.show(slug);
		if (stream.slug === slug) return stream;
		candidates = [stream.slug];
	} catch (error) {
		if (!(error instanceof StreamsCliError && ["not-found", "ambiguous", "invalid-ref"].includes(error.kind ?? ""))) throw error;
		candidates = error.candidates;
	}
	if (!candidates.length) candidates = (await cli.list()).map((stream) => stream.slug).filter((candidate) => candidate.includes(slug));
	const similar = candidates.length ? ` Close matches: ${candidates.slice(0, 10).join(", ")}.` : "";
	throw new Error(`No stream has the slug "${slug}".${similar} Pass an exact slug from \`streams list\`, or call create_stream if no stream fits.`);
}

export interface Recommendations {
	existing: Array<{ slug: string; reason: string }>;
	newStream?: { title: string; reason: string };
}

export function parseRecommendations(text: string, streams: Array<{ slug: string }>): Recommendations {
	let value: any;
	try { value = JSON.parse(text); } catch { return { existing: [] }; }
	const known = new Set(streams.map((stream) => stream.slug));
	const seen = new Set<string>();
	const existing = (Array.isArray(value?.existing) ? value.existing : []).filter((item: any) => {
		if (typeof item?.slug !== "string" || !known.has(item.slug) || seen.has(item.slug) || typeof item.reason !== "string") return false;
		seen.add(item.slug);
		return true;
	}).map((item: { slug: string; reason: string }) => ({ slug: item.slug, reason: item.reason.slice(0, 160) }));
	const proposal = value?.newStream;
	const title = typeof proposal?.title === "string" ? proposal.title.trim() : "";
	const newStream = title && typeof proposal.reason === "string"
		? { title, reason: proposal.reason.slice(0, 160) } : undefined;
	return newStream ? { existing, newStream } : { existing };
}

export function includeMainSession(content: string, ownerId: string): string {
	const frontmatter = /^---\r?\n([\s\S]*?)\r?\n---(?=\r?\n|$)/.exec(content);
	if (!frontmatter) throw new Error("Proposal must contain YAML frontmatter");
	const document = parseDocument(frontmatter[1]);
	if (document.errors.length) throw new Error(`Invalid proposal frontmatter: ${document.errors[0].message}`);
	const sessions = document.get("sessions", true);
	if (!isSeq(sessions)) throw new Error("Proposal frontmatter must contain a sessions list");
	const session = `pi:${ownerId}`;
	if ((sessions.toJSON() as unknown[]).includes(session)) return content;
	sessions.add(session);
	return `---\n${document.toString().trimEnd()}\n---${content.slice(frontmatter[0].length)}`;
}

export interface Pending {
	version: 1;
	ownerId: string;
	slug: string;
	messages: unknown[];
	draft?: Draft;
	source?: { ownerId: string; leafId: string | null };
	input: string;
	annotations?: Annotation[];
	annotationEditor?: AnnotationEditor;
	feedback?: Feedback[];
}

function pendingPath(agentDir: string, ownerId: string, slug: string): string {
	const key = createHash("sha256").update(JSON.stringify([ownerId, slug])).digest("hex");
	return join(agentDir, "checkpoints", `${key}.json`);
}

export function readPending(agentDir: string, ownerId: string, slug: string): Pending | undefined {
	const path = pendingPath(agentDir, ownerId, slug);
	if (!existsSync(path)) return undefined;
	const data = JSON.parse(readFileSync(path, "utf8"));
	if (data?.version !== 1 || data.ownerId !== ownerId || data.slug !== slug || !Array.isArray(data.messages) || typeof data.input !== "string") {
		throw new Error(`Invalid checkpoint state: ${path}`);
	}
	return data;
}

export function savePending(agentDir: string, pending: Pending): void {
	const path = pendingPath(agentDir, pending.ownerId, pending.slug);
	mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
	const temporary = `${path}.${randomUUID()}.tmp`;
	try {
		writeFileSync(temporary, JSON.stringify(pending), { flag: "wx", mode: 0o600 });
		renameSync(temporary, path);
	} finally {
		if (existsSync(temporary)) unlinkSync(temporary);
	}
}

export function clearPending(agentDir: string, ownerId: string, slug: string): void {
	const path = pendingPath(agentDir, ownerId, slug);
	if (existsSync(path)) unlinkSync(path);
}

/** `baseContent` is the note as read through the CLI when the draft was proposed. */
export function stageDraft(pending: Pending, binding: Binding, content: string, summary: string, baseContent: string): Draft {
	if (pending.ownerId !== binding.ownerId || pending.slug !== binding.slug) throw new Error("Checkpoint does not belong to this session and stream");
	const draft: Draft = {
		slug: binding.slug,
		revision: (pending.draft?.revision ?? 0) + 1,
		baseContent,
		proposedContent: content,
		summary,
	};
	return draft;
}

export interface ApplyDependencies {
	read(slug: string): Promise<Note>;
	update(slug: string, content: string, options: { dryRun?: boolean; ifMatch: string }): Promise<{ slug: string; updated: boolean }>;
	project?(): Promise<void>;
}

/**
 * Apply only the UI-approved revision; the CLI owns reading, hashing, validation and writing.
 * `--if-match` with the hash of the note as drafted makes the CLI refuse a write over newer changes.
 */
export async function applyDraft(draft: Draft, approvedRevision: number, deps: ApplyDependencies): Promise<{ written: true; validationError?: string; projectionWarning?: string }> {
	if (draft.revision !== approvedRevision) throw new Error("Draft revision differs from the approved revision");
	const current = await deps.read(draft.slug);
	if (current.content === draft.proposedContent) throw new Error("Draft already applied; do not repeat the write");
	if (current.content !== draft.baseContent) throw new Error("Scratch changed since this draft was proposed");
	const ifMatch = current.sha256;
	const preview = await deps.update(draft.slug, draft.proposedContent, { dryRun: true, ifMatch });
	if (preview.slug !== draft.slug || preview.updated) throw new Error("Dry-run did not confirm the draft target");
	try {
		const applied = await deps.update(draft.slug, draft.proposedContent, { ifMatch });
		if (applied.slug !== draft.slug || !applied.updated) throw new Error("CLI did not confirm the update");
	} catch (error) {
		if ((await deps.read(draft.slug).catch(() => undefined))?.content === draft.proposedContent) {
			return { written: true, validationError: `Update confirmation failed: ${String(error)}` };
		}
		throw error;
	}
	try {
		await deps.project?.();
	} catch (error) {
		return { written: true, projectionWarning: String(error) };
	}
	return { written: true };
}
