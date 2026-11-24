import {
  MAX_PROMPT_IMAGE_BASE64_LENGTH,
  type PromptImage,
} from "../../../shared/run-api.js";

export const PROMPT_IMAGE_TYPES = "image/jpeg,image/png,image/webp";
export const JPEG_DATA_URL_PREFIX = "data:image/jpeg;base64,";

const MAX_SOURCE_IMAGE_BYTES = 20 * 1024 * 1024;
const MAX_IMAGE_DIMENSION = 1600;

export async function preparePromptImage(file: File): Promise<PromptImage> {
  if (!PROMPT_IMAGE_TYPES.split(",").includes(file.type))
    throw new Error("Choose a JPEG, PNG, or WebP image.");
  if (file.size > MAX_SOURCE_IMAGE_BYTES)
    throw new Error("Choose an image under 20 MiB.");

  const bitmap = await createImageBitmap(file).catch(() => {
    throw new Error("Could not read that image.");
  });
  try {
    const scale = Math.min(
      1,
      MAX_IMAGE_DIMENSION / Math.max(bitmap.width, bitmap.height),
    );
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    const drawingContext = canvas.getContext("2d", { alpha: false });
    if (!drawingContext) throw new Error("Could not prepare the image.");
    drawingContext.fillStyle = "#fff";
    drawingContext.fillRect(0, 0, canvas.width, canvas.height);
    drawingContext.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    const dataUrl = canvas.toDataURL("image/jpeg", 0.85);
    if (!dataUrl.startsWith(JPEG_DATA_URL_PREFIX))
      throw new Error("Could not encode the image as JPEG.");
    const jpegBase64 = dataUrl.slice(JPEG_DATA_URL_PREFIX.length);
    if (jpegBase64.length > MAX_PROMPT_IMAGE_BASE64_LENGTH)
      throw new Error(
        "The processed image is too large. Choose a smaller image.",
      );
    return { filename: file.name.trim().slice(0, 255) || "image", jpegBase64 };
  } finally {
    bitmap.close();
  }
}
