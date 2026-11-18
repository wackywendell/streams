import { randomUUID } from "node:crypto";
import { lstatSync, readdirSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { buildInferencePayload } from "./agent.ts";
import { digest } from "./capture.ts";
import { privateDirectory, privateRead, privateWrite } from "./private-storage.ts";
import { parseLocalContext } from "./wire.ts";
import type { PreparedRequest } from "./prompt-lab.ts";

export type ArtifactKind = "capture" | "request" | "run";
const folders = {capture: "captures", request: "requests", run: "runs"};
const files = {capture: "capture.json", request: "request.json", run: "run.json"};
const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const object = (value: any): value is Record<string, any> => !!value && typeof value === "object" && !Array.isArray(value);
const identity = (value: any): value is string => typeof value === "string" && !!value.trim() && !/[\x00-\x1f]/.test(value);
export function artifactPath(root: string, kind: ArtifactKind, id: string): string {
 if (!Object.hasOwn(folders, kind) || !uuid.test(id)) throw Error("Choose an exact artifact kind and ID");
 const directory = join(root, folders[kind], id);
 return directory;
}
export function createArtifact(root: string, kind: ArtifactKind): {id: string; directory: string} {
 const id = randomUUID(), directory = artifactPath(root, kind, id);
 privateDirectory(directory, root);
 return {id, directory};
}
export function readArtifact(root: string, kind: ArtifactKind, id: string): any {
 const value = JSON.parse(privateRead(artifactPath(root, kind, id), files[kind], root));
 if (!object(value) || value.version !== 1 || value.id !== id) throw Error("Invalid artifact manifest");
 return value;
}
export function listArtifacts(root: string, kind: ArtifactKind): string[] {
 if (!Object.hasOwn(folders, kind)) throw Error("Choose capture, request or run");
 const parent = join(root, folders[kind]);
 let entries: string[];
 try {entries = readdirSync(privateDirectory(parent, root, false));} catch (error) {if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];throw error;}
 return entries.filter(id => {
  if (!uuid.test(id)) return false;
  const directory = privateDirectory(artifactPath(root, kind, id), root, false), file = lstatSync(join(directory, files[kind]), {throwIfNoEntry: false});
  if (file?.isSymbolicLink()) throw Error("Artifact manifest must not be a symbolic link");
  return !!file?.isFile();
 }).sort();
}
export function savePrepared(root: string, request: PreparedRequest): {id: string; directory: string} {
 validatePrepared(request);
 const artifact = createArtifact(root, "request");
 privateWrite(join(artifact.directory, "request.json"), JSON.stringify({...request, id: artifact.id}, null, 2) + "\n", root);
 return artifact;
}
export function loadPrepared(root: string, id: string): PreparedRequest {
 const request = readArtifact(root, "request", id);validatePrepared(request);return request;
}
export function validatePrepared(value: any): asserts value is PreparedRequest {
 if (!object(value) || value.version !== 1 || typeof value.trim !== "boolean" || !object(value.input) || !["enroll", "session", "stream"].includes(value.input.type) || !Array.isArray(value.input.messages) || value.input.reset !== undefined && typeof value.input.reset !== "boolean") throw Error("Invalid prepared task input");
 const input = value.input, catalog = parseLocalContext(input.catalog);
 if (input.type === "enroll" ? input.reset || catalog.session : !catalog[input.type as "session" | "stream"]) throw Error("Prepared task does not match its captured cards");
 const source = value.source;
 if (!object(source) || !uuid.test(source.capture_id) || !Number.isInteger(source.sample) || source.sample < 0 || !identity(source.host_id) || !identity(source.native_session_id) || !identity(source.cwd) || !isAbsolute(source.cwd) || !(source.leafId === null || identity(source.leafId)) || catalog.session && (catalog.session.host_id !== source.host_id || catalog.session.harness_session_id !== source.native_session_id)) throw Error("Invalid prepared source identity");
 const payload = value.model_request;
 if (!object(payload) || typeof payload.systemPrompt !== "string" || !payload.systemPrompt.trim() || !Array.isArray(payload.messages) || !Array.isArray(payload.tools) || JSON.stringify(payload.tools) !== JSON.stringify(buildInferencePayload({type: input.type, catalog, messages: input.messages, reset: input.reset}).tools)) throw Error("Prepared request output schema does not match production validation");
 if (value.input_hash !== digest(JSON.stringify({input, model_request: payload}))) throw Error("Prepared request inputs changed; prepare a new request instead");
 if (!object(value.source_files) || !Object.values(value.source_files).every(hash => typeof hash === "string" && /^[0-9a-f]{64}$/.test(hash))) throw Error("Invalid prepared source hashes");
}
