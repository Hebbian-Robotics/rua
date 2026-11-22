export function allowedViewerOrigins(
  stationUrl: string,
  configuredOrigins: string | undefined,
): string[] {
  return (
    configuredOrigins ??
    `${new URL(stationUrl).origin},http://127.0.0.1:5173,http://localhost:5173`
  )
    .split(",")
    .map((origin) => origin.trim())
    .filter((origin) => origin !== "");
}
