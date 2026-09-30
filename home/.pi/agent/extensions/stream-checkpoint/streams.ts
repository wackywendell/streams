import { spawn } from "node:child_process";

type Stream = { slug: string; title: string; status: string; state: string; path: string; [key: string]: unknown };
type UpdateResult = { slug: string; path: string; updated: boolean; findings: unknown[] };
type CommandResult = { code: number | null; stdout: string; stderr: string };
type Runner = (args: string[], input?: string) => Promise<CommandResult>;
export type CommandDiagnostic = CommandResult & { args: string[]; warnings?: string[] };
export type StreamsConfig = { notes: { root: string }; obsidian: { vault: string }; cmux: { enabled: boolean } };

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

	private async run(args: string[], input?: string): Promise<string> {
		let result: CommandResult;
		try { result = await this.runner(args, input); }
		catch (error) { result = { code: null, stdout: "", stderr: String(error) }; }
		this.diagnostics.push({ args: [...args], code: result.code, stdout: bounded(result.stdout), stderr: bounded(result.stderr) });
		if (this.diagnostics.length > 16) this.diagnostics.shift();
		if (result.code !== 0) throw new Error(`streams ${args[0]} (exit ${result.code ?? "unavailable"}): ${bounded(result.stderr.trim() || result.stdout.trim() || "No output")}`);
		if (result.stderr.trim()) this.warning?.(bounded(result.stderr.trim()));
		return result.stdout;
	}

	async config(): Promise<StreamsConfig> {
		const value = JSON.parse(await this.run(["config", "--json"]));
		if (typeof value?.notes?.root !== "string" || !value.notes.root || typeof value?.obsidian?.vault !== "string" || !value.obsidian.vault || typeof value?.cmux?.enabled !== "boolean") throw new Error("Invalid streams config response");
		return value;
	}

	async list(): Promise<Stream[]> {
		const value = JSON.parse(await this.run(["list", "--json"]));
		if (!Array.isArray(value) || !value.every(isStream)) throw new Error("Invalid streams list response");
		return value;
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
