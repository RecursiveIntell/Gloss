import { useState } from "react";
import { useSettingsStore } from "../../../stores/settingsStore";

export function ChatTemperatureControl({ providerId }: { providerId: string }) {
  const { settings, updateSetting } = useSettingsStore();
  const savedValue = settings.generation_temperature ?? "0.7";
  // A null draft follows acknowledged settings; an edited draft survives reloads.
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const value = draft ?? savedValue;
  const temperature = Number(value);
  const validationError = !value.trim() || !Number.isFinite(temperature) || temperature < 0 || temperature > 2
    ? "Enter a number from 0 to 2." : null;
  const supported = ["ollama", "openai", "llamacpp"].includes(providerId);
  const dirty = draft !== null && draft !== savedValue;
  const error = validationError ?? saveError;

  const apply = async () => {
    if (saving || !supported || !dirty || validationError) return;
    setSaving(true);
    setSaveError(null);
    try {
      await updateSetting("generation_temperature", String(temperature));
      setDraft(null);
    } catch (failure) {
      setSaveError(`Chat temperature was not saved: ${failure instanceof Error ? failure.message : String(failure)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-2 rounded border border-border bg-bg-tertiary/40 p-3">
      <label htmlFor="gloss-chat-temperature" className="block text-xs font-medium text-text">Chat temperature</label>
      <p id="gloss-chat-temperature-help" className="text-xs text-text-muted">
        Lower values reduce variation in replies. Range: 0–2. Default: 0.7.
      </p>
      <div className="flex flex-wrap gap-2">
        <input
          id="gloss-chat-temperature"
          type="number"
          min={0}
          max={2}
          step="any"
          inputMode="decimal"
          aria-label="Chat temperature"
          aria-describedby={`gloss-chat-temperature-help gloss-chat-temperature-saved${error ? " gloss-chat-temperature-error" : ""}${!supported ? " gloss-chat-temperature-unavailable" : ""}`}
          aria-invalid={!!validationError}
          value={value}
          disabled={saving || !supported}
          onChange={(event) => {
            if (saving || !supported) return;
            setDraft(event.target.value);
            setSaveError(null);
          }}
          className="w-24 rounded border border-border bg-bg-tertiary px-2 py-1.5 text-sm text-text focus:border-accent focus:outline-none disabled:opacity-50"
        />
        <button
          type="button"
          onClick={apply}
          disabled={saving || !supported || !dirty || !!validationError}
          className="rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-40"
        >
          {saving ? "Saving…" : "Apply chat temperature"}
        </button>
      </div>
      <p id="gloss-chat-temperature-saved" className="text-xs text-text-muted">
        {settings.generation_temperature === undefined ? "Default temperature" : "Saved temperature"}: {savedValue}
      </p>
      {!supported && (
        <p id="gloss-chat-temperature-unavailable" className="text-xs text-warning">
          {providerId === "anthropic"
            ? "Anthropic uses provider-managed temperature. This setting is not applied to Anthropic replies."
            : "Select Ollama, OpenAI, or llama.cpp to use chat temperature control."}
        </p>
      )}
      {error && <p id="gloss-chat-temperature-error" role="alert" className="text-xs text-error">{error}</p>}
    </div>
  );
}
