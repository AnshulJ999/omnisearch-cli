import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, ExternalLink, FlaskConical, Plus, Square, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";
import { toast } from "sonner";
import { Badge, Button, Card, CardHead, Empty, Input, Label, PageHeader, Select } from "@/components/ui";
import { get, post } from "@/lib/api";
import { ago, cn, runTime, usd } from "@/lib/format";
import type { Info, Job, QueryIn, RequestFile, RunRow } from "@/lib/types";

const MODES = ["general", "community", "news", "code", "deep", "academic", "forums", "social", "people", "scholar", "shopping"];

interface Draft {
  questions: { q: string; mode: string }[];
  since: string;
  width: string;
  fetch: string;
  synth: string;
  engines: string;
  sites: string;
}

const EMPTY: Draft = { questions: [{ q: "", mode: "general" }], since: "", width: "", fetch: "", synth: "", engines: "", sites: "" };

function loadDraft(): Draft {
  try {
    const d = JSON.parse(sessionStorage.getItem("osearch-draft") ?? "");
    if (Array.isArray(d?.questions)) return { ...EMPTY, ...d };
  } catch {
    /* no saved draft */
  }
  return EMPTY;
}

/** The request file for the form: only the fields the user set (the rest keep their tuned defaults). */
function toRequest(d: Draft): RequestFile {
  const list = (s: string) => s.split(",").map((x) => x.trim()).filter(Boolean);
  const queries: QueryIn[] = d.questions
    .filter((q) => q.q.trim())
    .map((q) => {
      const out: QueryIn = { q: q.q.trim() };
      if (q.mode !== "general") out.mode = q.mode;
      if (d.since) out.since = d.since;
      if (d.engines.trim()) out.engines = list(d.engines);
      if (d.sites.trim()) out.sites = list(d.sites);
      return out;
    });
  const req: RequestFile = { topic: queries[0]?.q ?? "", queries };
  if (d.width) req.width = d.width;
  if (d.fetch.trim() !== "") req.fetch = Number(d.fetch);
  if (d.synth) req.synth = d.synth === "on";
  return req;
}

export default function SearchPage() {
  const [draft, setDraft] = useState<Draft>(loadDraft);
  const [more, setMore] = useState(false);
  const qc = useQueryClient();
  useEffect(() => {
    try {
      sessionStorage.setItem("osearch-draft", JSON.stringify(draft));
    } catch {
      /* the draft just isn't kept */
    }
  }, [draft]);

  const info = useQuery({ queryKey: ["info"], queryFn: () => get<Info>("/info") });
  const noSynth = info.data ? !info.data.synth_configured : false;
  const jobs = useQuery({ queryKey: ["jobs"], queryFn: () => get<Job[]>("/jobs"), refetchInterval: (q) => (q.state.data?.some((j) => !j.ended) ? 1500 : false) });
  const start = useMutation({
    mutationFn: (body: Record<string, unknown>) => post<{ id: number }>("/jobs", body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["jobs"] }),
    onError: (e) => toast.error(`Couldn't start: ${e instanceof Error ? e.message : e}`),
  });

  const req = toRequest(draft);
  const ready = (req.queries?.length ?? 0) > 0;
  const setQ = (i: number, patch: Partial<Draft["questions"][number]>) =>
    setDraft((d) => ({ ...d, questions: d.questions.map((q, j) => (j === i ? { ...q, ...patch } : q)) }));

  return (
    <>
      <PageHeader title="Search" sub="Each question searches every web engine at once, reads the best pages, and keeps everything as a run." />
      <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,1fr)_380px]">
        <Card>
          <form
            className="space-y-4 p-4"
            onSubmit={(e) => {
              e.preventDefault();
              if (ready) start.mutate({ kind: "run", request: req });
            }}
          >
            <div className="space-y-2">
              <Label>Questions</Label>
              {draft.questions.map((q, i) => (
                <div key={i} className="flex flex-wrap gap-2 sm:flex-nowrap">
                  <Input
                    value={q.q}
                    autoFocus={i === 0}
                    placeholder={i === 0 ? "A plain question or a few keywords" : "Another angle on it"}
                    aria-label={`Question ${i + 1}`}
                    onChange={(e) => setQ(i, { q: e.target.value })}
                  />
                  <Select aria-label={`Mode of question ${i + 1}`} value={q.mode} onChange={(e) => setQ(i, { mode: e.target.value })} className="w-36">
                    {MODES.map((m) => (
                      <option key={m}>{m}</option>
                    ))}
                  </Select>
                  {draft.questions.length > 1 && (
                    <Button type="button" variant="ghost" size="icon" aria-label={`Remove question ${i + 1}`} onClick={() => setDraft((d) => ({ ...d, questions: d.questions.filter((_, j) => j !== i) }))}>
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  )}
                </div>
              ))}
              <Button type="button" variant="ghost" size="sm" onClick={() => setDraft((d) => ({ ...d, questions: [...d.questions, { q: "", mode: "general" }] }))}>
                <Plus className="h-3.5 w-3.5" /> Add a question
              </Button>
            </div>

            <button type="button" className="flex items-center gap-1 text-sm text-muted hover:text-text" aria-expanded={more} onClick={() => setMore(!more)}>
              {more ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />} More options
              <span className="text-xs">(left empty, each keeps its tuned default)</span>
            </button>
            {more && (
              <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
                <Field label="Only from the past">
                  <Select value={draft.since} onChange={(e) => setDraft({ ...draft, since: e.target.value })}>
                    <option value="">Any time</option>
                    <option value="day">Day</option>
                    <option value="week">Week</option>
                    <option value="month">Month</option>
                    <option value="year">Year</option>
                  </Select>
                </Field>
                <Field label="Width">
                  <Select value={draft.width} onChange={(e) => setDraft({ ...draft, width: e.target.value })}>
                    <option value="">Default</option>
                    <option value="standard">Standard (about six engines)</option>
                    <option value="wide">Wide (every web engine)</option>
                  </Select>
                </Field>
                <Field label="Pages to read" hint="Below the default is raised to it; 0 searches only.">
                  <Input type="number" min={0} value={draft.fetch} placeholder="Default" onChange={(e) => setDraft({ ...draft, fetch: e.target.value })} />
                </Field>
                <Field label="AI synthesis" hint={noSynth ? "Not set up: set synth.url and synth.model in Settings." : undefined}>
                  <Select value={noSynth ? "off" : draft.synth} disabled={noSynth} onChange={(e) => setDraft({ ...draft, synth: e.target.value })}>
                    <option value="">Default</option>
                    <option value="on">On</option>
                    <option value="off">Off</option>
                  </Select>
                </Field>
                <Field label="Only these engines" hint="Comma-separated; skips the side searches.">
                  <Input value={draft.engines} placeholder="All" onChange={(e) => setDraft({ ...draft, engines: e.target.value })} />
                </Field>
                <Field label="Only these sites" hint="Comma-separated domains.">
                  <Input value={draft.sites} placeholder="Any" onChange={(e) => setDraft({ ...draft, sites: e.target.value })} />
                </Field>
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2 border-t border-line pt-4">
              <Button type="submit" variant="primary" disabled={!ready} busy={start.isPending}>
                Search
              </Button>
              <Button type="button" disabled={!ready} onClick={() => setDraft(EMPTY)} variant="ghost">
                Clear
              </Button>
              <ResearchButton question={req.queries?.[0] && typeof req.queries[0] !== "string" ? req.queries[0].q : ""} start={(b) => start.mutate(b)} />
            </div>
          </form>
        </Card>

        <div className="space-y-6">
          <JobsCard jobs={jobs.data ?? []} />
          <RecentRuns />
        </div>
      </div>
    </>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1">
      <Label>{label}</Label>
      {children}
      {hint && <p className="text-xs text-muted">{hint}</p>}
    </div>
  );
}

/** Research mode costs far more than a search: its estimate comes first, starting it is a second click. */
function ResearchButton({ question, start }: { question: string; start: (b: Record<string, unknown>) => void }) {
  return (
    <Button type="button" variant="ghost" disabled={!question} title="The engines' own research agents: prints the cost estimate first, starts nothing" onClick={() => start({ kind: "research", question })}>
      <FlaskConical className="h-4 w-4" /> Research mode estimate
    </Button>
  );
}

function JobsCard({ jobs }: { jobs: Job[] }) {
  return (
    <Card>
      <CardHead title="Started here" hint="Searches you start on this page, newest first. They also land in Runs." />
      {jobs.length === 0 ? (
        <Empty title="Nothing started yet">Your searches show their progress here while they run.</Empty>
      ) : (
        <ul className="divide-y divide-line">
          {jobs.slice(0, 8).map((j) => (
            <JobRow key={j.id} job={j} />
          ))}
        </ul>
      )}
    </Card>
  );
}

function JobRow({ job }: { job: Job }) {
  const [open, setOpen] = useState(!job.ended);
  const qc = useQueryClient();
  const live = useQuery({ queryKey: ["job", job.id], queryFn: () => get<Job>(`/jobs/${job.id}`), enabled: open, refetchInterval: (q) => (q.state.data?.ended ? false : 1000) });
  const j = live.data ?? job;
  const stop = useMutation({ mutationFn: () => post(`/jobs/${job.id}/stop`), onSuccess: () => qc.invalidateQueries({ queryKey: ["jobs"] }) });
  const running = !j.ended;
  const failed = j.ended && j.code !== 0;
  const approve = j.kind === "research" && j.ended && j.code === 0;
  const out = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (out.current) out.current.scrollTop = out.current.scrollHeight;
  }, [j.lines?.length]);
  return (
    <li className="px-4 py-3">
      <div className="flex items-start gap-2">
        <button type="button" className="mt-0.5 text-muted" aria-label={open ? "Hide output" : "Show output"} onClick={() => setOpen(!open)}>
          {open ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        </button>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="truncate text-sm font-medium">{j.title}</span>
            {running ? <Badge tone="info">running</Badge> : failed ? <Badge tone="bad">failed ({j.code})</Badge> : <Badge tone="ok">done</Badge>}
            {j.kind !== "run" && <Badge>{j.kind}</Badge>}
          </div>
          {j.progress && (
            <p className="mt-1 text-xs text-muted tabular">
              {j.progress.calls} engine calls · {j.progress.pages_ok} pages read
              {j.progress.pages_failed ? ` · ${j.progress.pages_failed} failed` : ""}
              {j.progress.synthesis_running ? " · synthesis writing" : ""}
            </p>
          )}
          <div className="mt-2 flex flex-wrap gap-2">
            {j.run && (
              <Link to={`/runs/${encodeURIComponent(j.run)}`} className="inline-flex items-center gap-1 text-xs text-accent hover:underline">
                <ExternalLink className="h-3 w-3" /> Open the run
              </Link>
            )}
            {running && (
              <Button size="sm" variant="ghost" busy={stop.isPending} onClick={() => stop.mutate()}>
                <Square className="h-3 w-3" /> Stop
              </Button>
            )}
            {approve && <ApproveResearch question={j.title} />}
          </div>
        </div>
      </div>
      {open && j.lines && j.lines.length > 0 && (
        <pre ref={out} className="mt-2 max-h-72 overflow-auto rounded-md bg-panel-2 p-2 font-mono text-[11px] leading-relaxed whitespace-pre-wrap">
          {j.lines.join("\n")}
        </pre>
      )}
    </li>
  );
}

function ApproveResearch({ question }: { question: string }) {
  const qc = useQueryClient();
  const [sure, setSure] = useState(false);
  const go = useMutation({
    mutationFn: () => post("/jobs", { kind: "approve", question }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["jobs"] }),
    onError: (e) => toast.error(String(e)),
  });
  if (!sure)
    return (
      <Button size="sm" variant="secondary" onClick={() => setSure(true)}>
        Start research at this estimate…
      </Button>
    );
  return (
    <span className="flex flex-wrap items-center gap-2 text-xs">
      <span className="text-warn">This spends the estimate above.</span>
      <Button size="sm" variant="primary" busy={go.isPending} onClick={() => go.mutate()}>
        Start research
      </Button>
      <Button size="sm" variant="ghost" onClick={() => setSure(false)}>
        Cancel
      </Button>
    </span>
  );
}

function RecentRuns() {
  const runs = useQuery({ queryKey: ["runs"], queryFn: () => get<RunRow[]>("/runs") });
  const list = runs.data?.slice(0, 6) ?? [];
  return (
    <Card>
      <CardHead
        title="Recent runs"
        right={
          <Link to="/runs" className="text-xs text-accent hover:underline">
            All runs
          </Link>
        }
      />
      {runs.isLoading ? (
        <p className="p-4 text-sm text-muted">Loading…</p>
      ) : list.length === 0 ? (
        <Empty title="No runs yet">Runs appear here after your first search.</Empty>
      ) : (
        <ul className="divide-y divide-line">
          {list.map((r) => (
            <li key={r.id}>
              <Link to={`/runs/${encodeURIComponent(r.id)}`} className="block px-4 py-2.5 hover:bg-panel-2">
                <div className="truncate text-sm">{r.topic || r.id}</div>
                <div className={cn("mt-0.5 text-xs text-muted tabular")}>
                  {ago(runTime(r.id))} · {r.pages} {r.pages === 1 ? "page" : "pages"} · {usd(r.cost_usd)}
                </div>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
