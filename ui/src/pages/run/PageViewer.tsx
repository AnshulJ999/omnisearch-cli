import { useQuery } from "@tanstack/react-query";
import { ExternalLink } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";
import Markdown from "@/components/Markdown";
import { Badge, Dialog, ErrorBox, Loading } from "@/components/ui";
import { get, runPath } from "@/lib/api";
import { cn, shortDate, tokens } from "@/lib/format";
import type { PageEntry } from "@/lib/types";

/** One saved page with its outline; opens at a section when asked. */
export default function PageViewer({ runId, page, section, onClose, onSection }: { runId: string; page: PageEntry | undefined; section?: number; onClose: () => void; onSection: (s?: number) => void }) {
  const open = !!page;
  const q = useQuery({
    queryKey: ["page", runId, page?.id],
    queryFn: () => get<{ text: string }>(`${runPath(runId)}/pages/${page!.id}`),
    enabled: open && !!page?.ok,
  });
  const lines = useMemo(() => (q.data?.text ?? "").split("\n"), [q.data]);
  const sec = page?.sections.find((s) => s.id === section);
  const body = sec ? lines.slice(sec.start, sec.end).join("\n") : q.data?.text ?? "";
  const top = useRef<HTMLDivElement>(null);
  useEffect(() => top.current?.scrollIntoView({ block: "start" }), [section, page?.id]);
  if (!page) return null;
  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()} title={`${page.id} · ${page.title || page.url}`} wide>
      <div ref={top} className="mb-3 flex flex-wrap items-center gap-2 text-xs text-muted">
        <a href={page.final_url || page.url} target="_blank" rel="noreferrer noopener" className="inline-flex items-center gap-1 text-accent hover:underline">
          <ExternalLink className="h-3 w-3" /> {page.final_url || page.url}
        </a>
        <Badge>tier {page.tier} · {page.fetcher || "?"}</Badge>
        {page.ok ? <Badge tone="ok">{tokens(page.tokens)} tokens</Badge> : <Badge tone="bad">{page.failure ?? "failed"}</Badge>}
        {page.stamp && <Badge tone="info">{page.stamp}</Badge>}
        {page.published && <Badge>dated {shortDate(page.published)}</Badge>}
      </div>
      {page.note && <p className="mb-3 rounded-md bg-warn-soft px-3 py-2 text-xs text-warn">{page.note}</p>}
      {!page.ok ? (
        <div className="space-y-2 text-sm">
          <p>This page was not read: {page.message || page.failure}.</p>
          <Attempts page={page} />
        </div>
      ) : (
        <div className="grid gap-4 lg:grid-cols-[220px_minmax(0,1fr)]">
          <nav aria-label="Page outline" className="max-h-[60vh] overflow-y-auto border-line text-xs lg:border-r lg:pr-3">
            <button type="button" className={cn("block w-full truncate rounded px-2 py-1 text-left hover:bg-panel-2", !section && "bg-accent-soft text-accent")} onClick={() => onSection(undefined)}>
              Whole page
            </button>
            {page.sections.map((s) => (
              <button
                key={s.id}
                type="button"
                title={s.title}
                className={cn("block w-full truncate rounded px-2 py-1 text-left hover:bg-panel-2", section === s.id && "bg-accent-soft text-accent")}
                style={{ paddingLeft: `${0.5 + Math.max(0, s.level - 1) * 0.6}rem` }}
                onClick={() => onSection(s.id)}
              >
                <span className="text-muted">#{s.id}</span> {s.title}
              </button>
            ))}
            <div className="mt-3 border-t border-line pt-2">
              <Attempts page={page} />
            </div>
          </nav>
          <div className="min-w-0">{q.isLoading ? <Loading what="the page" /> : q.isError ? <ErrorBox error={q.error} /> : <Markdown text={body} />}</div>
        </div>
      )}
    </Dialog>
  );
}

function Attempts({ page }: { page: PageEntry }) {
  if (!page.attempts.length) return null;
  return (
    <div>
      <p className="mb-1 font-medium text-muted">Fetch attempts</p>
      <ol className="space-y-1">
        {page.attempts.map((a, i) => (
          <li key={i} className="flex gap-2">
            <span className={cn("tabular", a.outcome.startsWith("ok") ? "text-ok" : "text-bad")}>{a.outcome.startsWith("ok") ? "ok" : "no"}</span>
            <span className="min-w-0 flex-1 break-words">
              tier {a.tier} {a.fetcher}: {a.outcome} <span className="text-muted">({Math.round(a.ms)} ms)</span>
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}
