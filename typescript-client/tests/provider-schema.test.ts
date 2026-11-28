import { describe, expect, it } from "vitest";
import { robotToolSpecifications } from "../src/generated/robot-tools.js";
import { createRobotToolCatalog } from "../src/tool-catalog.js";

describe("provider tool schema", () => {
  it("describes the actions and scope available to the model", () => {
    expect(
      createRobotToolCatalog().tools.find(
        (tool) => tool.name === "action_sequence",
      )?.description,
    ).toBe(
      robotToolSpecifications.find((tool) => tool.name === "action_sequence")
        ?.description,
    );
    const motion = createRobotToolCatalog({
      sequenceActions: ["pose"],
    }).tools.find((tool) => tool.name === "action_sequence")?.description;
    expect(motion).toContain("without stopping");
    expect(motion).not.toContain("gripper_opening");
    expect(motion).not.toContain("single-arm step");

    const named = createRobotToolCatalog({
      sequenceActions: ["joints", "gripper"],
      sequenceScope: { kind: "named", arms: ["left", "right"] },
    }).tools.find((tool) => tool.name === "action_sequence")?.description;
    expect(named).toContain("single-arm step");
    expect(named).toContain("gripper_opening");

    const gripper = createRobotToolCatalog({
      sequenceActions: ["gripper"],
    }).tools.find((tool) => tool.name === "action_sequence")?.description;
    expect(gripper).not.toContain("waypoint");
    expect(gripper).toContain("does not confirm a grasp");
  });

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
