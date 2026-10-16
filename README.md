# zpg — PostgreSQL tables in Zed

Подробное руководство на русском: [docs/guide.ru.md](docs/guide.ru.md).

Zed extensions cannot add custom panels, so zpg shows a table as a text buffer and writes your edits back to the database:

1. `zpg open` creates `.zpg/catalog.pgdb` in the project and opens it: every configured connection with its tables.
2. Go to definition (`F12`) on a table opens a page of it as `.zpg/<conn>/<schema>.<table>.pgt`.
3. Edit cells like any text, save, then run the code action *Apply changes*: the language server compares the buffer with what it loaded and runs the UPDATE/INSERT/DELETE statements in one transaction, then re-renders the page. With `apply=save` saving alone writes.

```
# zpg v1 conn=local table=public.users pk=id page=1 limit=100 width=40 apply=manual
| id | email     | name | active  | note                 |
|----|-----------|------|---------|----------------------|
| 1  | anna@x.io | Anna | true    | short                |
| 2  | bob@x.io  | Bob  | NULL    | long text long text… |
| +  | new@x.io  | New  | DEFAULT | NULL                 |
```

## Connections

In the project's `.zed/settings.json` (`zed: open project settings`):

```json
{
  "lsp": {
    "zpg": {
      "settings": {
        "connections": {
          "local": "postgresql:///mydb?host=/var/run/postgresql",
          "prod": { "url": "postgresql://app:${PROD_PG_PASSWORD}@db.example.com/app", "read_only": true }
        },
        "page_size": 100,
        "max_cell_width": 40,
        "apply": "manual"
      }
    }
  }
}
```

- `${VAR}` is taken from the environment Zed sees for the project, so passwords can stay out of the file. Keep `.zed/settings.json` out of git if it holds secrets.
- `read_only: true` opens every table of the connection read-only, and its transactions are read-only too.
- TLS: `sslmode=prefer` (default) and `require` encrypt without checking the certificate, like libpq; unix sockets and `sslmode=disable` don't use TLS.
- SSH: `{ "url": "postgresql:///app?host=/var/run/postgresql", "ssh": "deploy@prod", "ssh_options": ["-p", "2222"] }` tunnels through the system `ssh` (so `~/.ssh/config`, keys, agent and `ProxyJump` apply); the url names the database as the server sees it, a remote unix socket included. Key or agent auth only, and the host must already be in `known_hosts`. A dead tunnel is reopened on the next request.

## Editing rules

| You write | Meaning |
|---|---|
| `NULL` | SQL NULL (the text "NULL" is written `\NULL`) |
| `DEFAULT` | column default (text: `\DEFAULT`) |
| `+` in a key column | new row, key from its default (text: `\+`) |
| `\|` `\\` `\n` `\t` `\r` | pipe, backslash, newline, tab, CR inside a value |
| `\s` | a space at the start or end of a value (cells are trimmed) |

- Rows are matched by primary key. A row with an unknown key (or `+`) is inserted, a missing row is deleted, so changing a key value means delete + insert.
- Values are sent as text and cast by Postgres to the column type, so every type (json, arrays, enums, ranges…) is edited in its usual text form.
- Each UPDATE/DELETE also checks the row's `xmin`: if someone changed the row since it was loaded, nothing is written and the row gets an error. Refresh to load the current data.
- Values longer than `width` are cut with `…` and can't be edited in the table; hover shows the full value. Edit them in the row view, or raise `width` in the header (`0` = no limit).
- Tables without a primary key, views and materialized views are read-only.
- Generated columns accept only `DEFAULT`.
- Zed soft-wraps lines longer than 512 columns whatever `soft_wrap` says, so when a page would be wider zpg cuts the widest non-key columns to a common width; if that's not enough, pick columns with `columns=`.

## Navigation and row view

- `F12` on a foreign key cell opens the referenced table filtered to that row; hover shows the referenced row.
- `F12` on a primary key cell (or *Open row*) opens the row as `.pgr`, one `column: value` per line. Multi-line text is a block (`note: |` followed by lines indented by two spaces) and jsonb is pretty-printed. It is applied like the table it came from, and the table page is updated afterwards.
- Completion: NULL/DEFAULT/`+`, booleans and enum labels in cells; column names and keywords inside `where="…"`/`order="…"`; header fields elsewhere.

## Running SQL

zpg also attaches to `.sql` files (install Zed's SQL extension and keep `"code_lens": "on"`):

- Every statement gets a code lens `▶ Run on <conn>` (not in files over 5 MB; a lens clicked before it was redrawn finds its statement again by text, or runs nothing); `-- @conn name` picks the connection for the statements below it (not needed with a single connection). Code actions run the statement under the cursor or the selection (several statements run in one transaction).
- Rows open in a read-only `.pgres` tab (at most `max_result_rows`, read through a cursor); other statements report their tag, e.g. `UPDATE 3`. Errors roll back and are underlined at the position Postgres reports.
- If a result comes from one table and includes its key, *Edit rows in table* opens that table's page filtered with `where="id IN (SELECT id FROM (<query>) AS zpg_q)"`.
- Settings: `max_result_rows` (1000), `statement_timeout_ms` (60000).

## Header

Edit the first line and save to reload with new settings:

| Field | Meaning |
|---|---|
| `page`, `limit` | which rows to show |
| `where` | SQL condition, e.g. `where="active and created_at > now() - interval '1 day'"` |
| `order` | SQL ORDER BY list, e.g. `order=id desc` (quotes are optional for `where`, `order` and `columns`); default is the primary key |
| `width` | max cell width, `0` = no limit |
| `columns` | columns to show, e.g. `columns="name, email"`; key columns are always shown, hidden ones are left alone on write |
| `apply=manual\|save` | `manual` (default) writes only with the code action *Apply changes*; `save` writes on every save |

`conn`, `table` and `pk` are informational.

## Code actions (`ctrl-.`)

*Apply changes*, *Preview SQL*, *Sort by <column> ↑/↓* (on the column row), *Open row*, *Insert row* (a template with `+`/DEFAULT/NULL filled in), *Next/Previous page*, *Refresh*, *Apply changes manually / on save*. In a row view: *Apply changes*, *Refresh row*. In the catalog: *Open table*, *Drop table…* (opens a `.sql` with the `DROP` to review and run with ▶; the catalog refreshes after DDL run from `.sql`), *Refresh catalog*.

Pending changes are shown as a diagnostic on the header line. Changed cells and new rows are underlined via semantic tokens, which Zed only shows when enabled:

```json
"languages": {
  "Pg Table": { "semantic_tokens": "combined" },
  "Pg Row": { "semantic_tokens": "combined" }
}
```

## Install

```sh
cargo install --path crates/zpg     # puts `zpg` into ~/.cargo/bin
rustup target add wasm32-wasip2
```

In Zed: `zed: install dev extension` → pick this directory. Add a task (`~/.config/zed/tasks.json`) to open the catalog:

```json
{ "label": "zpg: open catalog", "command": "zpg", "args": ["open", "$ZED_WORKTREE_ROOT"], "reveal": "never", "hide": "on_success" }
```

Without `zpg` in `PATH` the extension tries to download a prebuilt binary from GitHub releases of the repository in `extension.toml`.
