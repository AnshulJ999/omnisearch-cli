import { useQuery } from "@tanstack/react-query";
import { BarChart3, History, Menu, Moon, Search, Server, SlidersHorizontal, Sun, Workflow, X } from "lucide-react";
import { useEffect, useState } from "react";
import { NavLink, Outlet, useLocation } from "react-router";
import { get } from "@/lib/api";
import { cn } from "@/lib/format";
import type { Info } from "@/lib/types";
import Notes from "./Notes";
import { Button } from "./ui";

const NAV = [
  { to: "/", label: "Search", icon: Search, end: true },
  { to: "/runs", label: "Runs", icon: History },
  { to: "/how", label: "How it works", icon: Workflow },
  { to: "/insights", label: "Insights", icon: BarChart3 },
  { to: "/engines", label: "Engines", icon: Server },
  { to: "/settings", label: "Settings", icon: SlidersHorizontal },
];

function useTheme() {
  const [dark, setDark] = useState(() => document.documentElement.classList.contains("dark"));
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    try {
      localStorage.setItem("osearch-theme", dark ? "dark" : "light");
    } catch {
      /* private window: the choice lasts this visit only */
    }
  }, [dark]);
  return [dark, setDark] as const;
}

export default function Layout() {
  const info = useQuery({ queryKey: ["info"], queryFn: () => get<Info>("/info") });
  const [dark, setDark] = useTheme();
  const [open, setOpen] = useState(false);
  const loc = useLocation();
  useEffect(() => setOpen(false), [loc.pathname]);

  const nav = (
    <nav className="flex flex-col gap-0.5 p-2" aria-label="Main">
      {NAV.map(({ to, label, icon: Icon, end }) => (
        <NavLink
          key={to}
          to={to}
          end={end}
          className={({ isActive }) =>
            cn("flex items-center gap-2.5 rounded-md px-3 py-2 text-sm text-muted hover:bg-panel-2 hover:text-text", isActive && "bg-accent-soft font-medium text-accent hover:bg-accent-soft hover:text-accent")
          }
        >
          <Icon className="h-4 w-4" aria-hidden />
          {label}
        </NavLink>
      ))}
    </nav>
  );

  return (
    <div className="flex h-full">
      <aside className="hidden w-56 shrink-0 flex-col border-r border-line bg-panel md:flex">
        <Brand />
        {nav}
        <Footer version={info.data?.version} dark={dark} setDark={setDark} />
      </aside>
      {open && (
        <div className="fixed inset-0 z-30 md:hidden">
          <div className="absolute inset-0 bg-black/40" onClick={() => setOpen(false)} />
          <aside className="absolute inset-y-0 left-0 flex w-64 flex-col border-r border-line bg-panel">
            <div className="flex items-center justify-between pr-2">
              <Brand />
              <Button variant="ghost" size="icon" aria-label="Close menu" onClick={() => setOpen(false)}>
                <X className="h-4 w-4" />
              </Button>
            </div>
            {nav}
            <Footer version={info.data?.version} dark={dark} setDark={setDark} />
          </aside>
        </div>
      )}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center gap-2 border-b border-line bg-panel px-3 py-2 md:hidden">
          <Button variant="ghost" size="icon" aria-label="Open menu" onClick={() => setOpen(true)}>
            <Menu className="h-4 w-4" />
          </Button>
          <span className="font-semibold">OmniSearch</span>
        </header>
        <main className="min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto max-w-[1400px] px-4 py-6 md:px-8">
            <Outlet />
          </div>
        </main>
      </div>
      {info.data?.notes && <Notes endpoint={info.data.notes} />}
    </div>
  );
}

function Brand() {
  return (
    <div className="flex items-center gap-2 px-4 py-4">
      <img src="/icon.svg" alt="" className="h-6 w-6" />
      <div>
        <div className="text-sm font-semibold leading-tight">OmniSearch</div>
        <div className="text-[11px] text-muted leading-tight">osearch</div>
      </div>
    </div>
  );
}

function Footer({ version, dark, setDark }: { version?: string; dark: boolean; setDark: (v: boolean) => void }) {
  return (
    <div className="mt-auto flex items-center justify-between border-t border-line px-3 py-2 text-xs text-muted">
      <span>{version ? `v${version}` : ""}</span>
      <Button variant="ghost" size="icon" aria-label={dark ? "Switch to light theme" : "Switch to dark theme"} onClick={() => setDark(!dark)}>
        {dark ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
      </Button>
    </div>
  );
}
