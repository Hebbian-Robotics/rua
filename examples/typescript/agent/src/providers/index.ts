import { ClaudeConversation } from "./claude.js";
import { GeminiConversation } from "./gemini.js";
import { OpenAIConversation } from "./openai.js";
import type {
  ConversationSettings,
  ModelConversation,
  Provider,
} from "./types.js";

export function startConversation(
  provider: Provider,
  settings: ConversationSettings,
): ModelConversation {
  switch (provider) {
    case "anthropic":
      return new ClaudeConversation(settings);
    case "openai":
      return new OpenAIConversation(settings);
    case "gemini":
      return new GeminiConversation(settings);
  }
}
