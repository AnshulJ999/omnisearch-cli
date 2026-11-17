import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** A run id starts with its local time: 2026-11-17_051103_topic-words. */
export function runTime(id: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})_(\d{2})(\d{2})(\d{2})/.exec(id);
  if (!m) return null;
  return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], +m[6]);
}

export function when(d: Date | null): string {
  if (!d) return "unknown time";
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (sameDay) return `today ${time}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (d.toDateString() === yesterday.toDateString()) return `yesterday ${time}`;
  return `${d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" })} ${time}`;
}

export function ago(d: Date | null): string {
  if (!d) return "";
  const s = Math.round((Date.now() - d.getTime()) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

export const usd = (n: number) => (n === 0 ? "$0" : n < 0.01 ? `$${n.toFixed(4)}` : `$${n.toFixed(2)}`);

export const num = (n: number) => n.toLocaleString();

export function tokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function ms(n: number): string {
  if (n >= 60_000) return `${(n / 60_000).toFixed(1)} min`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)} s`;
  return `${Math.round(n)} ms`;
}

export function pct(part: number, whole: number): string {
  if (!whole) return "–";
  return `${Math.round((part / whole) * 100)}%`;
}

/** An engine's date as given: ISO dates cut to the day, anything else kept short. */
export function shortDate(s: string | null | undefined): string {
  if (!s) return "";
  return /^\d{4}-\d{2}-\d{2}/.test(s) ? s.slice(0, 10) : s.length > 16 ? s.slice(0, 15) + "…" : s;
}

export function host(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** The questions of a request, as text. */
export function questionsOf(queries: unknown[] | undefined): string[] {
  return (queries ?? []).map((q) => (typeof q === "string" ? q : String((q as { q?: string }).q ?? "")));
}
