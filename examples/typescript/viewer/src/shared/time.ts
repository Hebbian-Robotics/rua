export function formatOffset(seconds: number): string {
  const clamped = Math.max(0, seconds);
  const minutes = Math.floor(clamped / 60);
  const remainder = clamped - minutes * 60;
  return `${minutes}:${remainder.toFixed(2).padStart(5, "0")}`;
}
