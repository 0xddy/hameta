export interface HealthStatus {
  status: string;
  version: string;
  initialized: boolean;
}

export function fetchHealth(): Promise<HealthStatus> {
  return apiRequest<HealthStatus>("/healthz");
}

export class ApiError extends Error {
  status: number;
  code: string;
  detail?: string;

  constructor(status: number, code: string, message: string, detail?: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.detail = detail;
  }
}

export async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  if (init?.body && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }

  const resp = await fetch(path, {
    credentials: "include",
    ...init,
    headers,
  });

  const text = await resp.text();
  let payload: unknown = null;
  if (text) {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = null;
    }
  }

  if (!resp.ok) {
    if (payload && typeof payload === "object") {
      const errObj = payload as { code?: string; message?: string; detail?: string };
      throw new ApiError(
        resp.status,
        errObj.code ?? `HTTP_${resp.status}`,
        errObj.message ?? `请求失败 (${resp.status})`,
        errObj.detail,
      );
    }
    throw new ApiError(resp.status, `HTTP_${resp.status}`, `请求失败 (${resp.status})`);
  }

  return payload as T;
}
