import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";
import DataTable from "@/components/DataTable";
import { Badge, Card, CardHead, Empty } from "@/components/ui";
import { ms } from "@/lib/format";
import type { RunDetail } from "@/lib/types";
import { allPicks, searchLines, useCalls, type SearchLine } from "./data";

interface Row extends SearchLine {
  side: boolean;
  calls: number;
  failed: number;
  slowest: number;
  picked: number;
}

/** Every search of the run: your questions and the side searches osearch added. */
export default function SearchesTab({ run }: { run: RunDetail }) {
  const calls = useCalls(run.id);
  const rows: Row[] = useMemo(() => {
    const picks = allPicks(run);
    return searchLines(run).map((l) => {
      const mine = (calls.data ?? []).filter((c) => c.label === l.label);
      return {
        ...l,
        side: !/^q\d+$/.test(l.label),
        calls: mine.length,
        failed: mine.filter((c) => !(c.status && c.status >= 200 && c.status < 300)).length,
        slowest: Math.max(0, ...mine.map((c) => c.elapsed_ms)),
        picked: picks.filter((p) => p.from === l.label).length,
      };
    });
  }, [run, calls.data]);

  const columns: ColumnDef<Row, unknown>[] = [
    {
      id: "label",
      header: "Search",
      accessorFn: (r) => r.label,
      cell: ({ row }) => (
        <div className="flex items-center gap-1.5">
          <span className="font-mono text-xs">{row.original.label}</span>
          {row.original.side ? <Badge tone="info">side</Badge> : <Badge tone="accent">yours</Badge>}
        </div>
      ),
    },
    { id: "mode", header: "Mode", accessorFn: (r) => r.mode, cell: ({ getValue }) => <Badge>{getValue() as string}</Badge> },
    { id: "text", header: "What it searched", accessorFn: (r) => r.text, cell: ({ getValue }) => <span className="block min-w-[18rem] max-w-[36rem] break-words">{getValue() as string}</span> },
    { id: "engines", header: "Engines (links each)", accessorFn: (r) => r.engines, cell: ({ getValue }) => <span className="block min-w-[10rem] text-xs text-muted">{(getValue() as string) || "–"}</span> },
    { id: "links", header: "Links", accessorFn: (r) => r.links, meta: { num: true } },
    { id: "new", header: "New", accessorFn: (r) => r.fresh, meta: { num: true } },
    { id: "picked", header: "Pages picked", accessorFn: (r) => r.picked, meta: { num: true } },
    {
      id: "calls",
      header: "Calls",
      accessorFn: (r) => r.calls,
      meta: { num: true },
      cell: ({ row }) => (
        <span>
          {row.original.calls}
          {row.original.failed > 0 && <span className="ml-1 text-bad">({row.original.failed} failed)</span>}
        </span>
      ),
    },
    { id: "slowest", header: "Slowest call", accessorFn: (r) => r.slowest, cell: ({ getValue }) => ((getValue() as number) ? ms(getValue() as number) : "–"), meta: { num: true } },
  ];

  return (
    <Card>
      <CardHead title={`${rows.length} searches`} hint="New is links no earlier search of the run had found. Empty searches (0 links) are worth a look: they cost time and found nothing." />
      <DataTable
        data={rows}
        columns={columns}
        filterPlaceholder="Filter searches"
        pageSize={100}
        empty={<Empty title="No search lines in this run's summary">Runs from before the summary listed its searches show nothing here; the Timeline tab still has every call.</Empty>}
        rowClass={(r) => (r.links === 0 ? "bg-warn-soft/40" : undefined)}
      />
    </Card>
  );
}
