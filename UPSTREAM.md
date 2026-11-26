# Upstream

Base: [paperfoot/search-cli](https://github.com/paperfoot/search-cli) (MIT), crate `agent-search` v0.12.1, commit `16c36c2c7051c6919518db77dbb9be18a9771ed5` (2026-11-04). Copied in unchanged, then modified; credits for every project osearch took from are in `NOTICE.md`.

Check upstream monthly (`gh api repos/paperfoot/search-cli/commits`) and cherry-pick useful engine or page-verdict fixes by hand.

Last checked 2026-11-26, up to v0.14.1 (`2026-11-20`). Taken: Hacker News drops trailing words when a long query matches nothing (`removeWordsIfNoResults`); the DNS resolver's patched release (`hickory-resolver` 0.26.4, used by `osearch verify`). Already here: X's 40 s wait. Left: posts mixed into every web search, and the hosted-service changes (removed here).

The outside tools move faster than search-cli: donsetch went 5.2.1 to 5.5.12 in the same weeks (output contract unchanged; its search now also asks Exa and Parallel through their keyless endpoints, which osearch switches off when its own Exa or Parallel key is set, so one engine's answer never counts as two engines agreeing).

## Removed

- metasearch.sh: `auth.rs`, `hosted.rs`, the hosted adapter and routing in the engine, rerank, `login`/`logout`/`whoami`, Reddit's hosted global search.
- Self-update to upstream, `skill install`, `agent-info`, the `--last` replay, the table printer, `tests/integration.rs` (it drove the old `search` command line).

## Kept, changed

- Engines, rank fusion with `also_found_by`, cost per call, `usage`, `stats`, failure categories, filter warnings, page verdicts, the untrusted-content tag, the private-address guard, the 5-minute cache.
- The social platforms call ScrapeCreators directly (`SCRAPECREATORS_API_KEY`), off by default; the ScrapeCreators YouTube is `youtubesc`.
- `providers::send` records every provider answer untouched for the run archive.

## Added

You.com, donsetch, GitHub (`gh`) and YouTube (yt-dlp) engines; `code` and `shopping` modes; fetch tiers, run archive, outline reading, crawl, login and cookie store, contract checks, research mode, balances with balance-aware engine picking and standard/wide widths (`budget.rs`; replaces upstream's `[limits]`), per-engine result counts (see `README.md`). The reading model follows fatsearch (Apache-2.0); its token estimate is lifted from fatsearch's `clean.rs`.

Later additions: community and social routes (forums, Reddit login and RSS, Telegram, Bluesky, dev.to, X), AI synthesis, background pages with a per-site fetch memory, Jina DeepSearch, the Jina reranker, `osearch wait` and `osearch replay`.

From last30days (MIT, mvanhorn/last30days-skill): `vendor/bird-search` is its vendored copy of the Bird X search client (MIT, licence in that folder; changed here to ask X for top posts instead of the latest); `src/relevance.rs` ports its relevance scoring (meaningful words only, generic ones like "best" do not count); the Reddit RSS route, the YouTube caption order and the Telegram and Bluesky routes follow its sources.
