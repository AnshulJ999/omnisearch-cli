# OmniSearch CLI (`osearch`)

One command for web research. It asks every search engine you have a key for at once, reads the best pages through free-first fetchers, and keeps everything as a run you can read, search and come back to. Built for AI agents (Claude Code, Codex and the like) and usable by people, with a local web UI.

```
osearch "kopia vs restic for a homelab"
osearch "community: is the framework 16 worth it" "framework 16 hinge problems"
osearch ui
```

## Why it exists

An agent doing research usually juggles several search tools, guesses which one fits, reads a handful of pages, and loses the trail. osearch makes that one step:

- **Wide by default.** Every configured web engine answers each question, and results are merged by how many engines found them. Links that only one engine found get their own slots, since those are often the new ones.
- **Where people actually talk.** Community questions also search Reddit (several routes, with or without a login), forums (Google's forum filter, Discourse forums directly, GitHub Discussions, Lemmy), Telegram channels and Hacker News. Social mode adds YouTube captions, X, Bluesky and more.
- **Pages, not snippets.** The best pages are read through tiers: a site's own API first, a fast fetcher, a browser only when needed, hosted readers last, and the Wayback Machine for pages that are gone or walled. Each page is saved with an outline, so an agent reads one section instead of the whole page.
- **Everything kept.** Every engine's raw answer, every page, why each page was picked and how it was fetched. Runs can be continued, searched, re-ranked and compared later.
- **An answer you can check.** An optional AI synthesis writes a cited answer from the pages, one per round of follow-up searches, and every quote in it is checked word for word against the page it cites. What each synthesis was sent and what it cost is saved beside it.
- **Costs under control.** Each engine's allowance (monthly, one-time or expiring credits) steers which engines run, and spend is logged per call.

## Status

A working baseline from one person's daily use, published for others to fork and adapt. An agent can take this repo and reshape it for its user; [AGENTS.md](AGENTS.md) is the map for that. It is developed and used daily on Windows; the Linux and macOS builds come from the release workflow and have had less real use.

## Install

Download a release for your platform, or build it (below). Then:

```
osearch update     # fetches the outside tools osearch drives (donsetch, moli)
osearch doctor     # what is set up: keys, tools, browser, archive
```

**Keys.** osearch works with no keys at all (donsetch's keyless search, Reddit, Hacker News, GitHub through `gh`, the Wayback Machine). Each key you add brings in another engine. Set them as environment variables, or in a `KEY=value` file named by `paths.envfile`:

| Engine | Variable | Notes |
|---|---|---|
| Brave | `BRAVE_API_KEY` | free plan: one request a second |
| Exa | `EXA_API_KEY` | meaning-based search; also "pages like this" |
| Tavily | `TAVILY_API_KEY` | |
| You.com | `YDC_API_KEY` | |
| Linkup | `LINKUP_API_KEY` | |
| Parallel | `PARALLEL_API_KEY` | |
| Jina | `JINA_API_KEY` | search, reader, reranker, DeepSearch |
| Serper | `SERPER_API_KEY` | Google results |
| SerpApi | `SERPAPI_API_KEY` | Google forums and Scholar |
| Firecrawl | `FIRECRAWL_API_KEY` | paid reader, last fetch tier |
| Bluesky | `BSKY_HANDLE`, `BSKY_APP_PASSWORD` | social mode |

`osearch providers` lists every engine and fetcher and whether it is set up.

**AI synthesis** is off until you point it at a model. Any OpenAI-style endpoint works (OpenAI, OpenRouter, Ollama, LM Studio), as does an Anthropic-style one:

```
osearch config set synth.api openai
osearch config set synth.url https://openrouter.ai/api/v1/chat/completions
osearch config set synth.model <a cheap model with a large context>
osearch config set synth.key <key>
```

## Using it

| Command | What it does |
|---|---|
| `osearch "question"` | Search, read the top pages, print a summary. Each quoted argument is its own question; `community: ...` sets one's mode |
| `osearch run request.json` | Several angles with options per question (`osearch help run` prints the format with the current defaults) |
| `osearch fetch <url>...` | Read known pages through the same tiers |
| `osearch crawl <docs-url>` | A whole docs site into one run |
| `osearch read p3` / `p3#4` | A page's outline, or one section of it |
| `osearch grep <words>` | Matching lines across a run's pages |
| `osearch runs --find <topic>` | Was this researched before? |
| `osearch wait <run>` | Wait for a run's background pages or synthesis |
| `osearch ui` | The web UI |
| `osearch usage` / `osearch stats` | Balances left, and spend from the logs |
| `osearch login <site>` | Log in once in a browser window; the login is used when fetching that site |

Modes (`-m`): general, community, forums, social, news, code, academic, scholar, people, shopping, deep. Date limits: `-f day|week|month|year`.

## The web UI

`osearch ui` opens a local page (this machine only):

- **Search**: run searches and watch them progress.
- **Runs**: every past run, with its searches, results, the pages read (why each was picked, how it was fetched), a timeline of every call, the summary and the synthesis.
- **How it works**: each flow of osearch drawn as a map, with the settings that steer each step at their current values.
- **Insights**: what each engine finds that others miss, how each fetcher does, across all runs.
- **Engines** and **Settings**: balances, spend, and every setting in one place.

## Using it from an AI agent

[skill/SKILL.md](skill/SKILL.md) is a drop-in skill that teaches an agent to use osearch well: read the summary first, then single sections; continue a run instead of starting over; leave the tuned defaults alone. Copy it into your agent's skills folder and adjust.

## Configuration

`osearch config show` prints every setting with its value; `osearch config set <section.key> <value>` changes one; the web UI's Settings page does both. [config.example.toml](config.example.toml) shows the settings most people change. By default config and data live in your platform's usual folders; set `OMNISEARCH_HOME` (or put a file named `omnisearch-home` holding a folder path next to the executable) to keep them in one folder. Engine allowances go in `[balance.<engine>]`; an engine with none is used freely.

[docs/REFERENCE.md](docs/REFERENCE.md) covers everything in detail: every engine, how Reddit and forums are searched, the fetch tiers, page picking, synthesis, and research mode.

## Accounts and risk

Some features act as you, through your own logins. They are off or opt-in for a reason:

- **Logged-in Reddit search** uses your saved Reddit login (`osearch login reddit.com`, or `--as <name>` for a second account). Heavy automated use can get an account limited; a spare account for runs (`research.redditaccount`) is the safer setup. Logged-in requests are paced (`research.redditpace`, a minute per account); `research.redditsidebyside` (off by default) runs the spare and main logins at once, which is faster but sends more requests from one address. Without any login, Reddit refuses most requests from some addresses, so community search is weaker.
- **X search** (`research.xbird`, off by default) runs as your real X account.
- **Saved logins** are stored encrypted for your Windows user on Windows, and as a file readable only by you on Linux and macOS.
- **The stealth fetcher** imitates a browser's network fingerprint to get past refusals. Respect the sites you read.
- **Wayback copies** of walled or removed pages are served as archived copies and stamped as such.
- **Research mode** (`osearch research`) runs the engines' own research agents, which cost real money; without `--approved` it only prints an estimate.

## Build

```
bash build.sh                       # the web UI, then the release binary
cargo test --bin osearch
cargo build --release --no-default-features --features stealth   # without the web UI
```

Needs Rust, CMake, LLVM (libclang), NASM on Windows, and Node.js for the web UI. `OSEARCH_INSTALL_DIR` makes `build.sh` copy the binary there.

## Credits

osearch began as a copy of [search-cli](https://github.com/paperfoot/search-cli) and borrows from several projects; [NOTICE.md](NOTICE.md) lists them all and what came from each.

## Licence

OmniSearch CLI is free software under the [GNU Affero General Public License v3.0 only](LICENSE) (AGPL-3.0-only). Copyright (C) 2026 Anshul Jain.

You may use, change and share it. If you share a changed version, or run one as a service for other people, its source goes with it under the same licence. The parts taken from other projects keep their own licences: [NOTICE.md](NOTICE.md) lists them, and their licence texts are in [licenses/](licenses/).
