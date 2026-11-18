export type DomainObject = Record<string, unknown>;
export type PiRequestContext = {conversation: {harness: "pi"; native_id: string}; session_file: string | null};
export type TitleDefault = "automatic" | "manual";
export type CardMode = "inherit" | "automatic" | "manual";
export type CardSettings = {global: TitleDefault; session: CardMode; effective: TitleDefault};
export type ArtifactRequest =
 | {action: "write"; name: string; description: string; session_id?: string; stream_id?: string; text?: string; file?: string}
 | {action: "read"; reference: string; offset?: number; limit?: number}
 | {action: "list_session"; host?: string; session_id?: string; stream_id?: string}
 | {action: "list_stream"; host?: string; stream_id: string}
 | {action: "delete"; reference: string};
export type ConfigSnapshot = {
 naming: {stream: TitleDefault; session: TitleDefault};
 herdr: {
  viewer: {mode: "editor" | "less" | "hunk" | "native"; placement: "pane-right" | "pane-down" | "tab" | "floating"};
  naming: {tab: boolean; workspace: boolean};
  cards: {auto_open: TitleDefault};
  timeout_ms: number;
  pane_height_fraction: number;
 };
 pi: {
  naming: {session: boolean; import_edits: boolean};
  reporting: {
   mode: TitleDefault;
   enrollment: TitleDefault;
   frequency: number;
   context: "trimmed" | "full";
   model: {provider: string; id: string} | null;
   session_logging: boolean;
   debug_commands: boolean;
  };
 };
};
export type ChildRequest =
 | {kind: "session_started" | "attach" | "open_card" | "close_card" | "list_streams" | "discard_unreadable_card"}
 | {kind: "context"; stream_id?: string}
 | {kind: "capture_inputs"; target: {kind: "inventory"} | {kind: "stream"; stream_id: string} | {kind: "workspace"; workspace_id: string}}
 | {kind: "preview"; catalog: DomainObject; action: "apply" | "enroll"; request: DomainObject}
 | {kind: "enroll" | "apply"; request: DomainObject}
 | {kind: "sync"}
 | {kind: "lifecycle"; action: "resume" | "park"}
 | {kind: "artifact"; request: ArtifactRequest}
 | {kind: "set_title_default"; target: "stream" | "session"; mode: TitleDefault}
 | {kind: "get_config"}
 | {kind: "set_config"; key: string; value: string}
 | {kind: "unset_config"; key: string}
 | {kind: "card_settings"}
 | {kind: "set_card_mode"; target: "global" | "session"; mode: CardMode};
export type ChildOutcome =
 | {status: "success"; result: DomainObject}
 | {status: "saved_with_warnings"; result: DomainObject; warnings: string[]}
 | {status: "error"; error: {kind: string; message: string}};
export type ChildView = {desired_pi_name: string | null};
export type ServerFrame =
 | {type: "ready"; protocol: 2; source: string}
 | {type: "response"; id: number; outcome: ChildOutcome}
 | {type: "view"; view: ChildView}
 | {type: "warning"; message: string};
export type Runner = (request: ChildRequest, options?: {timeout: number; signal?: AbortSignal}) => Promise<ChildOutcome>;
const object = (value: unknown): value is DomainObject => !!value && typeof value === "object" && !Array.isArray(value);
const fields = (value: unknown, keys: string[]): value is DomainObject => object(value) && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));

export function hcLifecycle(value: DomainObject): {warnings: string[]} | undefined {
 if (value.backend !== "hc" || !fields(value, ["backend", "report"]) || !fields(value.report, ["native", "projection"])) return;
 const {native, projection} = value.report;
 if (!object(native) || !(fields(native, ["kind"]) && native.kind === "active"
  || fields(native, ["kind", "reason"]) && native.kind === "inactive" && ["outside_frontend", "not_enrolled", "unavailable", "stopped"].includes(native.reason as string))) return;
 if (!fields(projection, ["companion", "desired_pi_name", "warnings"])
  || !(projection.companion === null || ["opened", "existing", "suppressed", "deferred"].includes(projection.companion as string))
  || !(projection.desired_pi_name === null || typeof projection.desired_pi_name === "string")
  || !Array.isArray(projection.warnings) || !projection.warnings.every(item => typeof item === "string")) return;
 return {warnings: projection.warnings as string[]};
}

/** JSON.parse admits duplicate keys; reject them, including escaped and nested keys. */
function parse(line: string): unknown {
 const value: unknown = JSON.parse(line);
 const stack: ({keys: Set<string>; key: boolean} | null)[] = [];
 for (const token of line.match(/"(?:\\.|[^"\\])*"|[{}\[\]:,]|[^{}\[\]:,\s]+/g) ?? []) {
  if (token === "{") stack.push({keys: new Set(), key: true});
  else if (token === "[") stack.push(null);
  else if (token === "}" || token === "]") stack.pop();
  else {
   const frame = stack.at(-1);
   if (token === ",") {if (frame) frame.key = true;}
   else if (frame?.key && token.startsWith('"')) {
    const key: string = JSON.parse(token);
    if (frame.keys.has(key)) throw Error("Duplicate Streams child JSON key");
    frame.keys.add(key);frame.key = false;
   }
  }
 }
 return value;
}
export function decodeServerFrame(line: string): ServerFrame {
 const value = parse(line);
 if (object(value)) {
  if (value.type === "ready" && fields(value, ["type", "protocol", "source"]) && value.protocol === 2 && typeof value.source === "string") return value as ServerFrame;
  if (value.type === "warning" && fields(value, ["type", "message"]) && typeof value.message === "string") return value as ServerFrame;
  if (value.type === "view" && fields(value, ["type", "view"]) && fields(value.view, ["desired_pi_name"]) && (value.view.desired_pi_name === null || typeof value.view.desired_pi_name === "string")) return value as ServerFrame;
  if (value.type === "response" && fields(value, ["type", "id", "outcome"]) && Number.isSafeInteger(value.id) && (value.id as number) > 0 && object(value.outcome)) {
   const outcome = value.outcome;
   if (outcome.status === "success" && fields(outcome, ["status", "result"]) && object(outcome.result)
    || outcome.status === "saved_with_warnings" && fields(outcome, ["status", "result", "warnings"]) && object(outcome.result) && Array.isArray(outcome.warnings) && outcome.warnings.length > 0 && outcome.warnings.every(item => typeof item === "string")
    || outcome.status === "error" && fields(outcome, ["status", "error"]) && fields(outcome.error, ["kind", "message"]) && typeof outcome.error.kind === "string" && typeof outcome.error.message === "string") return value as ServerFrame;
  }
 }
 throw Error("Invalid Streams child v2 frame");
}
