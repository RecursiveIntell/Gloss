import type { StudioOutput } from "../../lib/types";
import { parseStudioArtifact } from "./studioArtifact";

export interface TimelineEntry { sequence: number; label: string; event: string; source_title?: string; source_id?: string; }
export function parseTimeline(raw?: string): { entries: TimelineEntry[] } | null {
  const parsed = parseStudioArtifact(raw, "timeline");
  if (!parsed.artifact || !parsed.artifact.content || typeof parsed.artifact.content !== "object") return null;
  const entries = (parsed.artifact.content as { entries?: unknown }).entries;
  if (!Array.isArray(entries) || !entries.every((item) => item && typeof item === "object" && typeof (item as TimelineEntry).sequence === "number" && typeof (item as TimelineEntry).label === "string" && typeof (item as TimelineEntry).event === "string")) return null;
  return { entries: entries as TimelineEntry[] };
}

export function TimelineView({ output }: { output: StudioOutput }) {
  const parsed = parseStudioArtifact(output.raw_content, "timeline");
  const artifact = parseTimeline(output.raw_content);
  if (!artifact) return <div className="p-4 text-xs text-error">Invalid timeline artifact: {parsed.error ?? "content schema is invalid"}</div>;
  if (!artifact.entries.length) return <div className="p-4 text-xs text-text-muted">Timeline contains no entries</div>;
  return <div className="min-h-0 flex-1 overflow-y-auto p-4"><div className="relative"><div className="absolute left-4 top-0 bottom-0 w-0.5 bg-border" /><div className="space-y-6">{artifact.entries.map((entry) => <div key={entry.sequence} className="relative flex gap-4"><div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border-2 border-accent bg-bg-secondary text-xs font-medium gloss-mono text-text">{entry.sequence}</div><div className="flex-1 rounded-lg border border-border bg-bg-secondary/30 p-3"><div className="gloss-mono mb-1 text-[10px] uppercase tracking-[0.03em] text-accent">{entry.label}</div><p className="text-sm leading-relaxed text-text-secondary whitespace-pre-wrap">{entry.event}</p></div></div>)}</div></div></div>;
}
