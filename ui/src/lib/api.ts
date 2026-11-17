// Every call to osearch's local API. Changes carry the UI header, which the
// server requires for anything that is not a read.

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`/api${path}`, {
    ...init,
    headers: { "content-type": "application/json", "x-osearch-ui": "1", ...(init?.headers ?? {}) },
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new ApiError(res.status, text || `${res.status} ${res.statusText}`);
  }
  return res.json() as Promise<T>;
}

export const get = <T>(path: string) => call<T>(path);
export const post = <T>(path: string, body?: unknown) =>
  call<T>(path, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });

export const runPath = (id: string) => `/runs/${encodeURIComponent(id)}`;
