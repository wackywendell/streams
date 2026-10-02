import { spawn } from "node:child_process";

type Stream = { slug: string; title: string; status: string; state: string; path: string; [key: string]: unknown };
type UpdateResult = { slug: string; path: string; updated: boolean; findings: unknown[] };
type CommandResult = { code: number | null; stdout: string; stderr: string };
type Runner = (args: string[], input?: string) => Promise<CommandResult>;
export type CommandDiagnostic = CommandResult & { args: string[]; warnings?: string[] };
export type StreamsConfig = { notes: { root: string }; obsidian: { vault: string }; cmux: { enabled: boolean } };

/** A failed CLI command; `kind` comes from the CLI's `--json` error, when it printed one. */
export class StreamsCliError extends Error {
	constructor(message: string, readonly kind?: string, readonly candidates: string[] = []) { super(message); }
}

function jsonError(stdout: string): { kind: string; message: string; candidates?: unknown } | undefined {
	try {
		const error = JSON.parse(stdout)?.error;
		return typeof error?.kind === "string" && typeof error.message === "string" ? error : undefined;
	} catch { return undefined; }
}

function bounded(text: string): string { return text.length > 2048 ? `${text.slice(0, 2048)}\n[truncated]` : text; }

function isStream(value: unknown): value is Stream {
	const stream = value as Partial<Stream> | null;
	return !!stream && typeof stream.slug === "string" && typeof stream.title === "string" && typeof stream.status === "string" && typeof stream.state === "string" && typeof stream.path === "string";
}

function defaultRunner(cwd: string): Runner {
	return (args, input) => new Promise((resolve, reject) => {
		const child = spawn("streams", args, { cwd, stdio: ["pipe", "pipe", "pipe"] });
		let stdout = "";
		let stderr = "";
		child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
		child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
		child.on("error", reject);
		child.on("close", (code) => resolve({ code, stdout, stderr }));
		child.stdin.on("error", (error: NodeJS.ErrnoException) => { if (error.code !== "EPIPE") reject(error); });
		child.stdin.end(input);
	});
}

export class StreamCli {
	readonly diagnostics: CommandDiagnostic[] = [];
	private readonly runner: Runner;
	constructor(cwd: string, runner?: Runner, private warning?: (text: string) => void) { this.runner = runner ?? defaultRunner(cwd); }

	/** `okCodes` lists the exit codes that still mean success, such as `list` exiting 1 to report invalid notes. */
	private async run(args: string[], input?: string, okCodes = [0]): Promise<string> {
		let result: CommandResult;
		try { result = await this.runner(args, input); }
		catch (error) { result = { code: null, stdout: "", stderr: String(error) }; }
		this.diagnostics.push({ args: [...args], code: result.code, stdout: bounded(result.stdout), stderr: bounded(result.stderr) });
		if (this.diagnostics.length > 16) this.diagnostics.shift();
		if (result.code === null || !okCodes.includes(result.code)) {
			const error = jsonError(result.stdout);
			const detail = error?.message ?? (result.stderr.trim() || result.stdout.trim() || "No output");
			const candidates = Array.isArray(error?.candidates) ? error.candidates.filter((item): item is string => typeof item === "string") : [];
			throw new StreamsCliError(`streams ${args[0]} (exit ${result.code ?? "unavailable"}): ${bounded(detail)}`, error?.kind, candidates);
		}
		if (result.stderr.trim()) this.warning?.(bounded(result.stderr.trim()));
		return result.stdout;
	}

	async config(): Promise<StreamsConfig> {
		const value = JSON.parse(await this.run(["config", "--json"]));
		if (typeof value?.notes?.root !== "string" || !value.notes.root || typeof value?.obsidian?.vault !== "string" || !value.obsidian.vault || typeof value?.cmux?.enabled !== "boolean") throw new Error("Invalid streams config response");
		return value;
	}

	async list(): Promise<Stream[]> {
		const value = JSON.parse(await this.run(["list", "--json"], undefined, [0, 1]));
		if (!Array.isArray(value?.streams) || !value.streams.every(isStream) || !Array.isArray(value.findings)) throw new Error("Invalid streams list response");
		const invalid = [...new Set(value.findings.filter((finding: { level?: unknown }) => finding?.level === "error").map((finding: { slug?: unknown }) => String(finding.slug)))];
		if (invalid.length) this.warning?.(`Skipped ${invalid.length} invalid stream note${invalid.length === 1 ? "" : "s"}: ${invalid.join(", ")}. Run \`streams check\` for details.`);
		return value.streams;
	}

	async show(ref: string): Promise<Stream> {
		const value = JSON.parse(await this.run(["show", ref, "--json"]));
		if (!isStream(value)) throw new Error("Invalid streams show response");
		return value;
	}

	async previewStart(title: string): Promise<Stream> {
		const value = JSON.parse(await this.run(["start", title, "--dry-run", "--json"]));
		if (!isStream(value)) throw new Error("Invalid streams start preview response");
		return value;
	}

	async start(title: string): Promise<Stream> {
		const value = JSON.parse(await this.run(["start", title, "--json"]));
		if (!isStream(value)) throw new Error("Invalid streams start response");
		return value;
	}

	async update(slug: string, content: string, options: { dryRun?: boolean; ifMatch?: string } = {}): Promise<UpdateResult> {
		const args = ["update", slug];
		if (options.dryRun) args.push("--dry-run");
		if (options.ifMatch) args.push("--if-match", options.ifMatch);
		args.push("--json");
		const value = JSON.parse(await this.run(args, content));
		if (typeof value?.slug !== "string" || typeof value.path !== "string" || typeof value.updated !== "boolean" || !Array.isArray(value.findings)) {
			throw new Error("Invalid streams update response");
		}
		const warnings = value.findings.filter((finding: { level?: string; message?: string }) => finding?.level === "warning" && typeof finding.message === "string").slice(0, 16).map((finding: { message: string }) => bounded(finding.message));
		this.diagnostics.at(-1)!.warnings = warnings;
		for (const warning of warnings) this.warning?.(warning);
		return value;
	}

	async project(slug: string): Promise<void> { await this.run(["cmux", "update", "-c", slug]); }
	async format(): Promise<string> { return this.run(["format"]); }
}
