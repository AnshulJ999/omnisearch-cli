import { useMutation, useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, CardHead, Dialog, ErrorBox, Loading, PageHeader } from "@/components/ui";
import { get, post } from "@/lib/api";
import { cn, num, pct, usd } from "@/lib/format";
import type { BalanceStatus, EngineRow } from "@/lib/types";

interface Stats {
  window_days: number;
  searches: number;
  estimated_total_spend_usd: number;
  by_provider: Record<string, { calls: number; failures: number; estimated_spend_usd: number }>;
}

export default function EnginesPage() {
  const engines = useQuery({ queryKey: ["engines"], queryFn: () => get<{ engines: EngineRow[]; balances: BalanceStatus[] }>("/engines") });
  const stats = useQuery({ queryKey: ["stats", 30], queryFn: () => get<Stats>("/stats?days=30") });
  const [confirm, setConfirm] = useState(false);
  const live = useMutation({ mutationFn: () => post<unknown>("/usage"), onSuccess: () => engines.refetch() });

  if (engines.isLoading) return <Loading what="engines" />;
  if (engines.isError || !engines.data) return <ErrorBox error={engines.error} onRetry={() => engines.refetch()} />;
  const bal = new Map(engines.data.balances.map((b) => [b.service, b]));
  const spend = stats.data?.by_provider ?? {};
  const list = [...engines.data.engines].sort((a, b) => Number(b.configured) - Number(a.configured) || a.name.localeCompare(b.name));

  return (
    <>
      <PageHeader
        title="Engines and balances"
        sub="Which engines are set up, what each costs per call, and what is left of each allowance."
        right={
          <Button size="sm" onClick={() => setConfirm(true)} busy={live.isPending}>
            <RefreshCw className="h-3.5 w-3.5" /> Check live balances
          </Button>
        }
      />
      {live.isSuccess && <p className="mb-4 rounded-md bg-ok-soft px-3 py-2 text-sm text-ok">Live balances read just now.</p>}
      {live.isError && <p className="mb-4 rounded-md bg-bad-soft px-3 py-2 text-sm text-bad">{String(live.error)}</p>}
      <Card>
        <CardHead title={`${list.filter((e) => e.configured).length} of ${list.length} set up`} hint={`Spend is osearch's estimate at list prices over the last ${stats.data?.window_days ?? 30} days.`} />
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-line text-left text-xs text-muted">
                <th className="px-3 py-2 font-medium">Engine</th>
                <th className="px-3 py-2 font-medium">Status</th>
                <th className="px-3 py-2 text-right font-medium">Per call</th>
                <th className="px-3 py-2 font-medium">Allowance left</th>
                <th className="px-3 py-2 text-right font-medium">Spend, 30 days</th>
                <th className="px-3 py-2 font-medium">Modes</th>
              </tr>
            </thead>
            <tbody>
              {list.map((e) => {
                const b = bal.get(e.name);
                const s = spend[e.name];
                return (
                  <tr key={e.name} className={cn("border-b border-line last:border-0", !e.configured && "text-muted")}>
                    <td className="px-3 py-2 font-medium">{e.name}</td>
                    <td className="px-3 py-2">
                      {e.configured ? <Badge tone="ok">{e.keyless ? "no key needed" : "key set"}</Badge> : <Badge>{e.keyless ? "tool missing" : "no key"}</Badge>}
                      {e.optional && <Badge className="ml-1">only when named</Badge>}
                    </td>
                    <td className="px-3 py-2 text-right tabular">{e.per_call_usd ? usd(e.per_call_usd) : "free"}</td>
                    <td className="px-3 py-2">{b ? <BalanceBar b={b} /> : <span className="text-xs text-muted">{e.per_call_usd ? "no allowance set (used freely)" : "–"}</span>}</td>
                    <td className="px-3 py-2 text-right tabular">{s ? usd(s.estimated_spend_usd) : "–"}</td>
                    <td className="px-3 py-2 text-xs text-muted">{e.modes.join(", ")}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>
      <Dialog open={confirm} onOpenChange={setConfirm} title="Check live balances?" description="osearch asks each provider for its balance. Most checks are free; Brave's costs one search request.">
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={() => setConfirm(false)}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              setConfirm(false);
              live.mutate();
            }}
          >
            Check balances
          </Button>
        </div>
      </Dialog>
    </>
  );
}

function BalanceBar({ b }: { b: BalanceStatus }) {
  const left = b.total ? b.remaining / b.total : 1;
  const tone = b.expired ? "bg-bad" : left < 0.2 ? "bg-warn" : "bg-ok";
  return (
    <div className="min-w-[12rem]">
      <div className="flex justify-between gap-2 text-xs">
        <span className="tabular">
          {num(Math.round(b.remaining * 100) / 100)} of {num(b.total)} {b.unit}
        </span>
        <span className="text-muted">{b.expired ? "expired" : pct(b.remaining, b.total)}</span>
      </div>
      <div className="mt-1 h-1.5 rounded bg-panel-2">
        <div className={cn("h-1.5 rounded", tone)} style={{ width: `${Math.max(2, Math.min(100, left * 100))}%` }} />
      </div>
      <div className="mt-0.5 text-[11px] text-muted">
        {b.kind}
        {b.until ? ` · ${b.kind === "expiring" ? "ends" : "refills"} ${b.until}` : ""}
        {b.plentiful ? " · plentiful" : ""}
        {b.live ? " · live reading" : ` · as of ${b.as_of}`}
      </div>
    </div>
  );
}
