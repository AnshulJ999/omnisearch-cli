import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil } from "lucide-react";
import { useMemo, useState } from "react";
import { Link } from "react-router";
import { toast } from "sonner";
import { Button, Card, CardHead, Dialog, ErrorBox, Input, Label, Loading, PageHeader, Select, Switch } from "@/components/ui";
import { get, post } from "@/lib/api";
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

export default function SettingsPage() {
  const cfg = useConfig();
  const [filter, setFilter] = useState("");
  const [editing, setEditing] = useState<Setting | null>(null);
  const groups = useMemo(() => {
    const m = new Map<string, Setting[]>();
    for (const s of cfg.data?.settings ?? []) {
      const f = filter.toLowerCase();
      if (f && !`${s.key} ${s.about}`.toLowerCase().includes(f)) continue;
      m.set(s.group, [...(m.get(s.group) ?? []), s]);
    }
    return [...m.entries()];
  }, [cfg.data, filter]);

  if (cfg.isLoading) return <Loading what="settings" />;
  if (cfg.isError || !cfg.data) return <ErrorBox error={cfg.error} onRetry={() => cfg.refetch()} />;
  return (
    <>
      <PageHeader
        title="Settings"
        sub={
          <>
            The values in <span className="font-mono text-xs">{cfg.data.path}</span>, with every default filled in. A change applies to the next search. <Link className="text-accent underline" to="/how">How it works</Link> shows where each one acts.
          </>
        }
      />
      <Input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Find a setting" aria-label="Find a setting" className="mb-5 max-w-sm" />
      {groups.length === 0 && <p className="text-sm text-muted">No setting matches "{filter}".</p>}
      <div className="space-y-6">
        {groups.map(([group, list]) => (
          <Card key={group}>
            <CardHead title={group} />
            <ul className="divide-y divide-line">
              {list.map((s) => (
                <li key={s.key} id={`setting-${s.key}`} className="grid gap-2 px-4 py-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,16rem)_auto] sm:items-center">
                  <div className="min-w-0">
                    <div className="font-mono text-xs">{s.key}</div>
                    <div className="mt-0.5 text-sm text-muted">{s.about}</div>
                  </div>
                  <div className="min-w-0 break-words text-sm tabular">{show(valueOf(cfg.data.values, s.key))}</div>
                  <Button size="sm" variant="ghost" onClick={() => setEditing(s)} aria-label={`Change ${s.key}`}>
                    <Pencil className="h-3.5 w-3.5" /> Change
                  </Button>
                </li>
              ))}
            </ul>
          </Card>
        ))}
      </div>
      {editing && <EditSetting setting={editing} current={valueOf(cfg.data.values, editing.key)} onClose={() => setEditing(null)} />}
    </>
  );
}

export function EditSetting({ setting, current, onClose }: { setting: Setting; current: unknown; onClose: () => void }) {
  const qc = useQueryClient();
  const start = Array.isArray(current) ? current.join(", ") : typeof current === "boolean" ? String(current) : String(current ?? "");
  const [value, setValue] = useState(start);
  const save = useMutation({
    mutationFn: () => post<ConfigAnswer>("/config", { key: setting.key, value }),
    onSuccess: (data) => {
      qc.setQueryData(["config"], data);
      toast.success(`${setting.key} is now ${show(valueOf(data.values, setting.key))}`);
      onClose();
    },
  });
  const secret = setting.key === "synth.key";
  return (
    <Dialog open onOpenChange={(v) => !v && onClose()} title={`Change ${setting.key}`} description={setting.about}>
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
              <Switch checked={value === "true"} onChange={(v) => setValue(String(v))} label={setting.key} />
              <span className="text-sm">{value === "true" ? "on" : "off"}</span>
            </div>
          ) : setting.kind === "choice" ? (
            <Select id="setting-value" value={value} onChange={(e) => setValue(e.target.value)} className="w-full">
              {setting.options.map((o) => (
                <option key={o}>{o}</option>
              ))}
            </Select>
          ) : (
            <Input id="setting-value" autoFocus type={setting.kind === "number" ? "number" : secret ? "password" : "text"} step="any" value={secret && value === start ? "" : value} placeholder={secret ? "Paste the new key" : undefined} onChange={(e) => setValue(e.target.value)} />
          )}
          {setting.kind === "list" && <p className="text-xs text-muted">Comma-separated; leave empty for none.</p>}
        </div>
        <p className="rounded-md bg-panel-2 px-3 py-2 text-sm">
          <span className="text-muted">Now:</span> {secret ? "(hidden)" : show(current)} <span className="text-muted">→ new:</span> {secret ? "(hidden)" : value || "(empty)"}
        </p>
        {save.isError && <p className="text-sm text-bad">{save.error instanceof Error ? save.error.message : String(save.error)}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" busy={save.isPending} disabled={value === start && !secret}>
            Save to the config file
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
