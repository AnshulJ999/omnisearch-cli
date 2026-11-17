import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import ReactECharts from "echarts-for-react";
import { useMemo } from "react";
import DataTable from "@/components/DataTable";
import { Badge, Card, CardHead, ErrorBox, Loading, PageHeader, Stat } from "@/components/ui";
import { get } from "@/lib/api";
import { ms, num, pct, runTime, tokens } from "@/lib/format";
import type { EngineNums, FetcherNums, Insights } from "@/lib/types";
import { WHY_LABEL, WHY_TONE } from "./run/data";

type EngineRow = EngineNums & { name: string };
type FetcherRow = FetcherNums & { name: string };

const engineCols: ColumnDef<EngineRow, unknown>[] = [
  { id: "name", header: "Engine", accessorFn: (r) => r.name, cell: ({ getValue }) => <span className="font-medium">{getValue() as string}</span> },
  { id: "calls", header: "Calls", accessorFn: (r) => r.calls, meta: { num: true } },
  { id: "failed", header: "Failed", accessorFn: (r) => (r.calls ? r.failed / r.calls : 0), cell: ({ row }) => pct(row.original.failed, row.original.calls), meta: { num: true } },
  { id: "speed", header: "Average call", accessorFn: (r) => (r.calls ? r.ms / r.calls : 0), cell: ({ getValue }) => ((getValue() as number) ? ms(getValue() as number) : "–"), meta: { num: true } },
  { id: "links", header: "Links", accessorFn: (r) => r.links, cell: ({ getValue }) => num(getValue() as number), meta: { num: true } },
  { id: "alone", header: "Found alone", accessorFn: (r) => r.alone, cell: ({ getValue }) => num(getValue() as number), meta: { num: true } },
  { id: "aloneShare", header: "Alone share", accessorFn: (r) => (r.links ? r.alone / r.links : 0), cell: ({ row }) => pct(row.original.alone, row.original.links), meta: { num: true } },
  { id: "picked", header: "Picked to read", accessorFn: (r) => r.picked, cell: ({ getValue }) => num(getValue() as number), meta: { num: true } },
  { id: "read", header: "Read", accessorFn: (r) => r.read, cell: ({ row }) => `${num(row.original.read)} (${pct(row.original.read, row.original.picked)})`, meta: { num: true } },
];

const fetcherCols: ColumnDef<FetcherRow, unknown>[] = [
  { id: "name", header: "Fetcher", accessorFn: (r) => r.name, cell: ({ getValue }) => <span className="font-medium">{getValue() as string}</span> },
  { id: "tries", header: "Tries", accessorFn: (r) => r.tries, cell: ({ getValue }) => num(getValue() as number), meta: { num: true } },
  { id: "ok", header: "Worked", accessorFn: (r) => (r.tries ? r.ok / r.tries : 0), cell: ({ row }) => `${num(row.original.ok)} (${pct(row.original.ok, row.original.tries)})`, meta: { num: true } },
  { id: "speed", header: "Average try", accessorFn: (r) => (r.tries ? r.ms / r.tries : 0), cell: ({ getValue }) => ms(getValue() as number), meta: { num: true } },
];

export default function InsightsPage() {
  const q = useQuery({ queryKey: ["insights"], queryFn: () => get<Insights>("/insights"), staleTime: 120_000 });
  const dark = document.documentElement.classList.contains("dark");
  const engines = useMemo(() => Object.entries(q.data?.engines ?? {}).map(([name, e]) => ({ name, ...e })).filter((e) => e.links || e.calls), [q.data]);
  const fetchers = useMemo(() => Object.entries(q.data?.fetchers ?? {}).map(([name, f]) => ({ name, ...f })), [q.data]);
  const runs = q.data?.runs ?? [];
  const perDay = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of runs) {
      const d = runTime(r.id);
      if (!d) continue;
      const k = d.toISOString().slice(0, 10);
      m.set(k, (m.get(k) ?? 0) + 1);
    }
    return [...m.entries()].sort();
  }, [runs]);

  if (q.isLoading) return <Loading what="numbers across every run (the first time reads every run folder)" />;
  if (q.isError || !q.data) return <ErrorBox error={q.error} onRetry={() => q.refetch()} />;
  const text = dark ? "#9a9a95" : "#6b6b66";
  const grid = dark ? "#2f302e" : "#e3e3de";
  const accent = dark ? "#2dd4bf" : "#0f766e";
  const top = [...engines].sort((a, b) => b.alone - a.alone).slice(0, 16);
  const pagesOk = runs.reduce((s, r) => s + r.pages_ok, 0);
  const pagesFailed = runs.reduce((s, r) => s + r.pages_failed, 0);
  const secs = runs.map((r) => r.seconds).filter(Boolean).sort((a, b) => a - b);
  const median = secs.length ? secs[Math.floor(secs.length / 2)] : 0;
  const whyTotal = Object.values(q.data.why).reduce((s, n) => s + n, 0);

  return (
    <>
      <PageHeader title="Insights" sub={`Numbers across all ${runs.length} runs: which engines bring what, how fetching goes, where time goes.`} />
      <div className="mb-6 grid grid-cols-2 gap-3 md:grid-cols-4">
        <Stat label="Runs" value={num(runs.length)} sub={`${runs.filter((r) => r.synthesis).length} with a synthesis`} />
        <Stat label="Pages read" value={num(pagesOk)} sub={`${pct(pagesOk, pagesOk + pagesFailed)} of those picked`} />
        <Stat label="Text read" value={tokens(runs.reduce((s, r) => s + r.tokens, 0))} sub="tokens" />
        <Stat label="Median run" value={ms(median * 1000)} sub="first call to last call" />
      </div>

      <div className="grid gap-6 xl:grid-cols-2">
        <Card>
          <CardHead title="Links each engine found that no other did" hint="What an engine adds beyond the others: the number to watch when choosing the standard set." />
          <ReactECharts
            style={{ height: 380 }}
            notMerge
            option={{
              grid: { left: 100, right: 30, top: 10, bottom: 30 },
              tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
              xAxis: { type: "value", axisLabel: { color: text }, splitLine: { lineStyle: { color: grid } } },
              yAxis: { type: "category", inverse: true, data: top.map((e) => e.name), axisLabel: { color: text } },
              series: [{ type: "bar", data: top.map((e) => e.alone), itemStyle: { color: accent, borderRadius: [0, 3, 3, 0] }, label: { show: true, position: "right", color: text } }],
            }}
          />
        </Card>
        <Card>
          <CardHead title="Runs per day" />
          <ReactECharts
            style={{ height: 380 }}
            notMerge
            option={{
              grid: { left: 40, right: 20, top: 10, bottom: 50 },
              tooltip: { trigger: "axis" },
              xAxis: { type: "category", data: perDay.map(([d]) => d), axisLabel: { color: text, rotate: 40 } },
              yAxis: { type: "value", minInterval: 1, axisLabel: { color: text }, splitLine: { lineStyle: { color: grid } } },
              series: [{ type: "bar", data: perDay.map(([, n]) => n), itemStyle: { color: accent, borderRadius: [3, 3, 0, 0] } }],
            }}
          />
        </Card>
      </div>

      <Card className="mt-6">
        <CardHead title="Engines" hint="Calls and speed from each run's call log; links, found alone, picked and read from each run's results. Click a header to sort." />
        <DataTable data={engines} columns={engineCols} pageSize={60} initialSort={[{ id: "alone", desc: true }]} />
      </Card>

      <div className="mt-6 grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHead title="Fetchers" hint="Every attempt to read a page, by the fetcher that tried. A page can take several tries." />
          <DataTable data={fetchers} columns={fetcherCols} pageSize={60} initialSort={[{ id: "tries", desc: true }]} />
        </Card>
        <Card>
          <CardHead title="Why pages were picked" hint="Across runs that recorded it (runs from 2026-11-17 on)." />
          {whyTotal === 0 ? (
            <p className="p-4 text-sm text-muted">No run has a pick record yet; new runs write one.</p>
          ) : (
            <ul className="space-y-2 p-4 text-sm">
              {Object.entries(q.data.why)
                .sort((a, b) => b[1] - a[1])
                .map(([k, v]) => (
                  <li key={k} className="flex items-center justify-between gap-3">
                    <Badge tone={WHY_TONE[k] ?? "neutral"}>{WHY_LABEL[k] ?? k}</Badge>
                    <span className="tabular text-muted">
                      {num(v)} · {pct(v, whyTotal)}
                    </span>
                  </li>
                ))}
            </ul>
          )}
        </Card>
      </div>
    </>
  );
}
