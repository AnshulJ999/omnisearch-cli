import ReactECharts from "echarts-for-react";
import { useMemo } from "react";
import { Card, CardHead, Empty, ErrorBox, Loading } from "@/components/ui";
import { ms } from "@/lib/format";
import { useDark } from "@/lib/theme";
import { useCalls } from "./data";

function color(status: number | null, dark: boolean) {
  if (status && status >= 200 && status < 300) return dark ? "#2dd4bf" : "#0f766e";
  if (status === 0 || status === null) return dark ? "#fbbf24" : "#b45309";
  return dark ? "#f87171" : "#b91c1c";
}

/** Every outside call of the run on one time axis: searches, then page fetches. */
export default function Timeline({ id }: { id: string }) {
  const calls = useCalls(id);
  const dark = useDark();
  const model = useMemo(() => {
    const list = (calls.data ?? []).map((c) => ({ ...c, start: c.ts * 1000 - c.elapsed_ms, end: c.ts * 1000 }));
    if (!list.length) return null;
    const t0 = Math.min(...list.map((c) => c.start));
    const lanes = [...new Set(list.map((c) => (c.label.startsWith("fetch") ? `fetch · ${c.provider}` : `${c.label} · ${c.provider}`)))];
    lanes.sort((a, b) => {
      const fa = a.startsWith("fetch"), fb = b.startsWith("fetch");
      if (fa !== fb) return fa ? 1 : -1;
      return a.localeCompare(b, undefined, { numeric: true });
    });
    const data = list.map((c) => {
      const lane = c.label.startsWith("fetch") ? `fetch · ${c.provider}` : `${c.label} · ${c.provider}`;
      return {
        value: [lanes.indexOf(lane), (c.start - t0) / 1000, (c.end - t0) / 1000, c.elapsed_ms],
        itemStyle: { color: color(c.status, dark) },
        name: `${c.label} · ${c.provider} · ${c.status ?? "no answer"} · ${ms(c.elapsed_ms)}\n${c.endpoint}`,
      };
    });
    const span = Math.max(...list.map((c) => c.end)) - t0;
    // Time when at least one call was open: the hours between rounds do not count.
    let active = 0;
    let open = -Infinity;
    for (const c of [...list].sort((a, b) => a.start - b.start)) {
      if (c.end <= open) continue;
      active += c.end - Math.max(c.start, open);
      open = c.end;
    }
    return { lanes, data, span, active, count: list.length };
  }, [calls.data, dark]);

  if (calls.isLoading) return <Loading what="the calls" />;
  if (calls.isError) return <ErrorBox error={calls.error} onRetry={() => calls.refetch()} />;
  if (!model) return <Empty title="No calls recorded for this run" />;

  const text = dark ? "#9a9a95" : "#6b6b66";
  const option = {
    grid: { left: 210, right: 24, top: 16, bottom: 40 },
    tooltip: { formatter: (p: { name: string }) => p.name.replace("\n", "<br/>") },
    xAxis: { type: "value", name: "seconds from the first call", nameLocation: "middle", nameGap: 26, axisLabel: { color: text }, nameTextStyle: { color: text }, splitLine: { lineStyle: { color: dark ? "#2f302e" : "#e3e3de" } } },
    yAxis: { type: "category", data: model.lanes, inverse: true, axisLabel: { color: text, fontSize: 11, width: 200, overflow: "truncate" } },
    dataZoom: [{ type: "inside", xAxisIndex: 0 }],
    series: [
      {
        type: "custom",
        renderItem: (_params: unknown, api: { value: (i: number) => number; coord: (v: number[]) => number[]; size: (v: number[]) => number[]; style: () => object }) => {
          const lane = api.value(0);
          const a = api.coord([api.value(1), lane]);
          const b = api.coord([api.value(2), lane]);
          const h = api.size([0, 1])[1] * 0.6;
          return { type: "rect", shape: { x: a[0], y: a[1] - h / 2, width: Math.max(2, b[0] - a[0]), height: h }, style: api.style() };
        },
        encode: { x: [1, 2], y: 0 },
        data: model.data,
      },
    ],
  };
  return (
    <Card>
      <CardHead
        title={model.span > model.active * 1.5 ? `${model.count} calls, ${ms(model.active)} of activity over ${ms(model.span)}` : `${model.count} calls over ${ms(model.span)}`}
        hint="Each bar is one call to an engine or fetcher, from when it was sent to its answer. Green answered, red refused, amber gave nothing. Activity counts only the time a call was open, so gaps between rounds are left out. Scroll to zoom; hover for details. Times are to the second."
      />
      <div className="p-2">
        <ReactECharts option={option} style={{ height: Math.max(260, model.lanes.length * 22 + 70) }} notMerge />
      </div>
    </Card>
  );
}
