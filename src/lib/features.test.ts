import { describe, expect, it } from "vitest";
import {
  canUseSemanticMemoryPreview,
  FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED,
} from "./features";
import type { FeatureFlagStatus } from "./types";

function semanticFlag(overrides: Partial<FeatureFlagStatus>): FeatureFlagStatus {
  return {
    id: FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED,
    label: "semantic-memory",
    section: "Memory & Retrieval",
    description: "fixture",
    enabled: true,
    active: false,
    available: true,
    stable: true,
    default_enabled: false,
    requires_experimental: false,
    unavailable_reason: null,
    ...overrides,
  };
}

describe("semantic-memory profile selection", () => {
  it("allows selecting an available profile before it is active", () => {
    expect(canUseSemanticMemoryPreview([semanticFlag({ active: false, available: true })])).toBe(true);
  });

  it("rejects a selected-looking profile when the build capability is unavailable", () => {
    expect(canUseSemanticMemoryPreview([semanticFlag({ active: true, available: false })])).toBe(false);
  });
});
