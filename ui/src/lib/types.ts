// Shapes of the API's answers (src/ui/api.rs and the run files it reads).

export interface Info {
  version: string;
  config_file: string;
  data_folder: string;
  archive: string;
  synth_configured: boolean;
  notes: string;
}

export interface RunRow {
  id: string;
  topic: string;
  kind: string;
  ts: number;
  rounds: number;
  queries: string[];
  urls: number;
  pages: number;
  cost_usd: number;
  synthesis: boolean;
}

export interface Section {
  id: number;
  level: number;
  title: string;
  start: number;
  end: number;
  tokens: number;
}

export interface Attempt {
  tier: number;
  fetcher: string;
  outcome: string;
  ms: number;
}

export interface PageEntry {
  id: string;
  url: string;
  final_url: string;
  title: string;
  ok: boolean;
  tier: number;
  fetcher: string;
  failure?: string;
  message?: string;
  tokens: number;
  sections: Section[];
  round: number;
  from: string;
  attempts: Attempt[];
  cost_micros: number;
  late?: boolean;
  stamp?: string;
  note?: string;
  published?: string;
  same_as?: string;
  overlap?: number;
}

export interface Pick {
  id: string;
  url: string;
  from: string;
  why: "url" | "question" | "side" | "solo" | "rank" | "spare" | string;
}

export interface QueryIn {
  q: string;
  mode?: string;
  since?: string;
  kind?: string;
  engines?: string[];
  sites?: string[];
  region?: string;
}

export interface RequestFile {
  topic?: string;
  queries?: (string | QueryIn)[];
  urls?: string[];
  fetch?: number;
  width?: string;
  synth?: boolean;
  [k: string]: unknown;
}

export interface Round {
  round: number;
  request: RequestFile;
  summary: string | null;
  picks: Pick[] | null;
  use: Record<string, unknown> | null;
  fetching: boolean;
}

export interface RunDetail {
  id: string;
  rounds: Round[];
  index: { round: number; ts: number; topic: string; urls: number; pages: number; cost_usd: number; kind: string }[];
  summary: string | null;
  synthesis: string | null;
  syntheses?: { tag: string; label: string; text: string | null; figures: SynthesisFigures | null }[];
  synthesis_running: boolean;
  jina_running: boolean;
  pages: PageEntry[];
}

/** What a synthesis was sent and what it cost (synthesis.rN.json). */
export interface SynthesisFigures {
  question?: string;
  scope?: string;
  model?: string;
  earlier_rounds?: number;
  tokens_in?: number;
  tokens_out?: number;
  cost_usd?: number;
  cost_estimated?: boolean;
  secs?: number;
  quotes?: number;
  quotes_found?: number;
  pages?: { sent?: number; as_excerpts?: number; left_out?: number; still_fetching?: number };
  sent_as?: { id: string; round?: number; sent: string; tokens?: number }[];
}

export interface Call {
  n: number;
  label: string;
  provider: string;
  kind: string;
  endpoint: string;
  request?: string;
  status: number | null;
  elapsed_ms: number;
  bytes: number;
  raw: string;
  ts: number;
}

export interface Sighting {
  q: string;
  engine: string;
  rank: number;
  title: string;
  snippet: string;
  published?: string | null;
  web?: boolean;
}

export interface ResultRow {
  rank: number;
  url: string;
  title: string;
  score: number;
  engines: string[];
  queries: string[];
  found_by: Sighting[];
  page?: string;
}

export interface Setting {
  key: string;
  name: string;
  group: string;
  kind: "number" | "bool" | "text" | "list" | "choice";
  options: string[];
  about: string;
}

export interface ConfigAnswer {
  path: string;
  values: Record<string, Record<string, unknown>>;
  defaults: Record<string, Record<string, unknown>>;
  settings: Setting[];
}

export interface EngineNums {
  calls: number;
  failed: number;
  ms: number;
  links: number;
  alone: number;
  picked: number;
  read: number;
}

export interface FetcherNums {
  tries: number;
  ok: number;
  ms: number;
}

export interface RunNums {
  id: string;
  modes: string[];
  questions: number;
  links: number;
  pages_ok: number;
  pages_failed: number;
  tokens: number;
  seconds: number;
  engines: Record<string, EngineNums>;
  fetchers: Record<string, FetcherNums>;
  why: Record<string, number>;
  synthesis: boolean;
}

export interface Insights {
  runs: RunNums[];
  engines: Record<string, EngineNums>;
  fetchers: Record<string, FetcherNums>;
  why: Record<string, number>;
}

export interface EngineRow {
  name: string;
  configured: boolean;
  keyless: boolean;
  per_call_usd: number;
  modes: string[];
  optional: boolean;
  /** The balance this engine draws on (several share ScrapeCreators'). */
  balance: string;
}

export interface BalanceStatus {
  service: string;
  kind: "monthly" | "onetime" | "expiring";
  unit: string;
  remaining: number;
  total: number;
  live: boolean;
  as_of: string;
  until: string | null;
  expired: boolean;
  used_share: number | null;
  month_share: number | null;
  plentiful: boolean;
}

export interface Job {
  id: number;
  kind: string;
  title: string;
  started: number;
  ended: number | null;
  code: number | null;
  run: string | null;
  progress: { calls: number; pages_ok: number; pages_failed: number; synthesis_running: boolean } | null;
  line_count: number;
  lines?: string[];
}
