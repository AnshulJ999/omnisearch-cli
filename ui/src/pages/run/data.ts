import { useQuery } from "@tanstack/react-query";
import { get, runPath } from "@/lib/api";
import type { Call, Pick, ResultRow, RunDetail } from "@/lib/types";

export const useRun = (id: string) =>
  useQuery({
    queryKey: ["run", id],
    queryFn: () => get<RunDetail>(runPath(id)),
    refetchInterval: (q) => (q.state.data && (q.state.data.synthesis_running || q.state.data.rounds.some((r) => r.fetching)) ? 3000 : false),
  });
export const useCalls = (id: string) => useQuery({ queryKey: ["calls", id], queryFn: () => get<Call[]>(`${runPath(id)}/calls`) });
export const useResults = (id: string) => useQuery({ queryKey: ["results", id], queryFn: () => get<ResultRow[]>(`${runPath(id)}/results`) });

/** One search line of a run: its label, mode, text and counts, from the summaries' Queries lists. */
export interface SearchLine {
  label: string;
  mode: string;
  text: string;
  links: number;
  fresh: number;
  engines: string;
  round: number;
}

const LINE = /^- (q[\w]+) \[([\w-]+)\] (.*): (\d+) links?, (\d+) new(?: · (.*))?$/;

export function searchLines(run: RunDetail): SearchLine[] {
  const out: SearchLine[] = [];
  for (const r of run.rounds) {
    const s = r.summary ?? "";
    const start = s.indexOf("## Queries");
    if (start < 0) continue;
    const end = s.indexOf("\n## ", start + 5);
    for (const line of s.slice(start, end < 0 ? undefined : end).split("\n")) {
      const m = LINE.exec(line.trim());
      if (m) out.push({ label: m[1], mode: m[2], text: m[3], links: +m[4], fresh: +m[5], engines: m[6] ?? "", round: r.round });
    }
  }
  return out;
}

export function allPicks(run: RunDetail): Pick[] {
  return run.rounds.flatMap((r) => r.picks ?? []);
}

/** Page ids the synthesis cites ([p3#4], [p3]). */
export function citedPages(synthesis: string | null): Set<string> {
  const set = new Set<string>();
  for (const m of (synthesis ?? "").matchAll(/\[(p\d+)(?:#\d+)?\]/g)) set.add(m[1]);
  return set;
}

export const WHY_LABEL: Record<string, string> = {
  url: "asked by address",
  question: "best of its question",
  side: "best of a side search",
  solo: "found by one engine only",
  rank: "by overall rank",
  spare: "spare (stands in for slow or walled pages)",
};

export const WHY_TONE: Record<string, "accent" | "info" | "ok" | "warn" | "neutral"> = {
  url: "neutral",
  question: "accent",
  side: "info",
  solo: "warn",
  rank: "ok",
  spare: "neutral",
};
