import { Background, Controls, Handle, MarkerType, Position, ReactFlow, type Edge, type Node, type NodeProps } from "@xyflow/react";
import ELK from "elkjs/lib/elk.bundled.js";
import { Archive, Cpu, GitBranch, Play, SquareStack, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { Badge, Button, Card, ErrorBox, Loading, PageHeader, Tabs } from "@/components/ui";
import { cn } from "@/lib/format";
import type { ConfigAnswer, Setting } from "@/lib/types";
import flowsData from "@/flows/flows.json";
import { EditSetting, show, useConfig, valueOf } from "./Settings";

interface FlowNode {
  id: string;
  label: string;
  kind: "start" | "step" | "decision" | "engine" | "store";
  detail: string;
  settings: string[];
  code: string[];
}
interface Flow {
  id: string;
  title: string;
  intro: string;
  nodes: FlowNode[];
  edges: (string | undefined)[][];
}
const FLOWS = flowsData as Flow[];

const KIND = {
  start: { icon: Play, tone: "border-accent", label: "start" },
  step: { icon: SquareStack, tone: "border-line", label: "step" },
  decision: { icon: GitBranch, tone: "border-warn", label: "decision" },
  engine: { icon: Cpu, tone: "border-info", label: "outside calls" },
  store: { icon: Archive, tone: "border-ok", label: "saved" },
};

const W = 260;
const elk = new ELK();

type BoxData = { node: FlowNode; values?: ConfigAnswer["values"]; selected: boolean };

function Box({ data }: NodeProps<Node<BoxData>>) {
  const { node, values, selected } = data;
  const k = KIND[node.kind] ?? KIND.step;
  const Icon = k.icon;
  return (
    <div className={cn("rounded-lg border-2 bg-panel px-3 py-2 text-left shadow-sm", k.tone, selected && "ring-2 ring-accent")} style={{ width: W }}>
      <Handle type="target" position={Position.Top} className="!h-1.5 !w-1.5 !border-0 !bg-muted" />
      <div className="flex items-start gap-1.5">
        <Icon className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted" aria-hidden />
        <div className="text-[13px] font-semibold leading-snug">{node.label}</div>
      </div>
      {node.settings.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {node.settings.slice(0, 6).map((s) => (
            <span key={s} className="rounded bg-panel-2 px-1 py-0.5 font-mono text-[10px] text-muted">
              {s.split(".")[1]}={values ? short(show(valueOf(values, s))) : "…"}
            </span>
          ))}
          {node.settings.length > 6 && <span className="text-[10px] text-muted">+{node.settings.length - 6} more</span>}
        </div>
      )}
      <Handle type="source" position={Position.Bottom} className="!h-1.5 !w-1.5 !border-0 !bg-muted" />
    </div>
  );
}

const short = (s: string) => (s.length > 18 ? s.slice(0, 17) + "…" : s);
const nodeTypes = { box: Box };

/** Box height from its content, so the layout leaves room. */
const heightOf = (n: FlowNode) => 44 + (n.settings.length ? 22 * Math.ceil(Math.min(7, n.settings.length) / 2) : 0);

function useLayout(flow: Flow) {
  const [pos, setPos] = useState<Record<string, { x: number; y: number }> | null>(null);
  useEffect(() => {
    let live = true;
    setPos(null);
    elk
      .layout({
        id: flow.id,
        layoutOptions: { "elk.algorithm": "layered", "elk.direction": "DOWN", "elk.spacing.nodeNode": "40", "elk.layered.spacing.nodeNodeBetweenLayers": "56", "elk.edgeRouting": "SPLINES" },
        children: flow.nodes.map((n) => ({ id: n.id, width: W, height: heightOf(n) })),
        edges: flow.edges.map((e, i) => ({ id: `e${i}`, sources: [e[0]!], targets: [e[1]!] })),
      })
      .then((g) => {
        if (live) setPos(Object.fromEntries((g.children ?? []).map((c) => [c.id, { x: c.x ?? 0, y: c.y ?? 0 }])));
      });
    return () => {
      live = false;
    };
  }, [flow]);
  return pos;
}

export default function HowPage() {
  const { flow: flowId = "run" } = useParams();
  const nav = useNavigate();
  const flow = FLOWS.find((f) => f.id === flowId) ?? FLOWS[0];
  const cfg = useConfig();
  const pos = useLayout(flow);
  const [sel, setSel] = useState<string | null>(null);
  const [editing, setEditing] = useState<Setting | null>(null);
  useEffect(() => setSel(null), [flow.id]);
  const dark = document.documentElement.classList.contains("dark");

  const nodes: Node<BoxData>[] = useMemo(
    () => (pos ? flow.nodes.map((n) => ({ id: n.id, type: "box", position: pos[n.id] ?? { x: 0, y: 0 }, data: { node: n, values: cfg.data?.values, selected: sel === n.id } })) : []),
    [pos, flow, cfg.data, sel],
  );
  const edges: Edge[] = useMemo(
    () =>
      flow.edges.map((e, i) => ({
        id: `e${i}`,
        source: e[0]!,
        target: e[1]!,
        label: e[2],
        markerEnd: { type: MarkerType.ArrowClosed },
        labelStyle: { fontSize: 11, fill: dark ? "#9a9a95" : "#6b6b66" },
        labelBgStyle: { fill: dark ? "#121312" : "#f7f7f5" },
      })),
    [flow, dark],
  );
  const node = flow.nodes.find((n) => n.id === sel);
  const settings = new Map((cfg.data?.settings ?? []).map((s) => [s.key, s]));

  return (
    <>
      <PageHeader title="How it works" sub="What osearch does, step by step, with the settings that steer each step at their current values. Click a box for details." />
      <Tabs value={flow.id} onChange={(v) => nav(`/how/${v}`)} items={FLOWS.map((f) => ({ value: f.id, label: f.title }))}>
        <p className="mt-3 text-sm text-muted">{flow.intro}</p>
      </Tabs>
      {cfg.isError && <ErrorBox error={cfg.error} />}
      <div className="mt-4 grid gap-4 lg:grid-cols-[minmax(0,1fr)_360px]">
        <Card className="h-[72vh] min-h-[480px] overflow-hidden">
          {!pos ? (
            <Loading what="the map" />
          ) : (
            <ReactFlow
              key={flow.id}
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              fitView
              fitViewOptions={{ padding: 0.15 }}
              nodesDraggable={false}
              nodesConnectable={false}
              onNodeClick={(_, n) => setSel(n.id)}
              onPaneClick={() => setSel(null)}
              minZoom={0.2}
              proOptions={{ hideAttribution: true }}
              colorMode={dark ? "dark" : "light"}
            >
              <Background gap={20} size={1} />
              <Controls showInteractive={false} />
            </ReactFlow>
          )}
        </Card>
        <Card className="max-h-[72vh] overflow-y-auto">
          {!node ? (
            <div className="space-y-3 p-4 text-sm text-muted">
              <p>Click a box to see what it does, the settings that steer it, and where it lives in the code.</p>
              <ul className="space-y-1.5">
                {Object.entries(KIND).map(([k, v]) => (
                  <li key={k} className="flex items-center gap-2">
                    <span className={cn("h-3 w-5 rounded border-2", v.tone)} /> {v.label}
                  </li>
                ))}
              </ul>
            </div>
          ) : (
            <div className="p-4">
              <div className="mb-2 flex items-start justify-between gap-2">
                <h2 className="text-base font-semibold">{node.label}</h2>
                <Button size="icon" variant="ghost" aria-label="Close details" onClick={() => setSel(null)}>
                  <X className="h-4 w-4" />
                </Button>
              </div>
              <Badge>{KIND[node.kind]?.label}</Badge>
              <p className="mt-3 text-sm leading-relaxed">{node.detail}</p>
              {node.settings.length > 0 && (
                <>
                  <h3 className="mt-4 mb-2 text-xs font-semibold text-muted">Settings</h3>
                  <ul className="space-y-2">
                    {node.settings.map((key) => {
                      const s = settings.get(key);
                      return (
                        <li key={key} className="rounded-md border border-line p-2">
                          <div className="flex items-center justify-between gap-2">
                            <span className="font-mono text-xs">{key}</span>
                            {s && (
                              <Button size="sm" variant="ghost" onClick={() => setEditing(s)}>
                                Change
                              </Button>
                            )}
                          </div>
                          <div className="mt-0.5 text-sm font-medium tabular">{cfg.data ? show(valueOf(cfg.data.values, key)) : "…"}</div>
                          {s && <div className="mt-0.5 text-xs text-muted">{s.about}</div>}
                        </li>
                      );
                    })}
                  </ul>
                </>
              )}
              <h3 className="mt-4 mb-2 text-xs font-semibold text-muted">In the code</h3>
              <ul className="space-y-1">
                {node.code.map((c) => (
                  <li key={c} className="font-mono text-xs break-words">
                    {c}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </Card>
      </div>
      {editing && cfg.data && <EditSetting setting={editing} current={valueOf(cfg.data.values, editing.key)} onClose={() => setEditing(null)} />}
    </>
  );
}
