export function errorMessage(
  error: unknown,
  fallback: string = String(error),
): string {
  return error instanceof Error ? error.message : fallback;
}
