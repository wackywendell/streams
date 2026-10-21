import { randomUUID } from "node:crypto";
import { constants, closeSync, fstatSync, openSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { InferenceObserver } from "./agent.ts";
import { privateDirectory, privateWrite } from "./private-storage.ts";

/** Opt-in JSONL sessions retain provider diagnostics without starting a second agent. */
export function helperSessionLogger(ctx: ExtensionContext, directory: string, onCreate: (value: {session_id: string; session_file: string}) => void): InferenceObserver {
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
   const lines = [JSON.stringify({type: "session", version: 3, id, cwd: ctx.cwd, timestamp: new Date().toISOString(), parentSession: ctx.sessionManager.getSessionFile()}) + "\n",
    encode("custom", {customType: "stream-helper-request", data: {parent_session_id: ctx.sessionManager.getSessionId(), parent_leaf_id: ctx.sessionManager.getLeafId(), task: event.request.type}}),
    encode("model_change", {provider: event.model.provider, modelId: event.model.id}),
    encode("message", {message: {role: "system", content: event.payload.systemPrompt, toolsAdded: event.payload.tools, timestamp: Date.now()}}),
    ...event.payload.messages.map(message => encode("message", {message})),
   ];
   privateWrite(path, lines.join(""), directory);
   const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
   try {inode = fstatSync(fd).ino;} finally {closeSync(fd);}
   file = path;
   onCreate({session_id: id, session_file: file});
  } else if (event.type === "response") entry("message", {message: event.response});
  else entry("custom", {customType: `stream-helper-${event.type}`, data: event});
 };
}
