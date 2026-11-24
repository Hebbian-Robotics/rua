import { expect, test } from "vitest";
import { preparePromptImage } from "../src/features/agent/prompt-image.js";

test("rejects reference images the model cannot accept before decoding them", async () => {
  await expect(
    preparePromptImage(
      new File(["text"], "reference.txt", { type: "text/plain" }),
    ),
  ).rejects.toThrow("Choose a JPEG, PNG, or WebP image.");

  const oversizedReferenceImage = new File(
    [new Uint8Array(21 * 1024 * 1024)],
    "large.png",
    { type: "image/png" },
  );
  await expect(preparePromptImage(oversizedReferenceImage)).rejects.toThrow(
    "Choose an image under 20 MiB.",
  );
});
