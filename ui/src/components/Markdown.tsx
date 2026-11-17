import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/format";

/** Markdown from a run. Citations like [p3#4] become buttons that open the page section. */
export default function Markdown({ text, onCite, className }: { text: string; onCite?: (page: string, section?: number) => void; className?: string }) {
  const linked = onCite ? text.replace(/\[(p\d+)(?:#(\d+))?\](?!\()/g, (_m, p, s) => `[${p}${s ? "#" + s : ""}](#cite-${p}${s ? "-" + s : ""})`) : text;
  return (
    <div className={cn("prose-md", className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => {
            const cite = href?.match(/^#cite-(p\d+)(?:-(\d+))?$/);
            if (cite && onCite) {
              return (
                <button type="button" className="rounded bg-accent-soft px-1 font-mono text-[0.8em] text-accent hover:underline" onClick={() => onCite(cite[1], cite[2] ? Number(cite[2]) : undefined)}>
                  {children}
                </button>
              );
            }
            return (
              <a href={href} target="_blank" rel="noreferrer noopener">
                {children}
              </a>
            );
          },
        }}
      >
        {linked}
      </ReactMarkdown>
    </div>
  );
}
