import { useState } from "react";
import Markdown from "@/components/Markdown";
import { cn, tokens, usd } from "@/lib/format";
import type { RunDetail, SynthesisFigures } from "@/lib/types";

/** A run's syntheses: a picker for each round's (and the whole run's), what
 * each was sent, then its text. Runs from before per-round synthesis have
 * only `synthesis.md`. */
export default function SynthesisView({ run, openPage, top }: { run: RunDetail; openPage: (p: string, s?: number) => void; top?: React.ReactNode }) {
  const list = run.syntheses ?? [];
  const newest = [...list].reverse().find((s) => s.tag !== "all") ?? list[list.length - 1];
  const [tag, setTag] = useState<string | null>(null);
  const shown = list.find((s) => s.tag === tag) ?? newest;
  if (!shown) {
    return (
      <>
        {top}
        <Markdown text={run.synthesis ?? ""} onCite={openPage} />
      </>
    );
  }
  return (
    <>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        {list.length > 1 ? (
          <div role="radiogroup" aria-label="Which synthesis" className="flex flex-wrap gap-1">
            {list.map((s) => (
              <button
                key={s.tag}
                role="radio"
                aria-checked={s.tag === shown.tag}
                onClick={() => setTag(s.tag)}
                className={cn(
                  "rounded-md border px-2.5 py-1 text-xs",
                  s.tag === shown.tag ? "border-accent bg-accent-soft text-text" : "border-line text-muted hover:text-text",
                )}
              >
                {s.label}
              </button>
            ))}
          </div>
        ) : (
          <span className="text-xs text-muted">{shown.label}</span>
        )}
        {top}
      </div>
      {shown.figures && <Sent figures={shown.figures} openPage={openPage} />}
      {shown.text ? <Markdown text={shown.text} onCite={openPage} /> : <p className="text-sm text-muted">This synthesis has no text.</p>}
    </>
  );
}

function Sent({ figures: f, openPage }: { figures: SynthesisFigures; openPage: (p: string) => void }) {
  const p = f.pages ?? {};
  const rows: [string, React.ReactNode][] = [
    ["Question", f.question],
    ["Pages", `${p.sent ?? 0} sent · ${p.as_excerpts ?? 0} as excerpts · ${p.left_out ?? 0} left out${p.still_fetching ? ` · ${p.still_fetching} still fetching` : ""}`],
    ["Earlier rounds", f.earlier_rounds ? `${f.earlier_rounds}, as their short answers` : "none"],
    ["Tokens", `${tokens(f.tokens_in ?? 0)} in · ${tokens(f.tokens_out ?? 0)} out`],
    ["Cost", `${usd(f.cost_usd ?? 0)}${f.cost_estimated ? " (estimated)" : ""}`],
    ["Time", f.secs != null ? `${f.secs} s` : "–"],
    ["Model", f.model],
    ["Quotes", f.quotes ? `${f.quotes_found ?? 0} of ${f.quotes} found in the pages` : "none"],
  ];
  return (
    <details className="mb-4 rounded-md border border-line bg-panel px-3 py-2 text-sm">
      <summary className="cursor-pointer select-none text-xs text-muted hover:text-text">What was sent</summary>
      <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="min-w-0 break-words tabular">{v}</dd>
          </div>
        ))}
      </dl>
      {(f.sent_as?.length ?? 0) > 0 && (
        <div className="mt-3 flex flex-wrap gap-1">
          {f.sent_as!.map((s) => (
            <button
              key={s.id}
              onClick={() => openPage(s.id)}
              className={cn("rounded border px-1.5 py-0.5 font-mono text-[11px] tabular", s.sent === "whole" ? "border-line" : "border-dashed border-line text-muted")}
              title={`${s.id}: ${s.sent}${s.tokens ? `, ${tokens(s.tokens)} tokens` : ""}`}
            >
              {s.id} {s.sent === "whole" ? "" : s.sent}
            </button>
          ))}
        </div>
      )}
    </details>
  );
}
