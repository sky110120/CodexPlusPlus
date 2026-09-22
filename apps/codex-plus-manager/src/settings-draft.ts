type JsonRecord = Record<string, unknown>;

export type SingleFlightRef = { current: boolean };

export async function runSingleFlight<T>(
  ref: SingleFlightRef,
  task: () => Promise<T>,
): Promise<{ accepted: boolean; value?: T }> {
  if (ref.current) return { accepted: false };
  ref.current = true;
  try {
    return { accepted: true, value: await task() };
  } finally {
    ref.current = false;
  }
}

function recordsEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function mergeChangedRecordFields(
  latest: unknown,
  base: unknown,
  draft: unknown,
): unknown {
  if (!isRecord(base) || !isRecord(draft)) return draft;
  const merged: JsonRecord = isRecord(latest) ? { ...latest } : {};
  for (const [key, value] of Object.entries(draft)) {
    if (!recordsEqual(base[key], value)) merged[key] = value;
  }
  return merged;
}

/** Preserve only locally changed draft fields while refreshing clean fields from the backend. */
export function mergeSettingsDraft<T extends object>(latest: T, base: T, draft: T): T {
  const latestRecord = latest as JsonRecord;
  const baseRecord = base as JsonRecord;
  const draftRecord = draft as JsonRecord;
  const merged: JsonRecord = { ...latestRecord };
  for (const [key, value] of Object.entries(draftRecord)) {
    if (key === "tools" && isRecord(value) && isRecord(baseRecord.tools)) {
      const latestTools = isRecord(latestRecord.tools) ? latestRecord.tools : {};
      const mergedTools: JsonRecord = { ...latestTools };
      for (const [toolId, shard] of Object.entries(value)) {
        if (recordsEqual(baseRecord.tools[toolId], shard)) continue;
        mergedTools[toolId] = mergeChangedRecordFields(latestTools[toolId], baseRecord.tools[toolId], shard);
      }
      merged.tools = mergedTools;
    } else if (!recordsEqual(baseRecord[key], value)) {
      merged[key] = value;
    }
  }
  return merged as T;
}

/** Replace only the Grok shard after a provider refresh; preserve every other draft field. */
export function mergeGrokProviderSnapshot<T extends { tools?: Record<string, unknown> }>(
  settings: T,
  profiles: unknown[],
  activeRelayId: string,
): T {
  const grok = isRecord(settings.tools?.grok) ? settings.tools.grok : {};
  return {
    ...settings,
    tools: {
      ...settings.tools,
      grok: { ...grok, relayProfiles: profiles, activeRelayId },
    },
  } as T;
}
