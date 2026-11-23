import { stationRequest } from "hebbian-rua";

export async function stationJson(
  path: string,
  init?: RequestInit,
): Promise<unknown> {
  return stationRequest(path, { cache: "no-store", ...init });
}
