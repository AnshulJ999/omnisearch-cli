# osearch web UI

The page behind `osearch ui`: React, TypeScript, Vite, Tailwind, TanStack Query and Table, React Flow (with elkjs) and ECharts. The API it talks to is `src/ui/` in the Rust crate.

```
npm ci
npm run build      # into dist/, which cargo packs into the osearch binary
npm run dev        # live reload on :5173, sending /api to a running `osearch ui` (OSEARCH_UI_PORT, default 7820)
```

- `src/pages/`: one file per area (Search, Runs, the run page and its tabs, How it works, Insights, Engines, Settings).
- `src/flows/flows.json`: the "How it works" maps. Each box names the settings that steer it; a Rust test fails when a setting is in no box or a box names a setting that does not exist.
- `src/lib/types.ts`: the API's answers.
- Colours are tokens in `src/index.css` (light and dark).
