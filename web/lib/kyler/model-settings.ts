import type { SessionModel, ThinkingLevel, ThinkingState } from "./types.ts";

export const thinkingLevels: readonly ThinkingLevel[] = [
  "off",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "ultra",
];
export const thinkingLabels: Record<ThinkingLevel, string> = {
  off: "Off",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Xhigh",
  max: "Max",
  ultra: "Ultra",
};

/** Reject mixed catalog generations instead of displaying a stale grade for a new model. */
export function validateModelSettings(value: SessionModel): SessionModel {
  const state = value.thinking;
  if (!value.selected || !state) {
    throw new Error(value.error || "The selected model's thinking settings are unavailable.");
  }
  if (state.profile !== value.selected.profile || state.revision !== value.selected.revision) {
    throw new Error("Model and thinking settings do not match. Retry loading settings.");
  }
  if (
    !["supported", "unsupported", "unknown"].includes(state.status) ||
    !thinkingLevels.includes(state.level) ||
    !Array.isArray(state.levels) ||
    !state.levels.every((level) => thinkingLevels.includes(level))
  ) {
    throw new Error("Invalid thinking capabilities returned for this model.");
  }
  if (state.status === "supported" && (!state.supported || !state.levels.includes(state.level))) {
    throw new Error("The current thinking level is not supported by this model.");
  }
  if (
    state.status === "unsupported" &&
    (state.level !== "off" || state.levels.some((level) => level !== "off"))
  ) {
    throw new Error("Invalid disabled thinking state returned for this model.");
  }
  if (state.status === "unknown" && state.levels.length > 0) {
    throw new Error("Unconfirmed thinking capabilities cannot enable levels.");
  }
  return value;
}

export function thinkingOptions(state?: ThinkingState) {
  return thinkingLevels.map((level) => ({
    level,
    label: thinkingLabels[level],
    available: !!state && state.status !== "unknown" && state.levels.includes(level),
    reason: !state
      ? "Thinking settings are unavailable."
      : state.status === "unknown"
        ? "This model's thinking capabilities have not been confirmed."
        : !state.levels.includes(level)
          ? "This model does not support this thinking level."
          : undefined,
  }));
}
