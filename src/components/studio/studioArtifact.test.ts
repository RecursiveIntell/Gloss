import { describe, expect, it } from "vitest";
import { parseDataTable } from "./DataTableView";
import { parseTimeline } from "./TimelineView";

const envelope = (output_type: string, content: unknown) => JSON.stringify({
  schema: "StudioArtifactV1", receipt_schema: "StudioArtifactReceiptV1", receipt_id: "r",
  output_type, title: "Test", prompt_used: "test", source_scope: {}, content,
  validation: { schema_validated: true, all_items_source_cited: true, deterministic: true, errors: [] },
});

describe("Studio artifact render parsers", () => {
  it("reads timeline and table data from artifact.content", () => {
    expect(parseTimeline(envelope("timeline", { entries: [{ sequence: 1, label: "Source", event: "Point" }] }))?.entries).toHaveLength(1);
    expect(parseDataTable(envelope("compare_table", { columns: ["source"], rows: [{ source: "A" }] }))?.rows).toHaveLength(1);
  });

  it("rejects bare content, invalid validation, and wrong shapes", () => {
    expect(parseTimeline(JSON.stringify({ entries: [] }))).toBeNull();
    expect(parseDataTable(envelope("compare_table", { columns: ["source"], rows: ["bad"] }))).toBeNull();
    expect(parseTimeline(envelope("timeline", { entries: [] }))).toEqual({ entries: [] });
  });
});
