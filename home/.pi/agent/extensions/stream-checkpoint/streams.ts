import { spawn } from "node:child_process";

type Stream = { slug: string; title: string; status: string; state: string; path: string; [key: string]: unknown };
type UpdateResult = { slug: string; path: string; updated: boolean; findings: unknown[] };
type Runner = (args: string[], input?: string) => Promise<string>;

function isStream(value: unknown): value is Stream {
	const stream = value as Partial<Stream> | null;
	return !!stream && typeof stream.slug === "string" && typeof stream.title === "string" && typeof stream.status === "string" && typeof stream.path === "string";
}

function defaultRunner(cwd: string): Runner {
	return (args, input) => new Promise((resolve, reject) => {
		const child = spawn("streams", args, { cwd, stdio: ["pipe", "pipe", "pipe"] });
		let stdout = "";
		let stderr = "";
		child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
		child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
		child.on("error", reject);
		child.on("close", (code) => {
			if (code === 0) resolve(stdout);
			else reject(new Error(stderr.trim() || `streams ${args[0]} exited ${code}`));
		});
		child.stdin.end(input);
	});
}

export class StreamCli {
	private readonly run: Runner;
	constructor(cwd: string, runner?: Runner) { this.run = runner ?? defaultRunner(cwd); }

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
		return value;
	}

	async project(slug: string): Promise<void> { await this.run(["cmux", "update", "-c", slug]); }
	async format(): Promise<string> { return this.run(["format"]); }
}
