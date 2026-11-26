import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router";
import { toast } from "sonner";
import { Badge, Button, Dialog, ErrorBox, Hint, Input, Label, Loading, PageHeader, Select, Switch } from "@/components/ui";
import { get, post } from "@/lib/api";
import { cn } from "@/lib/format";
import type { ConfigAnswer, Setting } from "@/lib/types";

export function valueOf(values: ConfigAnswer["values"], key: string): unknown {
  const [section, name] = key.split(".");
  return values?.[section]?.[name];
}

export function show(v: unknown): string {
  if (Array.isArray(v)) return v.length ? v.join(", ") : "(none)";
  if (typeof v === "boolean") return v ? "on" : "off";
  if (v === "" || v === null || v === undefined) return "(empty)";
  return String(v);
}

export const useConfig = () => useQuery({ queryKey: ["config"], queryFn: () => get<ConfigAnswer>("/config") });

const SECRET = "synth.key";
const groupId = (g: string) => `group-${g.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/** Save one setting; the page shows the server's answer. */
function useSave() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { setting: Setting; value: string }) => post<ConfigAnswer>("/config", { key: v.setting.key, value: v.value }),
    onSuccess: (data, v) => {
      qc.setQueryData(["config"], data);
      toast.success(v.setting.key === SECRET ? `${v.setting.name} saved` : `${v.setting.name}: ${show(valueOf(data.values, v.setting.key))}`);
    },
    onError: (e, v) => toast.error(`${v.setting.name} not saved: ${e instanceof Error ? e.message : String(e)}`),
  });
}

export default function SettingsPage() {
  const cfg = useConfig();
  const save = useSave();
  const [filter, setFilter] = useState("");
  const [changedOnly, setChangedOnly] = useState(false);
  const [editing, setEditing] = useState<Setting | null>(null);
  const [active, setActive] = useState<string | null>(null);

  const changed = (s: Setting) => s.key !== SECRET && cfg.data != null && !same(valueOf(cfg.data.values, s.key), valueOf(cfg.data.defaults, s.key));
  const groups = useMemo(() => {
    const m = new Map<string, Setting[]>();
    const f = filter.trim().toLowerCase();
    for (const s of cfg.data?.settings ?? []) {
      if (f && !`${s.name} ${s.key} ${s.about}`.toLowerCase().includes(f)) continue;
      if (changedOnly && !changed(s)) continue;
      m.set(s.group, [...(m.get(s.group) ?? []), s]);
    }
    return [...m.entries()];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cfg.data, filter, changedOnly]);

  // The side index follows the group at the top of the page.
  useEffect(() => {
    const seen = new Map<string, boolean>();
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) seen.set(e.target.id, e.isIntersecting);
        const first = groups.find(([g]) => seen.get(groupId(g)));
        if (first) setActive(first[0]);
      },
      { rootMargin: "-10% 0px -60% 0px" },
    );
    for (const [g] of groups) {
      const el = document.getElementById(groupId(g));
      if (el) io.observe(el);
    }
    return () => io.disconnect();
  }, [groups]);

  if (cfg.isLoading) return <Loading what="settings" />;
  if (cfg.isError || !cfg.data) return <ErrorBox error={cfg.error} onRetry={() => cfg.refetch()} />;
  const data = cfg.data;
  const changedCount = data.settings.filter(changed).length;
  const jump = (g: string) => document.getElementById(groupId(g))?.scrollIntoView({ behavior: "smooth", block: "start" });

  return (
    <>
      <PageHeader
        title="Settings"
        sub={
          <>
            Saved in <span className="font-mono text-xs">{data.path}</span>. A change applies to the next search. <Link className="text-accent underline" to="/how">How it works</Link> shows where each one acts.
          </>
        }
      />
      <div className="mb-5 flex flex-wrap items-center gap-x-5 gap-y-2">
        <Input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Find a setting" aria-label="Find a setting" className="max-w-sm" />
        <label className="flex items-center gap-2 text-sm">
          <Switch checked={changedOnly} onChange={setChangedOnly} label="Show changed settings only" />
          Changed only <span className="text-muted tabular">({changedCount})</span>
        </label>
      </div>
      <div className="lg:grid lg:grid-cols-[12rem_minmax(0,1fr)] lg:gap-8">
        <nav aria-label="Setting groups" className="mb-4 lg:mb-0">
          <ul className="flex flex-wrap gap-1 lg:sticky lg:top-4 lg:flex-col lg:gap-0.5">
            {groups.map(([g, list]) => (
              <li key={g}>
                <button
                  onClick={() => jump(g)}
                  aria-current={active === g ? "true" : undefined}
                  className={cn(
                    "flex w-full items-center justify-between gap-2 rounded-md px-2.5 py-1 text-left text-sm",
                    active === g ? "bg-accent-soft text-text" : "text-muted hover:text-text",
                  )}
                >
                  <span>{g}</span>
                  <span className="text-xs tabular text-muted">{list.length}</span>
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <div className="min-w-0 space-y-8">
          {groups.length === 0 && (
            <p className="text-sm text-muted">
              No setting matches{filter ? ` "${filter}"` : ""}{changedOnly ? " among the changed ones" : ""}.{" "}
              <button className="text-accent underline" onClick={() => { setFilter(""); setChangedOnly(false); }}>
                Show all
              </button>
            </p>
          )}
          {groups.map(([g, list]) => (
            <section key={g} id={groupId(g)} aria-labelledby={`${groupId(g)}-title`} className="scroll-mt-4">
              <h2 id={`${groupId(g)}-title`} className="mb-1 text-base font-semibold">{g}</h2>
              <ul className="divide-y divide-line border-y border-line">
                {list.map((s) => (
                  <Row key={s.key} s={s} data={data} changed={changed(s)} busy={save.isPending && save.variables?.setting.key === s.key} onSave={(value) => save.mutate({ setting: s, value })} onEdit={() => setEditing(s)} />
                ))}
              </ul>
            </section>
          ))}
        </div>
      </div>
      {editing && <EditSetting setting={editing} current={valueOf(data.values, editing.key)} onClose={() => setEditing(null)} />}
    </>
  );
}

function Row({ s, data, changed, busy, onSave, onEdit }: { s: Setting; data: ConfigAnswer; changed: boolean; busy: boolean; onSave: (v: string) => void; onEdit: () => void }) {
  const value = valueOf(data.values, s.key);
  const secret = s.key === SECRET;
  let control: React.ReactNode;
  if (s.kind === "bool") {
    control = (
      <span className="flex items-center gap-2">
        <span className="text-sm text-muted">{value ? "on" : "off"}</span>
        <Switch checked={value === true} onChange={(v) => !busy && onSave(String(v))} label={s.name} />
      </span>
    );
  } else if (s.kind === "choice") {
    control = (
      <Select value={String(value ?? "")} disabled={busy} onChange={(e) => onSave(e.target.value)} aria-label={s.name} className="min-w-28">
        {s.options.map((o) => (
          <option key={o}>{o}</option>
        ))}
      </Select>
    );
  } else {
    control = (
      <span className="flex min-w-0 items-center gap-2">
        <span className="min-w-0 break-words text-sm tabular">{secret ? (value ? "set" : "not set") : show(value)}</span>
        <Button size="sm" variant="ghost" busy={busy} onClick={onEdit} aria-label={`Change ${s.name}`}>
          <Pencil className="h-3.5 w-3.5" /> {secret ? "Replace…" : "Change…"}
        </Button>
      </span>
    );
  }
  return (
    <li id={`setting-${s.key}`} className="grid gap-2 py-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:gap-6">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2 text-sm font-medium">
          {s.name}
          {changed && (
            <Hint text={`Default: ${show(valueOf(data.defaults, s.key))}`}>
              <span>
                <Badge tone="info">changed</Badge>
              </span>
            </Hint>
          )}
        </div>
        <div className="mt-0.5 text-sm text-muted">{s.about}</div>
        <div className="mt-0.5 font-mono text-[11px] text-muted">{s.key}</div>
      </div>
      <div className="flex justify-start sm:justify-end">{control}</div>
    </li>
  );
}

export function EditSetting({ setting, current, onClose }: { setting: Setting; current: unknown; onClose: () => void }) {
  const qc = useQueryClient();
  const secret = setting.key === SECRET;
  // A secret arrives masked; its box starts empty so the mask is never saved back.
  const start = secret ? "" : Array.isArray(current) ? current.join(", ") : typeof current === "boolean" ? String(current) : String(current ?? "");
  const [value, setValue] = useState(start);
  const save = useMutation({
    mutationFn: () => post<ConfigAnswer>("/config", { key: setting.key, value }),
    onSuccess: (data) => {
      qc.setQueryData(["config"], data);
      toast.success(secret ? `${setting.name} saved` : `${setting.name}: ${show(valueOf(data.values, setting.key))}`);
      onClose();
    },
  });
  return (
    <Dialog open onOpenChange={(v) => !v && onClose()} title={`Change ${setting.name}`} description={setting.about}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate();
        }}
        className="space-y-4"
      >
        <div className="space-y-1">
          <Label htmlFor="setting-value">New value</Label>
          {setting.kind === "bool" ? (
            <div className="flex items-center gap-2">
              <Switch checked={value === "true"} onChange={(v) => setValue(String(v))} label={setting.name} />
              <span className="text-sm">{value === "true" ? "on" : "off"}</span>
            </div>
          ) : setting.kind === "choice" ? (
            <Select id="setting-value" value={value} onChange={(e) => setValue(e.target.value)} className="w-full">
              {setting.options.map((o) => (
                <option key={o}>{o}</option>
              ))}
            </Select>
          ) : (
            <Input id="setting-value" autoFocus type={setting.kind === "number" ? "number" : secret ? "password" : "text"} step="any" min={setting.kind === "number" ? 0 : undefined} value={value} placeholder={secret ? "Paste a new key; the saved one stays unless you do" : undefined} onChange={(e) => setValue(e.target.value)} />
          )}
          {setting.kind === "list" && <p className="text-xs text-muted">Comma-separated; leave empty for none.</p>}
          <p className="font-mono text-[11px] text-muted">{setting.key}</p>
        </div>
        <p className="rounded-md bg-panel-2 px-3 py-2 text-sm">
          <span className="text-muted">Now:</span> {secret ? "(hidden)" : show(current)} <span className="text-muted">→ new:</span> {secret ? "(hidden)" : value || "(empty)"}
        </p>
        {save.isError && <p className="text-sm text-bad">{save.error instanceof Error ? save.error.message : String(save.error)}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" busy={save.isPending} disabled={value === start || (secret && !value.trim())}>
            Save to the config file
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
