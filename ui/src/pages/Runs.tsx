import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import { Link, useNavigate } from "react-router";
import DataTable from "@/components/DataTable";
import { Badge, Card, Empty, ErrorBox, Loading, PageHeader } from "@/components/ui";
import { get } from "@/lib/api";
import { runTime, usd, when } from "@/lib/format";
import type { RunRow } from "@/lib/types";

const columns: ColumnDef<RunRow, unknown>[] = [
  {
    id: "when",
    header: "When",
    accessorFn: (r) => r.id,
    cell: ({ row }) => <span className="whitespace-nowrap text-muted">{when(runTime(row.original.id))}</span>,
  },
  {
    id: "topic",
    header: "Topic",
    accessorFn: (r) => `${r.topic} ${r.queries.join(" ")}`,
    cell: ({ row }) => (
      <div className="min-w-[16rem] max-w-[40rem]">
        <Link to={`/runs/${encodeURIComponent(row.original.id)}`} className="font-medium hover:text-accent hover:underline" onClick={(e) => e.stopPropagation()}>
          {row.original.topic || row.original.id}
        </Link>
        {row.original.queries.length > 1 && <div className="mt-0.5 truncate text-xs text-muted">{row.original.queries.length} questions: {row.original.queries.slice(0, 3).join(" · ")}</div>}
      </div>
    ),
  },
  {
    id: "kind",
    header: "Kind",
    accessorFn: (r) => r.kind,
    cell: ({ row }) => (
      <div className="flex flex-wrap gap-1">
        <Badge tone={row.original.kind === "unfinished" ? "warn" : "neutral"}>{row.original.kind || "research"}</Badge>
        {row.original.rounds > 1 && <Badge tone="info">{row.original.rounds} rounds</Badge>}
        {row.original.synthesis && <Badge tone="accent">synthesis</Badge>}
      </div>
    ),
  },
  { id: "links", header: "Links", accessorFn: (r) => r.urls, meta: { num: true } },
  { id: "pages", header: "Pages", accessorFn: (r) => r.pages, meta: { num: true } },
  { id: "cost", header: "Cost", accessorFn: (r) => r.cost_usd, cell: ({ getValue }) => usd(getValue() as number), meta: { num: true } },
];

export default function RunsPage() {
  const runs = useQuery({ queryKey: ["runs"], queryFn: () => get<RunRow[]>("/runs") });
  const nav = useNavigate();
  return (
    <>
      <PageHeader title="Runs" sub="Every search osearch has run, from the CLI and from here. Click one to see what it did." />
      <Card>
        {runs.isLoading ? (
          <Loading what="runs" />
        ) : runs.isError ? (
          <ErrorBox error={runs.error} onRetry={() => runs.refetch()} />
        ) : (
          <DataTable
            data={runs.data ?? []}
            columns={columns}
            filterPlaceholder="Filter by topic or question"
            initialSort={[{ id: "when", desc: true }]}
            onRowClick={(r) => nav(`/runs/${encodeURIComponent(r.id)}`)}
            empty={<Empty title="No runs yet">Start one from Search, or with `osearch "question"`.</Empty>}
          />
        )}
      </Card>
    </>
  );
}
