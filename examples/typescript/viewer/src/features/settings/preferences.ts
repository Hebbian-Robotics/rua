import { z } from "zod";
import { motionLimitsSchema, providerSchema } from "../../../shared/run-api.js";

const VIEWER_PREFERENCES_KEY = "hebbian-viewer-preferences";

const preferencesSchema = z.object({
  version: z.literal(1),
  theme: z.enum(["system", "light", "dark"]).catch("system").default("system"),
  showTargetJoints: z.boolean().catch(true).default(true),
  showCommandedJoints: z.boolean().catch(false).default(false),
  showCameraLabels: z.boolean().catch(true).default(true),
  agentModelSelection: z
    .object({
      provider: providerSchema,
      model: z.string().min(1).nullable(),
      custom: z.literal(true).optional(),
    })
    .nullable()
    .catch(null)
    .default(null),
  motionLimits: motionLimitsSchema.catch({}).default({}),
  recordWithControl: z.boolean().catch(false).default(false),
});

export type ViewerPreferences = z.output<typeof preferencesSchema>;
export type AgentModelSelection = ViewerPreferences["agentModelSelection"];
export type ViewerColorScheme = Exclude<ViewerPreferences["theme"], "system">;

export function defaultViewerPreferences(): ViewerPreferences {
  return preferencesSchema.parse({ version: 1 });
}

type PreferenceStorage = Pick<Storage, "getItem" | "setItem">;

export function readViewerPreferences(
  storage: PreferenceStorage,
): ViewerPreferences {
  try {
    const serialized = storage.getItem(VIEWER_PREFERENCES_KEY);
    if (serialized !== null) {
      const result = preferencesSchema.safeParse(JSON.parse(serialized));
      if (result.success) return result.data;
    }
  } catch {
    // Blocked storage or corrupt JSON falls back to defaults.
  }
  return defaultViewerPreferences();
}

// WHY: Keep preferences usable in memory when browser storage is unavailable.
export function writeViewerPreferences(
  storage: PreferenceStorage,
  preferences: ViewerPreferences,
): void {
  try {
    storage.setItem(VIEWER_PREFERENCES_KEY, JSON.stringify(preferences));
  } catch {
    // Private browsing or a full storage quota need not prevent using the viewer.
  }
}
