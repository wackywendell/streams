export const STREAM_PROMPTING = {
	manual: "Only open stream selection or creation when the user asks to choose, switch or create a stream. An unbound session alone is not a reason to open the chooser.",
	auto: "You may suggest an existing or new stream when substantive work has no bound stream, or clearly changes scope. The user must confirm the selection or creation. Do not re-confirm the current stream.",
} as const;

const low = "You may initiate a checkpoint at a substantial task's completion or a clear pause/handoff, when durable pickup context has changed.";
const medium = `${low} Also checkpoint after major decisions or a substantial change in direction.`;
export const CHECKPOINT_PROMPTING = {
	manual: "Only initiate a checkpoint when the user asks for one.",
	low,
	medium,
	high: `${medium} Also checkpoint at meaningful subtask milestones to preserve useful partial progress.`,
} as const;

export interface PromptingConfig {
	streamPrompting: keyof typeof STREAM_PROMPTING;
	checkpointPrompting: keyof typeof CHECKPOINT_PROMPTING;
}

/** Main-agent initiation policy, separate from tool availability and user approval. */
export function promptingPolicy(config: PromptingConfig): string {
	return [
		"Follow this policy for initiating stream tools, even when the streams skill describes a more proactive workflow. It does not restrict user-invoked /checkpoint or the separate checkpoint conversation.",
		`Stream selection / creation: ${config.streamPrompting}. ${STREAM_PROMPTING[config.streamPrompting]}`,
		`Checkpoint frequency: ${config.checkpointPrompting}. ${CHECKPOINT_PROMPTING[config.checkpointPrompting]}`,
		"For every automatic checkpoint level, skip routine steps, unchanged context, an already-open checkpoint and unbound sessions. Do not choose or create a stream merely to enable checkpointing. These are contextual triggers, not a timer or a requirement to checkpoint every turn.",
	].join("\n");
}
