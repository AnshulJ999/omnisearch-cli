import { useEffect } from "react";

// The review-notes widget, loaded from a notes server only when the config
// names one (`ui.notes`). When the server is not running, nothing appears.

interface NotesWindow extends Window {
  NotesStarterConfig?: { state: string; files: string[] };
  NotesWidget?: { init: (options: Record<string, unknown>) => unknown };
}

function screenLabel(): string {
  const parts = location.pathname.split("/").filter(Boolean);
  if (!parts.length) return "Search";
  return parts.map((p, i) => (i === 0 ? p.charAt(0).toUpperCase() + p.slice(1) : decodeURIComponent(p))).join(" ▸ ");
}

export default function Notes({ endpoint }: { endpoint: string }) {
  useEffect(() => {
    if (!endpoint) return;
    let server: string;
    try {
      server = new URL(endpoint).origin;
    } catch {
      return;
    }
    const w = window as NotesWindow;
    const init = () =>
      w.NotesWidget?.init({
        app: "OmniSearch UI",
        endpoint,
        position: "bottom-right",
        instance: `Local :${location.port}`,
        screen: screenLabel,
        context: () => ({ path: location.pathname + location.search }),
        isDark: () => document.documentElement.classList.contains("dark"),
        goTo: (note: { page?: string }) => {
          if (note.page) location.assign(note.page);
        },
      });
    window.addEventListener("notes-loaded", init, { once: true });
    w.NotesStarterConfig = { state: `${endpoint}/state`, files: [`${server}/devnotes/notes-widget.css`, `${server}/devnotes/notes-widget.js`] };
    const starter = document.createElement("script");
    starter.src = `${server}/devnotes/notes-starter.js`;
    starter.onerror = () => starter.remove();
    document.head.appendChild(starter);
    return () => window.removeEventListener("notes-loaded", init);
  }, [endpoint]);
  return null;
}
