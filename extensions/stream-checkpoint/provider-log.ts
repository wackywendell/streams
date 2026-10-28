import type { StreamOptions } from "@earendil-works/pi-ai";

type ProviderModel = {provider: string; id: string; api: string};
export type ProviderLogEvent =
 | {type: "provider-request"; model: ProviderModel; payload: unknown}
 | {type: "provider-response"; model: ProviderModel; status: number; headers: Record<string, string>}
 | {type: "provider-event"; model: ProviderModel; event: Record<string, unknown>};
const privateKeys = new Set(["authorization", "proxyauthorization", "apikey", "accesstoken", "refreshtoken", "token", "headers", "env", "thinkingsignature", "signature", "encryptedcontent"]);
/** Copy the provider body, not its credentials or opaque reasoning, without changing the request. */
function diagnosticPayload(value: any): any {
 if (Array.isArray(value)) return value.map(diagnosticPayload);
 if (!value || typeof value !== "object") return value;
 if (["thinking", "redacted_thinking", "reasoning"].includes(value.type)) return {type: value.type, omitted: true};
 return Object.fromEntries(Object.entries(value).map(([key, item]) => [key,
  privateKeys.has(key.replace(/[_-]/g, "").toLowerCase()) || (key === "thinking" || key === "reasoning_content") && typeof item === "string" ? "[omitted]" : diagnosticPayload(item),
 ]));
}
const pick = (value: any, keys: string[]) => Object.fromEntries(keys.filter(key => value && Object.hasOwn(value, key)).map(key => [key, value[key]]));
/** Keep raw tool arguments and completion boundaries; never copy text or thinking streams. */
function diagnosticEvent(value: any): Record<string, unknown> | undefined {
 if (!value || typeof value !== "object") return;
 const event = pick(value, ["type", "index", "sequence_number", "item_id", "output_index", "response_id"]);
 switch (value.type) {
  case "message_start": return {...event, message: pick(value.message, ["id", "type", "role", "model", "usage", "input_transformations"])};
  case "content_block_start": return {...event, content_block: pick(value.content_block, value.content_block?.type === "tool_use" ? ["type", "id", "name", "input"] : ["type"])};
  case "content_block_delta":
   if (value.delta?.type === "input_json_delta") return {...event, delta: pick(value.delta, ["type", "partial_json"])};
   return;
  case "content_block_stop": case "message_stop": return event;
  case "message_delta": return {...event, delta: pick(value.delta, ["stop_reason", "stop_sequence", "stop_details"]), ...pick(value, ["usage", "input_transformations"])};
  case "error": return {...event, error: pick(value.error, ["type", "code", "message"])};
  case "response.created": case "response.in_progress": case "response.completed": case "response.failed": case "response.incomplete":
   return {...event, response: pick(value.response, ["id", "model", "status", "usage", "error", "incomplete_details"])};
  case "response.output_item.added": case "response.output_item.done":
   return {...event, item: pick(value.item, value.item?.type === "function_call" ? ["type", "id", "call_id", "name", "arguments", "status"] : ["type", "id", "status"])};
  case "response.function_call_arguments.delta": return {...event, ...pick(value, ["delta"])};
  case "response.function_call_arguments.done": return {...event, ...pick(value, ["arguments", "name", "call_id"])};
 }
}
const responseHeaders = new Set(["content-type", "request-id", "x-request-id", "x-amzn-requestid", "x-amz-request-id", "x-datadog-trace-id", "traceparent", "retry-after", "retry-after-ms"]);
export function providerLogOptions(observe: (event: ProviderLogEvent) => void): Pick<StreamOptions, "onPayload" | "onResponse" | "onProviderStreamEvent"> {
 const identity = (model: ProviderModel) => ({provider: model.provider, id: model.id, api: model.api});
 return {
  onPayload: (payload, model) => {observe({type: "provider-request", model: identity(model), payload: diagnosticPayload(payload)});},
  onResponse: (response, model) => {observe({type: "provider-response", model: identity(model), status: response.status,
   headers: Object.fromEntries(Object.entries(response.headers).map(([key, value]) => [key.toLowerCase(), value]).filter(([key]) => responseHeaders.has(key))),
  });},
  onProviderStreamEvent: (value, model) => {
   const event = diagnosticEvent(value);
   if (event) observe({type: "provider-event", model: identity(model), event: diagnosticPayload(event)});
  },
 };
}
