import type { ColumnDef } from "@tanstack/react-table";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import DataTable from "@/components/DataTable";
import { Badge, Button, Card, CardHead, Dialog, ErrorBox, Loading } from "@/components/ui";
import { host, shortDate } from "@/lib/format";
import type { ResultRow, RunDetail } from "@/lib/types";
import { useResults } from "./data";

/** Every different link the run found, ranked by how many engines agreed. */
export default function ResultsTab({ run, openPage }: { run: RunDetail; openPage: (p: string) => void }) {
  const results = useResults(run.id);
  const [onlyAlone, setOnlyAlone] = useState(false);
  const [shown, setShown] = useState<ResultRow | null>(null);
  const read = useMemo(() => new Set(run.pages.filter((p) => p.ok).map((p) => p.id)), [run]);
  const data = useMemo(() => (results.data ?? []).filter((r) => !onlyAlone || r.engines.length === 1), [results.data, onlyAlone]);

  const columns: ColumnDef<ResultRow, unknown>[] = [
    { id: "rank", header: "Rank", accessorFn: (r) => r.rank, meta: { num: true } },
    {
      id: "title",
      header: "Result",
      accessorFn: (r) => `${r.title} ${r.url}`,
      cell: ({ row }) => (
        <div className="min-w-[18rem] max-w-[40rem]">
          <button type="button" className="text-left font-medium hover:text-accent hover:underline" onClick={() => setShown(row.original)}>
            {row.original.title || row.original.url}
          </button>
          <div className="truncate text-xs text-muted">{host(row.original.url)}</div>
        </div>
      ),
    },
    {
      id: "engines",
      header: "Engines",
      accessorFn: (r) => r.engines.length,
      meta: { num: true },
      cell: ({ row }) => (
        <span title={row.original.engines.join(", ")} className={row.original.engines.length === 1 ? "text-warn" : undefined}>
          {row.original.engines.length}
        </span>
      ),
    },
    { id: "who", header: "Found by", accessorFn: (r) => r.engines.join(", "), cell: ({ getValue }) => <span className="block max-w-[14rem] text-xs text-muted">{getValue() as string}</span> },
    { id: "searches", header: "Searches", accessorFn: (r) => r.queries.join(", "), cell: ({ getValue }) => <span className="font-mono text-xs text-muted">{getValue() as string}</span> },
    {
      id: "date",
      header: "Dated",
      accessorFn: (r) => r.found_by.find((f) => f.published)?.published ?? "",
      cell: ({ getValue }) => <span className="whitespace-nowrap text-xs text-muted">{shortDate(getValue() as string)}</span>,
    },
    {
      id: "page",
      header: "Read",
      accessorFn: (r) => r.page ?? "",
      cell: ({ row }) =>
        row.original.page ? (
          <button type="button" onClick={() => openPage(row.original.page!)}>
            <Badge tone={read.has(row.original.page) ? "ok" : "bad"}>{row.original.page}</Badge>
          </button>
        ) : (
          <span className="text-xs text-muted">–</span>
        ),
    },
  ];

  if (results.isLoading) return <Loading what="the results" />;
  if (results.isError) return <ErrorBox error={results.error} onRetry={() => results.refetch()} />;
  return (
    <Card>
      <CardHead
        title={`${results.data?.length ?? 0} different links`}
        hint={`Rank comes from how many engines found a link and how high. A number in orange means one engine found it alone.${run.rounds.length > 1 ? " Each round ranks its own results, so ranks repeat." : ""}`}
        right={
          <Button size="sm" variant={onlyAlone ? "primary" : "secondary"} onClick={() => setOnlyAlone(!onlyAlone)} aria-pressed={onlyAlone}>
            Found by one engine only
          </Button>
        }
      />
      <DataTable data={data} columns={columns} filterPlaceholder="Filter by title, address, engine" initialSort={[{ id: "rank", desc: false }]} />
      <Sightings row={shown} onClose={() => setShown(null)} />
    </Card>
  );
}

function Sightings({ row, onClose }: { row: ResultRow | null; onClose: () => void }) {
  const [open, setOpen] = useState<number | null>(0);
  if (!row) return null;
  return (
    <Dialog open onOpenChange={(v) => !v && onClose()} title={row.title || row.url} description={row.url} wide>
      <p className="mb-3 text-sm text-muted">Each engine's own title and snippet for this link.</p>
      <ul className="divide-y divide-line rounded-md border border-line">
        {row.found_by.map((s, i) => (
          <li key={i}>
            <button type="button" className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-panel-2" aria-expanded={open === i} onClick={() => setOpen(open === i ? null : i)}>
              {open === i ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
              <Badge tone="accent">{s.engine}</Badge>
              <span className="font-mono text-xs text-muted">{s.q} · #{s.rank}</span>
              <span className="min-w-0 flex-1 truncate">{s.title}</span>
              {s.published && <span className="text-xs text-muted">{shortDate(s.published)}</span>}
            </button>
            {open === i && <p className="px-9 pb-3 text-sm whitespace-pre-wrap break-words">{s.snippet || "(no snippet)"}</p>}
          </li>
        ))}
      </ul>
      <a className="mt-3 inline-block text-sm text-accent hover:underline" href={row.url} target="_blank" rel="noreferrer noopener">
        Open the link
      </a>
    </Dialog>
  );
}
