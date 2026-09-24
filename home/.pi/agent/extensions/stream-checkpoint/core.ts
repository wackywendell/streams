import { existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createHash, randomUUID } from "node:crypto";

export const BINDING_TYPE = "stream-checkpoint-binding";

export interface Binding {
	ownerId: string;
	slug: string;
	path: string;
}

export interface Draft {
	path: string;
	slug: string;
	revision: number;
	baseContent: string;
	proposedContent: string;
}

export function loadConfig(agentDir: string): { cmux: boolean } {
	const path = join(agentDir, "checkpoint.json");
	if (!existsSync(path)) return { cmux: false };
	const config = JSON.parse(readFileSync(path, "utf8"));
	if (config === null || typeof config !== "object" || Array.isArray(config)) throw new Error("checkpoint.json must be an object");
	if (config.cmux !== undefined && typeof config.cmux !== "boolean") throw new Error("checkpoint.json: cmux must be a boolean");
	return { cmux: config.cmux === true };
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

export interface Pending {
	version: 1;
	ownerId: string;
	slug: string;
	messages: unknown[];
	draft?: Draft;
	input: string;
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

export interface ApplyDependencies {
	update(slug: string, content: string, options: { dryRun?: boolean; ifMatch: string }): Promise<{ path: string; updated: boolean }>;
	project?(): Promise<void>;
}

/** Apply only the UI-approved revision; the CLI owns validation and writing. */
export async function applyDraft(draft: Draft, approvedRevision: number, deps: ApplyDependencies): Promise<{ written: true; validationError?: string; projectionWarning?: string }> {
	if (draft.revision !== approvedRevision) throw new Error("Draft revision differs from the approved revision");
	const current = readFileSync(draft.path, "utf8");
	if (current === draft.proposedContent) throw new Error("Draft already applied; do not repeat the write");
	if (current !== draft.baseContent) throw new Error("Scratch changed since this draft was proposed");
	const ifMatch = createHash("sha256").update(draft.baseContent).digest("hex");
	const preview = await deps.update(draft.slug, draft.proposedContent, { dryRun: true, ifMatch });
	if (preview.path !== draft.path || preview.updated) throw new Error("Dry-run did not confirm the draft target");
	if (readFileSync(draft.path, "utf8") !== draft.baseContent) throw new Error("Scratch changed since this draft was proposed");
	try {
		const applied = await deps.update(draft.slug, draft.proposedContent, { ifMatch });
		if (applied.path !== draft.path || !applied.updated) throw new Error("CLI did not confirm the update");
	} catch (error) {
		if (readFileSync(draft.path, "utf8") === draft.proposedContent) {
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
