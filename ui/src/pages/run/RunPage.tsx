import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, FolderOpen, RefreshCw, Sparkles } from "lucide-react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { toast } from "sonner";
import Markdown from "@/components/Markdown";
import { Badge, Button, Card, Empty, ErrorBox, Hint, Loading, PageHeader, TabPanel, Tabs } from "@/components/ui";
import { post } from "@/lib/api";
import { questionsOf, runTime, usd, when } from "@/lib/format";
import { useRun } from "./data";
import Overview from "./Overview";
import PagesTab from "./PagesTab";
import PageViewer from "./PageViewer";
import ResultsTab from "./ResultsTab";
import SearchesTab from "./SearchesTab";
import Timeline from "./Timeline";

const TABS = [
  { value: "overview", label: "Overview" },
  { value: "searches", label: "Searches" },
  { value: "results", label: "Results" },
  { value: "pages", label: "Pages" },
  { value: "timeline", label: "Timeline" },
  { value: "summary", label: "Summary" },
  { value: "synthesis", label: "Synthesis" },
];

export default function RunPage() {
  const { id = "", tab = "overview" } = useParams();
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  const run = useRun(id);
  const qc = useQueryClient();
  const synth = useMutation({
    mutationFn: () => post("/jobs", { kind: "synth", run: id }),
    onSuccess: () => {
      toast.success("Synthesis started; it shows here when it lands.");
      setTimeout(() => qc.invalidateQueries({ queryKey: ["run", id] }), 2000);
    },
    onError: (e) => toast.error(String(e)),
  });

  if (run.isLoading) return <Loading what="the run" />;
  if (run.isError || !run.data) return <ErrorBox error={run.error ?? "no data"} onRetry={() => run.refetch()} />;
  const r = run.data;
  const topic = r.rounds[0]?.request.topic || questionsOf(r.rounds[0]?.request.queries)[0] || id;
  const cost = r.index.reduce((s, l) => s + l.cost_usd, 0);
  const openPage = (p: string, s?: number) => setParams((prev) => {
    const n = new URLSearchParams(prev);
    n.set("page", s ? `${p}#${s}` : p);
    return n;
  });
  const [pageId, sec] = (params.get("page") ?? "").split("#");
  const page = r.pages.find((p) => p.id === pageId && (p.ok || !r.pages.some((q) => q.id === pageId && q.ok)));

  return (
    <>
      <Link to="/runs" className="mb-3 inline-flex items-center gap-1 text-xs text-muted hover:text-text">
        <ArrowLeft className="h-3 w-3" /> All runs
      </Link>
      <PageHeader
        title={topic}
        sub={
          <span className="flex flex-wrap items-center gap-2">
            <span>{when(runTime(id))}</span>
            <span>·</span>
            <span className="tabular">{usd(cost)}</span>
            {r.rounds.length > 1 && <Badge tone="info">{r.rounds.length} rounds</Badge>}
            {r.rounds.some((x) => x.fetching) && <Badge tone="warn">pages still fetching</Badge>}
            {r.synthesis_running && <Badge tone="warn">synthesis writing</Badge>}
            <Hint text="The run folder's name">
              <span className="inline-flex items-center gap-1 font-mono text-[11px]">
                <FolderOpen className="h-3 w-3" /> {id}
              </span>
            </Hint>
          </span>
        }
        right={
          <Button size="sm" variant="ghost" onClick={() => run.refetch()} aria-label="Reload this run">
            <RefreshCw className="h-3.5 w-3.5" /> Reload
          </Button>
        }
      />
      <Tabs value={tab} onChange={(v) => nav(`/runs/${encodeURIComponent(id)}${v === "overview" ? "" : `/${v}`}${params.toString() ? `?${params}` : ""}`)} items={TABS}>
        <TabPanel value="overview">
          <Overview run={r} openPage={openPage} />
        </TabPanel>
        <TabPanel value="searches">
          <SearchesTab run={r} />
        </TabPanel>
        <TabPanel value="results">
          <ResultsTab run={r} openPage={openPage} />
        </TabPanel>
        <TabPanel value="pages">
          <PagesTab run={r} openPage={openPage} />
        </TabPanel>
        <TabPanel value="timeline">
          <Timeline id={id} />
        </TabPanel>
        <TabPanel value="summary">
          <Card className="p-5">{r.summary ? <Markdown text={r.summary} onCite={openPage} /> : <Empty title="No summary saved">The run stopped before writing one.</Empty>}</Card>
        </TabPanel>
        <TabPanel value="synthesis">
          <Card className="p-5">
            {r.synthesis_running ? (
              <Empty title="The synthesis is still being written">This page checks again every few seconds.</Empty>
            ) : r.synthesis ? (
              <Markdown text={r.synthesis} onCite={openPage} />
            ) : (
              <Empty title="This run has no synthesis" icon={<Sparkles className="h-6 w-6" />}>
                <p>A model can read the run's pages and write a cited answer. It costs a few cents on the configured endpoint.</p>
                <Button className="mt-3" variant="primary" busy={synth.isPending} onClick={() => synth.mutate()}>
                  Write a synthesis
                </Button>
              </Empty>
            )}
          </Card>
        </TabPanel>
      </Tabs>
      <PageViewer
        runId={id}
        page={page}
        section={sec ? Number(sec) : undefined}
        onClose={() =>
          setParams((prev) => {
            const n = new URLSearchParams(prev);
            n.delete("page");
            return n;
          })
        }
        onSection={(s) => pageId && openPage(pageId, s)}
      />
    </>
  );
}
