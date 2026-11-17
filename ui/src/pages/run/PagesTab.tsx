import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";
import DataTable from "@/components/DataTable";
import { Badge, Card, CardHead, Hint } from "@/components/ui";
import { host, shortDate, tokens } from "@/lib/format";
import type { PageEntry, RunDetail } from "@/lib/types";
import { allPicks, citedPages, WHY_LABEL, WHY_TONE } from "./data";

interface Row extends PageEntry {
  why?: string;
  cited: boolean;
}

/** Every page the run picked: why, how it was fetched, and what came of it. */
export default function PagesTab({ run, openPage }: { run: RunDetail; openPage: (p: string) => void }) {
  const rows: Row[] = useMemo(() => {
    const picks = new Map(allPicks(run).map((p) => [p.id, p.why]));
    const cited = citedPages(run.synthesis);
    return run.pages.map((p) => ({ ...p, why: picks.get(p.id), cited: cited.has(p.id) }));
  }, [run]);

  const columns: ColumnDef<Row, unknown>[] = [
    {
      id: "id",
      header: "Page",
      accessorFn: (r) => Number(r.id.slice(1)),
      cell: ({ row }) => (
        <button type="button" className="font-mono text-xs text-accent hover:underline" onClick={() => openPage(row.original.id)}>
          {row.original.id}
        </button>
      ),
    },
    {
      id: "title",
      header: "Title",
      accessorFn: (r) => `${r.title} ${r.url}`,
      cell: ({ row }) => (
        <div className="min-w-[16rem] max-w-[34rem]">
          <button type="button" className="text-left hover:text-accent hover:underline" onClick={() => openPage(row.original.id)}>
            {row.original.title || row.original.url}
          </button>
          <div className="truncate text-xs text-muted">{host(row.original.final_url || row.original.url)}</div>
        </div>
      ),
    },
    {
      id: "why",
      header: "Why picked",
      accessorFn: (r) => r.why ?? "",
      cell: ({ row }) =>
        row.original.why ? (
          <Badge tone={WHY_TONE[row.original.why] ?? "neutral"}>{WHY_LABEL[row.original.why] ?? row.original.why}</Badge>
        ) : (
          <span className="text-xs text-muted">{row.original.late ? "late" : "–"}</span>
        ),
    },
    { id: "from", header: "From", accessorFn: (r) => r.from, cell: ({ getValue }) => <span className="font-mono text-xs">{getValue() as string}</span> },
    {
      id: "fetch",
      header: "Fetched by",
      accessorFn: (r) => r.fetcher,
      cell: ({ row }) => {
        const p = row.original;
        const tries = p.attempts.map((a) => `tier ${a.tier} ${a.fetcher}: ${a.outcome} (${Math.round(a.ms)} ms)`).join("\n");
        return (
          <Hint text={<span className="whitespace-pre-wrap">{tries || "no attempts recorded"}</span>}>
            <span className="cursor-help whitespace-nowrap text-xs">
              {p.fetcher || "–"} <span className="text-muted">t{p.tier}</span>
              {p.attempts.length > 1 && <span className="text-muted"> · {p.attempts.length} tries</span>}
            </span>
          </Hint>
        );
      },
    },
    {
      id: "result",
      header: "Result",
      accessorFn: (r) => (r.ok ? "ok" : r.failure ?? "failed"),
      cell: ({ row }) => (row.original.ok ? <Badge tone="ok">read</Badge> : <Badge tone="bad">{row.original.failure ?? "failed"}</Badge>),
    },
    { id: "tokens", header: "Tokens", accessorFn: (r) => r.tokens, cell: ({ getValue }) => tokens(getValue() as number), meta: { num: true } },
    {
      id: "copy",
      header: "Copy",
      accessorFn: (r) => r.stamp ?? "",
      cell: ({ row }) => <span className="block max-w-[14rem] text-xs text-muted">{row.original.stamp ?? ""}{row.original.published && !row.original.stamp?.includes("dated") ? ` · dated ${shortDate(row.original.published)}` : ""}</span>,
    },
    { id: "cited", header: "Cited", accessorFn: (r) => (r.cited ? 1 : 0), cell: ({ row }) => (row.original.cited ? <Badge tone="accent">cited</Badge> : null) },
  ];

  const ok = rows.filter((r) => r.ok).length;
  return (
    <Card>
      <CardHead title={`${ok} of ${rows.length} pages read`} hint="Hover Fetched by for every attempt. Copy says where the text came from (read live, a cache or an archive)." />
      <DataTable data={rows} columns={columns} filterPlaceholder="Filter pages" pageSize={100} initialSort={[{ id: "id", desc: false }]} rowClass={(r) => (!r.ok ? "opacity-80" : undefined)} />
    </Card>
  );
}
