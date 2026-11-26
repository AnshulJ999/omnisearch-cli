# Credits and third-party notices

osearch stands on other people's work. This file lists what it took or learned from each project, and the licence the borrowed parts keep. osearch itself is under the GNU AGPL v3 ([LICENSE](LICENSE)); the licence texts of the projects below are in [licenses/](licenses/).

## Code taken in

- **[search-cli](https://github.com/paperfoot/search-cli)** by Boris Djordjevic, 199 Biotechnologies and Paperfoot AI (MIT). osearch began as a copy of it (crate `agent-search` v0.12.1): the engines, rank fusion, cost per call, usage and stats, failure categories, page verdicts and the private-address guard come from there. Its copyright notice is in [licenses/search-cli-MIT.txt](licenses/search-cli-MIT.txt); what changed since is in [UPSTREAM.md](UPSTREAM.md).
- **[fatsearch](https://github.com/fatsearch/fatsearch)** (Apache-2.0): the reading model (pages kept whole, read by outline and section), the token estimate in `src/outline.rs`, and the loss check in `src/fetch/convert.rs`.
- **[last30days](https://github.com/mvanhorn/last30days-skill)** by mvanhorn (MIT): `src/relevance.rs` ports its relevance scoring; the Reddit RSS route, the YouTube caption order and the Telegram and Bluesky routes follow its sources.
- **Bird** by Peter Steinberger (MIT), the X search client, vendored through last30days in `vendor/bird-search` with its licence; changed to ask X for top posts.
- **[Open Deep Research](https://github.com/langchain-ai/open_deep_research)** by LangChain (MIT): the synthesis prompt (`prompts/synth.md`) is based on its final report prompt, with citation and opinion lines from **[GPT Researcher](https://github.com/assafelovic/gpt-researcher)** (Apache-2.0).

## Programs osearch runs

These are separate programs: osearch starts them and reads their output, and `osearch update` downloads the ones it manages. None of them is part of osearch's own files or release downloads.

- **[donsetch](https://github.com/dondai44423/donsetch)** (AGPL-3.0): keyless search and the main page reader.
- **[moli](https://github.com/lexmount/moli)** and **[Obscura](https://github.com/h4ckf0r0day/obscura)**: light headless browsers for pages built by JavaScript.
- **[crawl4ai](https://github.com/unclecode/crawl4ai)** (`crwl`): the browser fetcher for hard walls.
- **[GitHub CLI](https://cli.github.com/)** (`gh`), **[yt-dlp](https://github.com/yt-dlp/yt-dlp)**, and Playwright's Chromium.

## Services

Search and reading go through the services' own APIs under each user's own keys and terms: Brave, Exa, Tavily, You.com, Linkup, Parallel, Jina, Serper, SerpApi, Firecrawl, ScrapeCreators, Reddit, Hacker News, Lemmy, Bluesky, Telegram's public channel previews, the [Arctic Shift](https://github.com/ArthurHeitmann/arctic_shift) Reddit archive and the Internet Archive's Wayback Machine.

## Libraries

The Rust crates (`Cargo.toml`) and the web UI's npm packages (`ui/package.json`) keep their own licences.
