export interface StudioArtifactEnvelope {
  schema: "StudioArtifactV1";
  receipt_schema: string;
  receipt_id: string;
  output_type: string;
  title: string;
  prompt_used: string;
  source_scope: unknown;
  content: unknown;
  validation: { schema_validated: boolean; all_items_source_cited: boolean; deterministic: boolean; errors: string[] };
}

export function parseStudioArtifact(raw: string | undefined, expectedOutputType: string): { artifact?: StudioArtifactEnvelope; error?: string } {
  if (!raw) return { error: "Studio artifact is missing" };
  let value: unknown;
  try { value = JSON.parse(raw); } catch { return { error: "Studio artifact is not valid JSON" }; }
  if (!value || typeof value !== "object") return { error: "Studio artifact must be an object" };
  const artifact = value as Partial<StudioArtifactEnvelope>;
  if (artifact.schema !== "StudioArtifactV1") return { error: "Studio artifact schema is invalid" };
  if (artifact.output_type !== expectedOutputType) return { error: "Studio artifact output type does not match the renderer" };
  if (!Object.prototype.hasOwnProperty.call(artifact, "content")) return { error: "Studio artifact content is missing" };
  if (!artifact.validation || artifact.validation.schema_validated !== true) return { error: "Studio artifact validation failed" };
  return { artifact: artifact as StudioArtifactEnvelope };
}
