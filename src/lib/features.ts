import type { FeatureFlagStatus } from "./types";

export const EXPERIMENTAL_FEATURES_ENABLED = "experimental_features_enabled";
export const FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED =
  "feature_semantic_memory_preview_enabled";
export const FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED =
  "feature_semantic_memory_turbo_quant_enabled";

export function featureById(
  flags: FeatureFlagStatus[],
  id: string
): FeatureFlagStatus | undefined {
  return flags.find((flag) => flag.id === id);
}

export function featureSections(
  flags: FeatureFlagStatus[]
): Record<string, FeatureFlagStatus[]> {
  return flags.reduce<Record<string, FeatureFlagStatus[]>>((sections, flag) => {
    if (!sections[flag.section]) sections[flag.section] = [];
    sections[flag.section].push(flag);
    return sections;
  }, {});
}

export function canUseSemanticMemoryPreview(flags: FeatureFlagStatus[]): boolean {
  return featureById(flags, FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED)?.active === true;
}
