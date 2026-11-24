import type { Provider } from "../../shared/run-api.js";

export const viewerModelCatalog = {
  openai: {
    label: "OpenAI",
    models: [
      { id: "gpt-6.2-astra", label: "GPT-6.2 Astra" },
      { id: "gpt-6-astra", label: "GPT-6 Astra" },
    ],
  },
  gemini: {
    label: "Gemini",
    models: [
      { id: "gemini-4-neon", label: "Gemini 4 Neon" },
      { id: "gemini-4-hydrogen", label: "Gemini 4 Hydrogen" },
      { id: "gemini-4-carbon", label: "Gemini 4 Carbon" },
      { id: "gemini-4-argon", label: "Gemini 4 Argon" },
    ],
  },
  anthropic: {
    label: "Anthropic",
    models: [
      { id: "claude-fable-5-2", label: "Claude Fable 5.2" },
      { id: "claude-opus-5-5", label: "Claude Opus 5.5" },
    ],
  },
} satisfies Record<
  Provider,
  { label: string; models: { id: string; label: string }[] }
>;
