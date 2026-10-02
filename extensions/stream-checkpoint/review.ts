import type { Draft } from "./core.ts";

export type ReviewLine =
	| { kind: "context"; text: string; oldLine: number; newLine: number }
	| { kind: "removed"; text: string; oldLine: number }
	| { kind: "added"; text: string; newLine: number };

export interface AnnotationAnchor {
	revision: number;
	side: "original" | "proposed";
	line: number;
	section: string;
	excerpt: string;
}
export interface Annotation extends AnnotationAnchor { comment: string }
export interface AnnotationEditor { anchor: AnnotationAnchor; text: string }
export type Author = "main-agent";
export interface Feedback { prompt: string; text: string; annotations: Annotation[]; author?: Author }

/** Logical document lines, independent of terminal wrapping and inserted comment rows. */
export function reviewLines(draft: Pick<Draft, "baseContent" | "proposedContent">): ReviewLine[] {
	const before = draft.baseContent.replace(/\n$/, "").split("\n");
	const after = draft.proposedContent.replace(/\n$/, "").split("\n");
	if (before.length * after.length > 2_000_000) {
		return [...before.map((text, i): ReviewLine => ({ kind: "removed", text, oldLine: i + 1 })),
			...after.map((text, i): ReviewLine => ({ kind: "added", text, newLine: i + 1 }))];
	}
	const counts = Array.from({ length: before.length + 1 }, () => new Uint32Array(after.length + 1));
	for (let i = before.length - 1; i >= 0; i--) {
		for (let j = after.length - 1; j >= 0; j--) {
			counts[i][j] = before[i] === after[j] ? counts[i + 1][j + 1] + 1 : Math.max(counts[i + 1][j], counts[i][j + 1]);
		}
	}
	const lines: ReviewLine[] = [];
	let i = 0;
	let j = 0;
	while (i < before.length || j < after.length) {
		if (i < before.length && j < after.length && before[i] === after[j]) {
			lines.push({ kind: "context", text: before[i], oldLine: ++i, newLine: ++j });
		} else if (i < before.length && (j === after.length || counts[i + 1][j] >= counts[i][j + 1])) {
			lines.push({ kind: "removed", text: before[i], oldLine: ++i });
		} else {
			lines.push({ kind: "added", text: after[j], newLine: ++j });
		}
	}
	return lines;
}

export function diffText(line: ReviewLine): string {
	return `${line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " "} ${line.text}`;
}

export function anchorAnnotation(draft: Draft, selected: ReviewLine): AnnotationAnchor {
	const side = selected.kind === "removed" ? "original" : "proposed";
	const line = selected.kind === "removed" ? selected.oldLine : selected.newLine;
	const source = (side === "original" ? draft.baseContent : draft.proposedContent).replace(/\n$/, "").split("\n");
	const section = source.slice(0, line).reverse().find((text) => /^#{1,6}\s/.test(text)) ?? "(frontmatter or document start)";
	const start = Math.max(0, line - 3);
	const excerpt = source.slice(start, line + 2).map((text, index) => `${start + index + 1}: ${text}`).join("\n");
	return { revision: draft.revision, side, line, section, excerpt };
}

export function sameAnchor(a: AnnotationAnchor, b: AnnotationAnchor): boolean {
	return a.revision === b.revision && a.side === b.side && a.line === b.line;
}

export function annotationContext(annotation: Annotation): string {
	return `Draft ${annotation.revision} · ${annotation.side} line ${annotation.line}\nSection: ${annotation.section}\n${annotation.excerpt}\nComment: ${annotation.comment}`;
}

/** The model receives the full evidence; only the transcript presentation collapses it. */
export function feedbackPrompt(text: string, annotations: Annotation[]): string {
	return [text.trim(), ...annotations.map((annotation, index) => `Annotation ${index + 1}\n${annotationContext(annotation)}`)].filter(Boolean).join("\n\n");
}
