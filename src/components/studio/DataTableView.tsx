import type { StudioOutput } from "../../lib/types";
import { parseStudioArtifact } from "./studioArtifact";

export interface DataTableRow { source?: string; supported_point?: string; [key: string]: unknown; }
export function parseDataTable(raw?: string): { columns: string[]; rows: DataTableRow[] } | null {
  const parsed = parseStudioArtifact(raw, "compare_table");
  if (!parsed.artifact || !parsed.artifact.content || typeof parsed.artifact.content !== "object") return null;
  const content = parsed.artifact.content as { columns?: unknown; rows?: unknown };
  if (!Array.isArray(content.columns) || !content.columns.every((column) => typeof column === "string") || !Array.isArray(content.rows) || !content.rows.every((row) => row && typeof row === "object" && !Array.isArray(row))) return null;
  return { columns: content.columns as string[], rows: content.rows as DataTableRow[] };
}

function formatCellValue(value: unknown): string { if (value == null) return ""; if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return String(value); return JSON.stringify(value); }
export function DataTableView({ output }: { output: StudioOutput }) {
  const parsed = parseStudioArtifact(output.raw_content, "compare_table");
  const artifact = parseDataTable(output.raw_content);
  if (!artifact) return <div className="p-4 text-xs text-error">Invalid table artifact: {parsed.error ?? "content schema is invalid"}</div>;
  if (!artifact.rows.length) return <div className="p-4 text-xs text-text-muted">Table contains no rows</div>;
  return <div className="min-h-0 flex-1 overflow-y-auto p-2"><table className="w-full border-collapse text-xs"><thead><tr className="border-b border-border">{artifact.columns.map((col) => <th key={col} className="px-2 py-1.5 text-left gloss-mono font-normal uppercase tracking-[0.03em] text-text-muted">{col.replace(/_/g, " ")}</th>)}</tr></thead><tbody>{artifact.rows.map((row, idx) => <tr key={idx} className="border-b border-border/50">{artifact.columns.map((col) => <td key={col} className="px-2 py-1.5 text-text-secondary">{formatCellValue(row[col])}</td>)}</tr>)}</tbody></table></div>;
}
