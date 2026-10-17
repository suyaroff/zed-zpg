# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

zpg is a Zed extension that shows PostgreSQL tables as text buffers and writes edits back to the database. User-facing behavior (header fields, editing rules, code actions, settings) is documented in `README.md`; `docs/guide.ru.md` is the Russian guide — keep both in sync with behavior changes.

## Commands

Same checks as CI (`.github/workflows/ci.yml`):

```sh
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --release --target wasm32-wasip2   # the Zed extension (root crate); needs `rustup target add wasm32-wasip2`
```

- Single test: `cargo test -p zpg-core <test_name>` (or `-p zpg`). Tests are unit tests inside the modules; none need a running Postgres.
- Install the server locally: `cargo install --path crates/zpg`. In Zed: `zed: install dev extension` → this directory. The extension prefers `zpg` from `PATH` over downloading a release.
- Release: bump the version in all three `Cargo.toml`s and `extension.toml`, tag `v*`; `release.yml` builds `zpg-<target>.tar.gz|zip` assets, whose names `src/lib.rs` relies on.

## Architecture

Three crates in one workspace:

- **Root crate (`src/lib.rs`)** — the wasm Zed extension. Only locates/downloads the `zpg` binary and starts it as `zpg lsp` with the project's shell env. Zed's `lsp.zpg.settings` is not used.
- **`crates/zpg-core`** — pure, DB-free logic: parsing and rendering the text formats and computing changes. This is where nearly all tests live.
  - `header.rs`: the `# zpg v1 ...` first line (`TableHeader`, `Query`, `Apply`).
  - `table.rs` / `record.rs` / `catalog.rs`: render+parse `.pgt` table pages, `.pgr` row views, `.pgdb` catalog.
  - `cell.rs`: cell escaping (`NULL`, `DEFAULT`, `+`, `\|`, `\s`…) and display width.
  - `snapshot.rs`: `Snapshot` = what was loaded (columns, pk, FKs, rows with `xmin`). Lines are capped at `MAX_LINE` (500) because Zed force-wraps >512 columns.
  - `diff.rs`: buffer vs. snapshot, matched by primary key → `Op::{Update, Insert, Delete}`.
  - `sql.rs`: ops → SQL (values sent as text and cast; UPDATE/DELETE guarded by `xmin`). `sqlsplit.rs`: splitting `.sql` files into statements. `sqlctx.rs`: tokenizer, tables/aliases of a statement, and completion/name resolution over a `Schema` (for `.sql` files). `export.rs`: rows → CSV/JSON/INSERT. `history.rs`: entries of `.zpg/history.sql`.
- **`crates/zpg`** — the binary (`zpg open` CLI + `zpg lsp` language server, synchronous `lsp-server` + `postgres`).
  - `lsp/mod.rs`: the `Server`; all features (code actions, commands `zpg.*`, go-to-definition, hover, diagnostics, semantic tokens) are implemented as LSP requests since Zed extensions can't add panels. `lsp/query.rs` handles `.sql` files (code lenses, run, `.pgres` results), `lsp/sqlinfo.rs` their completion/hover/F12 over a per-connection schema cache; `lsp/export.rs` the *Export* actions; `lsp/activity.rs` *Server activity* and cancelling/terminating backends; `lsp/running.rs` reads messages on a thread of its own so a running statement can be cancelled (`Server::cancellable`); `complete.rs`, `values.rs` (typed hover), `edits.rs` (text edits) are helpers.
  - `db.rs`: introspection, page loading, applying ops in one transaction. `config.rs`: all settings come from the user file `~/.config/zpg/config.json` (JSONC, created from `TEMPLATE` on first start, re-read by the server when its mtime changes); `${VAR}` expansion, `read_only`, `ssh`. `tunnel.rs`: SSH tunnel via system `ssh`.
  - `state.rs`: the snapshot for each file is persisted as `.state/<file>.json` next to it, because on save the file on disk already contains the edited text. Generated files live under the project's `.zpg/` (`<conn>/<schema>.<table>.pgt`, `<...>.rows/<key>.pgr`).

The flow: LSP loads a page → renders text + saves `Snapshot` → user edits/saves → `diff` against the snapshot → `sql` ops → `db` applies → re-render.

## Languages and grammar

- `languages/pg-{table,row,result,catalog}/` define the Zed languages (`.pgt`, `.pgr`, `.pgres`, `.pgdb`); all share one tree-sitter grammar in `grammar/`. Semantic token type names (`zpgModified`, `zpgInserted`, `zpgColumn`) in `lsp/mod.rs` must match each language's `semantic_token_rules.json`.
- `extension.toml` loads the grammar from GitHub at a pinned `rev`, so grammar changes only take effect after they are pushed and `rev` is updated. Regenerate `grammar/src/` with the tree-sitter CLI after editing `grammar/grammar.js`.
