import { randomUUID } from "node:crypto";
import { constants, closeSync, fstatSync, openSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { InferenceObserver, InferenceSource, InferenceLogOptions } from "./agent.ts";
import { privateDirectory, privateWrite } from "./private-storage.ts";

/** Opt-in JSONL sessions retain provider diagnostics without starting a second agent. */
export function helperSessionLogger(source: InferenceSource, options: InferenceLogOptions): InferenceObserver {
 const {directory, onCreate} = options;
 let file: string | undefined, parentId: string | null = null, inode: number | undefined;
 const encode = (type: string, fields: Record<string, unknown>) => {
  const id = randomUUID(), line = JSON.stringify({type, id, parentId, timestamp: new Date().toISOString(), ...fields}) + "\n";
  parentId = id;return line;
 };
 const entry = (type: string, fields: Record<string, unknown>) => {
  if (!file) return;
  privateDirectory(directory, directory);
  const fd = openSync(file, constants.O_WRONLY | constants.O_APPEND | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
   const stat = fstatSync(fd);
   if (!stat.isFile() || stat.ino !== inode || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0) throw Error("Helper log identity or permissions changed");
   writeFileSync(fd, encode(type, fields));
  } finally {closeSync(fd);}
 };
 return event => {
  if (event.type === "request") {
   file = undefined;parentId = null;
   const id = randomUUID(), path = join(directory, `${id}.jsonl`);
   const lines = [JSON.stringify({type: "session", version: 3, id, cwd: source.cwd, timestamp: new Date().toISOString(), parentSession: source.sessionFile}) + "\n",
    encode("session_info", {name: `Streams helper · ${event.request.type}${"reset" in event.request && event.request.reset ? " reset" : ""}`}),
    encode("custom", {customType: "stream-helper-request", data: {parent_session_id: source.sessionId, parent_leaf_id: source.leafId, task: event.request.type, reset: "reset" in event.request && Boolean(event.request.reset)}}),
    encode("model_change", {provider: event.model.provider, modelId: event.model.id}),
    encode("message", {message: {role: "system", content: event.payload.systemPrompt, toolsAdded: event.payload.tools, timestamp: Date.now()}}),
    ...event.payload.messages.map(message => encode("message", {message})),
   ];
   privateWrite(path, lines.join(""), directory);
   const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
   try {inode = fstatSync(fd).ino;} finally {closeSync(fd);}
   file = path;
   onCreate?.({session_id: id, session_file: file});
  } else if (event.type === "response") entry("message", {message: event.response});
  else entry("custom", {customType: `stream-helper-${event.type}`, data: event.type === "result" ? event.result : event});
 };
}
