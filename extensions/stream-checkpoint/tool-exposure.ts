/**
 * Streams' model tools are reached through codemode rather than declared in every
 * request: the codemode description shows this namespace line, and scripts call
 * `tools.checkpoint({})`, `tools.artifact({...})` and the rest.
 */
export const codemodeExposure = {
 exposure: "codemode",
 namespace: {
  name: "streams",
  description: "Durable work records for this Session: stream_read, checkpoint (request a progress report), artifact (save/read named outputs), select_stream and create_stream (initial attachment only).",
 },
} as const;
