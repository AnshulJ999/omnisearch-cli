import { useMemo } from "react";
import { Badge, Card, CardHead, Hint, Stat } from "@/components/ui";
import { cn, num, pct, tokens, usd } from "@/lib/format";
import type { RunDetail } from "@/lib/types";
import { allPicks, citedPages, searchLines, useResults, WHY_LABEL, WHY_TONE } from "./data";

/** The run as a funnel, from the questions asked to the pages the synthesis cited. */
export default function Overview({ run, openPage }: { run: RunDetail; openPage: (p: string) => void }) {
  const results = useResults(run.id);
  const lines = useMemo(() => searchLines(run), [run]);
  const picks = allPicks(run);
  const cited = citedPages(run.synthesis);
  const ok = run.pages.filter((p) => p.ok);
  const failed = run.pages.filter((p) => !p.ok);
  const sightings = results.data?.reduce((s, r) => s + r.found_by.length, 0) ?? lines.reduce((s, l) => s + l.links, 0);
  const unique = results.data?.length ?? 0;
  const own = run.rounds.reduce((s, r) => s + (r.request.queries?.length ?? 0), 0);
  const steps = [
    { label: "Questions asked", value: own, hint: "The request's own questions, every round." },
    { label: "Searches run", value: lines.length, hint: "Your questions plus the side searches osearch added (Reddit, forums, GitHub, Telegram...)." },
    { label: "Links found", value: sightings, hint: "Every result every engine returned, counting repeats." },
    { label: "Different links", value: unique, hint: "After merging the same address found by several engines." },
    { label: "Picked to read", value: picks.length || run.pages.length, hint: "Pages chosen for reading (see Pages for why each one)." },
    { label: "Read", value: ok.length, hint: "Pages fetched with usable text." },
    { label: "Cited by the synthesis", value: cited.size, hint: "Pages the AI synthesis quotes or cites." },
  ];
  const max = Math.max(...steps.map((s) => s.value), 1);
  const why: Record<string, number> = {};
  for (const p of picks) why[p.why] = (why[p.why] ?? 0) + 1;
  const cost = run.index.reduce((s, l) => s + l.cost_usd, 0);

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-6">
        <Stat label="Pages read" value={`${ok.length}`} sub={failed.length ? `${failed.length} failed` : "none failed"} />
        <Stat label="Text read" value={tokens(ok.reduce((s, p) => s + p.tokens, 0))} sub="tokens" />
        <Stat label="Different links" value={num(unique)} sub={`${num(sightings)} sightings`} />
        <Stat label="Searches" value={lines.length} sub={`${own} your own`} />
        <Stat label="Cost" value={usd(cost)} sub="estimated, list prices" />
        <Stat label="Synthesis" value={run.synthesis ? "yes" : run.synthesis_running ? "writing" : "no"} sub={run.synthesis ? `${cited.size} pages cited` : undefined} />
      </div>

      <Card>
        <CardHead title="From question to answer" hint="How much survived each step of this run." />
        <ol className="space-y-2 p-4">
          {steps.map((s, i) => (
            <li key={s.label} className="grid grid-cols-[10rem_minmax(0,1fr)_4rem] items-center gap-3 text-sm sm:grid-cols-[13rem_minmax(0,1fr)_5rem]">
              <Hint text={s.hint}>
                <span className="cursor-help truncate text-muted underline decoration-dotted underline-offset-2">{s.label}</span>
              </Hint>
              <div className="h-5 rounded bg-panel-2">
                <div className={cn("h-5 rounded", i < 2 ? "bg-info/60" : i < 4 ? "bg-accent/60" : "bg-ok/60")} style={{ width: `${Math.max(1.5, (s.value / max) * 100)}%` }} />
              </div>
              <span className="text-right tabular">{num(s.value)}</span>
            </li>
          ))}
        </ol>
      </Card>

      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHead title="Why pages were picked" hint="Each question's best result and each side search's best come first, then two links found by one engine only, then overall rank." />
          {picks.length === 0 ? (
            <p className="p-4 text-sm text-muted">This run predates the pick record (picks.rN.json), so the reasons were not saved. Each page still shows which search led to it.</p>
          ) : (
            <ul className="space-y-2 p-4 text-sm">
              {Object.entries(why)
                .sort((a, b) => b[1] - a[1])
                .map(([k, v]) => (
                  <li key={k} className="flex items-center justify-between gap-3">
                    <Badge tone={WHY_TONE[k] ?? "neutral"}>{WHY_LABEL[k] ?? k}</Badge>
                    <span className="tabular text-muted">
                      {v} · {pct(v, picks.length)}
                    </span>
                  </li>
                ))}
            </ul>
          )}
        </Card>
        <Card>
          <CardHead title="Questions" hint="As the request asked them, each round." />
          <ul className="divide-y divide-line">
            {run.rounds.map((r) =>
              (r.request.queries ?? []).map((q, i) => {
                const o = typeof q === "string" ? { q } : q;
                return (
                  <li key={`${r.round}-${i}`} className="flex flex-wrap items-center gap-2 px-4 py-2 text-sm">
                    <span className="font-mono text-xs text-muted">q{i + 1}{run.rounds.length > 1 ? ` r${r.round}` : ""}</span>
                    <span className="min-w-0 flex-1">{o.q}</span>
                    {o.mode && <Badge tone="accent">{o.mode}</Badge>}
                    {o.since && <Badge>past {o.since}</Badge>}
                  </li>
                );
              }),
            )}
          </ul>
        </Card>
      </div>

      {failed.length > 0 && (
        <Card>
          <CardHead title="Pages that were not read" hint="What stopped each one." />
          <ul className="divide-y divide-line text-sm">
            {failed.map((p) => (
              <li key={p.id + p.url} className="flex flex-wrap items-baseline gap-2 px-4 py-2">
                <button type="button" className="font-mono text-xs text-accent hover:underline" onClick={() => openPage(p.id)}>
                  {p.id}
                </button>
                <span className="min-w-0 flex-1 truncate">{p.url}</span>
                <Badge tone="bad">{p.failure ?? "failed"}</Badge>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </div>
  );
}
