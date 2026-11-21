import { describe, expect, it } from "vitest";
import { createRobotToolCatalog } from "../src/tool-catalog.js";

describe("provider tool schema", () => {
  it("declares every required field in the same schema object", () => {
    const tool = createRobotToolCatalog().tools.find(
      (item) => item.name === "action_sequence",
    );
    expect(tool).toBeDefined();
    function verifyRequiredFields(value: unknown): void {
      if (Array.isArray(value)) {
        for (const item of value) verifyRequiredFields(item);
        return;
      }
      if (value === null || typeof value !== "object") return;
      const object = value as Record<string, unknown>;
      if (Array.isArray(object["required"])) {
        expect(object["properties"]).toBeDefined();
        for (const field of object["required"]) {
          expect(object["properties"]).toHaveProperty(String(field));
        }
      }
      for (const item of Object.values(object)) verifyRequiredFields(item);
    }
    verifyRequiredFields(tool?.parameters);
  });
});
