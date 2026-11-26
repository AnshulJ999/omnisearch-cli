// The shared building blocks. Every colour comes from the theme tokens in
// index.css, so light and dark stay in step.
import { Dialog as D, Switch as S, Tabs as T, Tooltip as Tip } from "radix-ui";
import { cva, type VariantProps } from "class-variance-authority";
import { AlertTriangle, Inbox, Loader2, X } from "lucide-react";
import * as React from "react";
import { cn } from "@/lib/format";

const button = cva(
  "inline-flex items-center justify-center gap-1.5 rounded-md text-sm font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed select-none whitespace-nowrap",
  {
    variants: {
      variant: {
        primary: "bg-accent text-accent-text hover:opacity-90",
        secondary: "bg-panel-2 text-text hover:bg-line border border-line",
        ghost: "text-text hover:bg-panel-2",
        danger: "bg-bad text-white hover:opacity-90",
      },
      size: { sm: "h-7 px-2.5 text-xs", md: "h-9 px-3.5", icon: "h-8 w-8" },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof button> {
  busy?: boolean;
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, busy, children, disabled, ...props }, ref) => (
    <button ref={ref} className={cn(button({ variant, size }), className)} disabled={disabled || busy} aria-busy={busy || undefined} {...props}>
      {busy && <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />}
      {children}
    </button>
  ),
);
Button.displayName = "Button";

const badge = cva("inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium leading-none whitespace-nowrap", {
  variants: {
    tone: {
      neutral: "bg-panel-2 text-muted border border-line",
      accent: "bg-accent-soft text-accent",
      ok: "bg-ok-soft text-ok",
      warn: "bg-warn-soft text-warn",
      bad: "bg-bad-soft text-bad",
      info: "bg-info-soft text-info",
    },
  },
  defaultVariants: { tone: "neutral" },
});

export function Badge({ tone, className, ...p }: React.HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badge>) {
  return <span className={cn(badge({ tone }), className)} {...p} />;
}

export function Card({ className, ...p }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("rounded-lg border border-line bg-panel", className)} {...p} />;
}

export function CardHead({ title, hint, right }: { title: React.ReactNode; hint?: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-3 border-b border-line px-4 py-3">
      <div className="min-w-0">
        <h2 className="text-sm font-semibold">{title}</h2>
        {hint && <p className="mt-0.5 text-xs text-muted">{hint}</p>}
      </div>
      {right}
    </div>
  );
}

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(({ className, ...p }, ref) => (
  <input
    ref={ref}
    className={cn("h-9 w-full rounded-md border border-line bg-panel px-3 text-sm placeholder:text-muted focus:border-accent", className)}
    {...p}
  />
));
Input.displayName = "Input";

export const Textarea = React.forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(({ className, ...p }, ref) => (
  <textarea
    ref={ref}
    className={cn("w-full rounded-md border border-line bg-panel px-3 py-2 text-sm placeholder:text-muted focus:border-accent", className)}
    {...p}
  />
));
Textarea.displayName = "Textarea";

export function Select({ className, ...p }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return <select className={cn("h-9 rounded-md border border-line bg-panel px-2 text-sm focus:border-accent", className)} {...p} />;
}

export function Label({ className, ...p }: React.LabelHTMLAttributes<HTMLLabelElement>) {
  return <label className={cn("text-xs font-medium text-muted", className)} {...p} />;
}

export function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <S.Root
      checked={checked}
      onCheckedChange={onChange}
      aria-label={label}
      className="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-line bg-panel-2 transition-colors data-[state=checked]:bg-accent"
    >
      <S.Thumb className="block h-4 w-4 translate-x-0.5 rounded-full bg-white shadow transition-transform data-[state=checked]:translate-x-[18px]" />
    </S.Root>
  );
}

export function Tabs({ value, onChange, items, children }: { value: string; onChange: (v: string) => void; items: { value: string; label: React.ReactNode }[]; children: React.ReactNode }) {
  return (
    <T.Root value={value} onValueChange={onChange}>
      {/* overflow-y-hidden: the triggers' -mb-px would otherwise add a 1px vertical scrollbar. */}
      <T.List className="flex gap-1 overflow-x-auto overflow-y-hidden border-b border-line" aria-label="Sections">
        {items.map((i) => (
          <T.Trigger
            key={i.value}
            value={i.value}
            className="-mb-px border-b-2 border-transparent px-3 py-2 text-sm text-muted whitespace-nowrap hover:text-text data-[state=active]:border-accent data-[state=active]:text-text"
          >
            {i.label}
          </T.Trigger>
        ))}
      </T.List>
      {children}
    </T.Root>
  );
}
export const TabPanel = ({ value, children }: { value: string; children: React.ReactNode }) => (
  <T.Content value={value} className="pt-4 focus-visible:outline-none">
    {children}
  </T.Content>
);

export function Dialog({ open, onOpenChange, title, description, children, wide }: { open: boolean; onOpenChange: (v: boolean) => void; title: string; description?: string; children: React.ReactNode; wide?: boolean }) {
  return (
    <D.Root open={open} onOpenChange={onOpenChange}>
      <D.Portal>
        <D.Overlay className="fixed inset-0 z-40 bg-black/40" />
        <D.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 max-h-[85vh] w-[calc(100vw-2rem)] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-lg border border-line bg-panel p-5 shadow-xl",
            wide ? "max-w-4xl" : "max-w-lg",
          )}
        >
          <div className="mb-3 flex items-start justify-between gap-3">
            <div>
              <D.Title className="text-base font-semibold">{title}</D.Title>
              {description ? <D.Description className="mt-1 text-sm text-muted">{description}</D.Description> : <D.Description className="sr-only">{title}</D.Description>}
            </div>
            <D.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close">
                <X className="h-4 w-4" />
              </Button>
            </D.Close>
          </div>
          {children}
        </D.Content>
      </D.Portal>
    </D.Root>
  );
}

export function Hint({ text, children }: { text: React.ReactNode; children: React.ReactNode }) {
  return (
    <Tip.Root delayDuration={300}>
      <Tip.Trigger asChild>{children}</Tip.Trigger>
      <Tip.Portal>
        <Tip.Content sideOffset={4} className="z-50 max-w-xs rounded-md border border-line bg-panel px-2.5 py-1.5 text-xs shadow-lg">
          {text}
        </Tip.Content>
      </Tip.Portal>
    </Tip.Root>
  );
}
export const HintProvider = Tip.Provider;

export function Loading({ what }: { what: string }) {
  return (
    <div className="flex items-center gap-2 p-6 text-sm text-muted" role="status">
      <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> Loading {what}…
    </div>
  );
}

export function Empty({ title, children, icon }: { title: string; children?: React.ReactNode; icon?: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
      <div className="text-muted">{icon ?? <Inbox className="h-6 w-6" aria-hidden />}</div>
      <p className="text-sm font-medium">{title}</p>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
    </div>
  );
}

export function ErrorBox({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const msg = error instanceof Error ? error.message : String(error);
  return (
    <div className="m-4 flex items-start gap-3 rounded-lg border border-bad/40 bg-bad-soft p-4 text-sm" role="alert">
      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-bad" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="font-medium text-bad">Couldn't load this</p>
        <p className="mt-1 break-words text-text">{msg}</p>
        {msg.includes("Failed to fetch") && <p className="mt-1 text-muted">Is `osearch ui` still running?</p>}
      </div>
      {onRetry && (
        <Button size="sm" onClick={onRetry}>
          Try again
        </Button>
      )}
    </div>
  );
}

export function Stat({ label, value, sub }: { label: string; value: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <div className="min-w-0 rounded-lg border border-line bg-panel px-4 py-3">
      <div className="text-xs text-muted">{label}</div>
      <div className="mt-1 truncate text-xl font-semibold tabular">{value}</div>
      {sub && <div className="mt-0.5 truncate text-xs text-muted">{sub}</div>}
    </div>
  );
}

export function PageHeader({ title, sub, right }: { title: React.ReactNode; sub?: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div className="mb-5 flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h1 className="text-xl font-semibold tracking-tight break-words">{title}</h1>
        {sub && <div className="mt-1 text-sm text-muted">{sub}</div>}
      </div>
      {right && <div className="flex flex-wrap items-center gap-2">{right}</div>}
    </div>
  );
}
