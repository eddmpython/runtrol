export type UsageTelemetry = {
  used: number | null;
  size: number | null;
  amount: number | null;
  currency: string;
};

export type LimitTelemetry = {
  usedPercent: number;
  resetsAt: number | null;
  windowMinutes: number | null;
};

export function usageTelemetry(value: unknown): UsageTelemetry {
  const body = object(value);
  const cost = object(body?.cost);
  return {
    used: nonNegativeNumber(body?.used),
    size: nonNegativeNumber(body?.size),
    amount: finiteNumber(cost?.amount),
    currency: typeof cost?.currency === "string" ? cost.currency : "",
  };
}

export function limitTelemetry(value: unknown): LimitTelemetry | null {
  const window = object(value);
  const usedPercent = nonNegativeNumber(window?.used_percent);
  if (usedPercent === null) return null;
  return {
    usedPercent: Math.min(100, usedPercent),
    resetsAt: nonNegativeNumber(window?.resets_at),
    windowMinutes: nonNegativeNumber(window?.window_minutes),
  };
}

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function nonNegativeNumber(value: unknown): number | null {
  const parsed = finiteNumber(value);
  return parsed !== null && parsed >= 0 ? parsed : null;
}
