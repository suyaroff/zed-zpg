# zpg — PostgreSQL tables in Zed

Zed extensions cannot add custom panels, so zpg shows a PostgreSQL table as a text buffer and writes your edits back to the database. Edit cells like any text, save, run *Apply changes*, and zpg turns the difference into `UPDATE`/`INSERT`/`DELETE` in one transaction.

```
# zpg v1 conn=local table=zpg_demo.orders pk=id page=1 limit=100 total=9 width=40 apply=manual
| id | customer_id | status    | created_at                    | total   | comment             |
|----|-------------|-----------|-------------------------------|---------|---------------------|
| 1  | 1           | paid      | 2026-09-28 01:40:44.321115+05 | 64.90   | NULL                |
| 2  | 1           | shipped   | 2026-10-06 01:40:44.321115+05 | 289.00  | Подарочная упаковка |
| +  | 3           | new       | DEFAULT                       | DEFAULT | NULL                |
```

**Documentation (Russian): [docs/ru](docs/ru/README.md).** Demo schema to try everything on: [docs/examples/demo.sql](docs/examples/demo.sql); a walk through the `.sql` features: [docs/examples/tour.sql](docs/examples/tour.sql).

## Install

```sh
cargo install --path crates/zpg     # puts `zpg` into ~/.cargo/bin
```

1. In Zed: `zed: install dev extension` → pick this directory. Without `zpg` in `PATH` the extension downloads a prebuilt binary from the GitHub releases of the repository in `extension.toml`.
2. Install Zed's **SQL** extension (for `.sql` files) and set `"code_lens": "on"`.
3. Add a task to open the catalog (`~/.config/zed/tasks.json`):

   ```json
   { "label": "zpg: open catalog", "command": "zpg", "args": ["open", "$ZED_WORKTREE_ROOT"], "reveal": "never", "hide": "on_success" }
   ```

4. Add connections to `~/.config/zpg/config.json` (created on first start; `zpg config` opens it):

   ```jsonc
   {
     "connections": {
       "local": "postgresql:///shop?host=/var/run/postgresql",
       "prod": { "url": "postgresql:///shop?host=/var/run/postgresql", "ssh": "deploy@prod", "read_only": true },
     },
   }
   ```

Details: [install](docs/ru/01-install.md), [connections and settings](docs/ru/02-connections.md).

## Features

Everything works through Zed's own actions: **F12** (go to definition), **hover**, **completion** and **`ctrl-.`** (code actions).

**Catalog** ([docs](docs/ru/04-catalog.md)) — `.zpg/catalog.pgdb` lists every connection with its tables and row estimates. Hover shows columns, size and indexes; *Show DDL* rebuilds `CREATE TABLE` with constraints, indexes, triggers and comments; *Drop…* opens the `DROP` to review.

**Table pages** ([docs](docs/ru/05-table.md)) — `.pgt` files with a header line for `where`, `order`, `columns`, `page`/`limit`, `width` and `apply`, and `total` rows matching. Sort by a column, filter by a cell's value, page with *Next/Last page (2/37)*. Typed hover for json, timestamps, arrays, bytea.

**Editing** ([docs](docs/ru/06-editing.md)) — `NULL`, `DEFAULT`, `+` for a new key, `\n`/`\|` escapes; *Insert row* template; *Preview SQL*; one transaction per apply; `xmin` guards against concurrent changes; `apply=save` writes on every save.

**Relations** ([docs](docs/ru/07-relations.md)) — F12 on a foreign key opens the referenced row; *Rows of … referencing this* goes the other way; the `.pgr` row view edits long text and pretty-printed jsonb.

**SQL files** ([docs](docs/ru/08-sql.md)) — ▶ runs a statement (`-- @conn` picks the connection), results open as `.pgres`, *Edit rows in table* turns a result into an editable page. Completion, hover and F12 for tables and columns; errors underlined on save (Postgres parses each statement, nothing runs); warnings and a *Run / Cancel* prompt for `UPDATE`/`DELETE` without `WHERE`, `TRUNCATE`, `DROP`; *Explain* / *Explain analyze* (rolled back); `.zpg/history.sql`; cancel a running statement from `ctrl-.` or the status bar.

**Administration** ([docs](docs/ru/09-admin.md)) — *Server activity* from `pg_stat_activity` with `blocked_by`; cancel or terminate a backend from any result with a `pid` column.

**Export** ([docs](docs/ru/10-export.md)) — a page or a result as CSV, JSON or INSERT statements; all rows matching `where` via `COPY`.

All actions, settings, header fields, files and error messages: [reference](docs/ru/11-reference.md).

## Development

See [CLAUDE.md](CLAUDE.md) for the layout of the crates. Checks, as in CI:

```sh
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --release --target wasm32-wasip2   # needs `rustup target add wasm32-wasip2`
```
