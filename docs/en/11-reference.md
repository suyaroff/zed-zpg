# Reference

## Zed keys zpg works through

| Key | What it does in zpg |
|---|---|
| **F12** / `ctrl`+click (go to definition) | open a table from the catalog or `.sql`, follow a foreign key, open a row by its key |
| **hover** (cursor on the spot) | a table's columns, a cell's type and full value, the referenced row, who references it |
| **`ctrl-.`** (code actions) | every action below |
| **`ctrl-space`** (completion) | cell values, foreign keys included, header fields, tables and columns in `.sql` |
| **`ctrl-shift-o`**, the Outline panel | the catalog's tables; the statements of `.sql` (with `"document_symbols": "on"`) |
| **`ctrl-t`** (project symbols) | find a table from any file, see [Catalog](04-catalog.md#finding-a-table) |
| **`ctrl-s`** | save; with `apply=save`, write to the database right away; in `.sql`, check the statements again |
| buttons above statements (code lens) | **▶ Run on …**, **Explain** |
| buttons above the header of `.pgt`, `.pgr` and the catalog | **Apply changes**, **Next/Previous page**, **Refresh**, **Open settings file**, see [Table page](05-table.md#buttons-above-the-header) |

## All actions (`ctrl-.`)

### Catalog (`.pgdb`)

| Action | When | What it does |
|---|---|---|
| **zpg: Open \<table\>** | cursor on a table line | opens the table's page (same as F12) |
| **zpg: Show DDL of \<table\>** | cursor on a table line | the table's `CREATE …` with indexes, triggers, comments → [Catalog](04-catalog.md#table-structure-ddl) |
| **zpg: Back up \<table\> (pg_dump)** | cursor on a table line | `pg_dump --table` into `.zpg/<connection>/backups/` → [Backup](04-catalog.md#table-backup-pg_dump) |
| **zpg: Drop \<table\>… (opens the SQL to review and run)** | cursor on a table line | opens a file with the `DROP` to review and run → [Catalog](04-catalog.md#dropping-a-table) |
| **zpg: Server activity of \<connection\>** | a line in a connection's section | the server's sessions → [Administration](09-admin.md) |
| **zpg: Databases of \<connection\>** | a line in a connection's section | the server's databases, without templates → [Administration](09-admin.md#databases) |
| **zpg: Schemas of \<connection\>** | a line in a connection's section | the schemas, empty ones included → [Administration](09-admin.md#schemas) |
| **zpg: Refresh catalog** | always | rereads the list of tables |
| **zpg: Open settings file** | always | opens `~/.config/zpg/config.json` |

### Table page (`.pgt`)

| Action | When | What it does |
|---|---|---|
| **zpg: Apply changes (…)** | there are unwritten edits, the file is saved | writes them in one transaction → [Editing](06-editing.md#when-changes-go-to-the-database) |
| **zpg: Preview SQL** | there are unwritten edits | opens the SQL that will run |
| **zpg: Open row** | cursor on a data row | opens the row vertically → [Row view](07-relations.md#row-view) |
| **zpg: Insert row** | the table is editable | inserts a new row template under the cursor |
| **zpg: Import rows from \<file\>.csv** | the table is editable, the file is in `imports/` | adds the file's rows to the page, writes nothing to the database → [Import](06-editing.md#importing-a-csv-file) |
| **zpg: Import CSV…** | the table is editable, `imports/` has no files | creates the folder and shows its path |
| **zpg: Open change log** | the table already has a log | opens the written changes and their undo statements → [Change log](06-editing.md#change-log-and-undo) |
| **zpg: Filter: col = '…'** / **<>** / **IS NULL** / **IS NOT NULL** | cursor on a data cell, no edits | sets `where`; if one is set, **Also filter** adds with `AND` → [Filter](05-table.md#filtering-by-a-cells-value) |
| **zpg: Clear filter (where=)** | `where` is set, no edits | removes the filter |
| **zpg: Sort by \<column\> ↑ / ↓** | cursor on a column name, no edits | sorts, from the first page |
| **zpg: Next / Previous / First / Last page (2/37)** | no edits | another page → [Paging](05-table.md#paging) |
| **zpg: Refresh** / **Refresh (discard changes)** | always | rereads the page and the structure from the database, dropping edits |
| **zpg: Apply changes manually (apply=manual)** / **on save (apply=save)** | always | switches the write mode in the header |
| **zpg: Show DDL** | always | the table's DDL |
| **zpg: Back up \<table\> (pg_dump)** | always | a backup of the table with `pg_dump` |
| **zpg: Export page as CSV / JSON / INSERT statements** | always | exports the loaded page → [Export](10-export.md) |
| **zpg: Export all rows matching where= as CSV** | always | exports every row via `COPY` |
| **zpg: Rows of \<table\> referencing this (\<columns\>)** | cursor on a data row | rows of another table that refer to this one; one per foreign key, at the end of the menu → [Relations](07-relations.md#who-references-a-row) |

### Row view (`.pgr`)

| Action | When | What it does |
|---|---|---|
| **zpg: Apply changes (1 update)** | there is an unwritten edit | writes the row |
| **zpg: Refresh row (discard changes)** | always | rereads the row from the database |
| **zpg: Open change log** | the table already has a log | as on the table page |
| **zpg: Rows of … referencing this (…)** | always | as on the table page |

### `.sql`

| Action | When | What it does |
|---|---|---|
| **zpg: Run statement on \<connection\>** | cursor on a statement | runs it (same as ▶) |
| **zpg: Run selection on \<connection\>** | text is selected | runs the selected statements in one transaction |
| **zpg: Explain statement on \<connection\>** | cursor on `SELECT`/`INSERT`/`UPDATE`/`DELETE`/`WITH`/`VALUES`/`MERGE` | the plan without running (same as the Explain button) |
| **zpg: Explain analyze statement on \<connection\> (runs it, then rolls back)** | the same | runs with timing and rolls back → [EXPLAIN](08-sql.md#explain) |

### Query result (`.pgres`)

| Action | When | What it does |
|---|---|---|
| **zpg: Run again** | always | reruns the statement |
| **zpg: Edit rows in table** | all columns from one table, its key among them | the table's page filtered by the statement → [Results](08-sql.md#editing-the-rows-of-a-result) |
| **zpg: Export result as CSV / JSON** | always | exports the rows shown |
| **zpg: Export all rows of the query as CSV (runs it again)** | the result was cut at `max_result_rows` | exports every row via `COPY` → [Export](10-export.md#what-can-be-exported) |
| **zpg: Cancel the query of pid N** | there is a `pid` column, cursor on a row | `pg_cancel_backend` → [Administration](09-admin.md#cancelling-a-query-or-a-session) |
| **zpg: Terminate backend pid N…** | the same | `pg_terminate_backend`, after confirmation |

### While a statement or pg_dump runs

| Action | Where | What it does |
|---|---|---|
| **zpg: Cancel the statement running on \<connection\>** | in any file, while a statement runs | cancels it → [Cancelling](08-sql.md#cancelling-a-long-statement) |
| **zpg: Cancel pg_dump of \<table\>** | in any file, while a backup runs | stops `pg_dump`, deletes the unfinished file |

## Language server commands

Code actions and code lenses call them; you don't need them by hand, but they show in the logs.

| Command | Action |
|---|---|
| `zpg.open`, `zpg.openRow` | Open \<table\>, Open row |
| `zpg.refresh`, `zpg.page`, `zpg.sort`, `zpg.filter` | Refresh, pages, sorting, filter |
| `zpg.apply`, `zpg.previewSql` | Apply changes, Preview SQL |
| `zpg.referencing` | Rows of … referencing this |
| `zpg.ddl`, `zpg.drop`, `zpg.backup` | Show DDL, Drop, Back up |
| `zpg.run`, `zpg.rerun`, `zpg.explain`, `zpg.editResult` | running, Run again, Explain, Edit rows in table |
| `zpg.export` | every kind of export |
| `zpg.import`, `zpg.changes` | Import rows, Open change log |
| `zpg.activity`, `zpg.databases`, `zpg.schemas`, `zpg.signal`, `zpg.cancel` | activity, the lists of databases and schemas, Cancel/Terminate backend, cancelling your own statement |
| `zpg.openConfig` | Open settings file |

## Settings

The full description is in [Connections and settings](02-connections.md#all-settings).

| Setting | Default |
|---|---|
| `connections` | — |
| `page_size` | `100` |
| `max_cell_width` | `40` |
| `apply` | `"manual"` |
| `max_result_rows` | `1000` |
| `statement_timeout_ms` | `60000` |
| `check_sql` | `true` |
| `pg_dump` | `"pg_dump"` |
| `pg_dump_args` | `[]` |

Environment variables: `ZPG_CONFIG` is the path of the settings file; `ZPG_ZED` the Zed CLI for `zpg open`/`zpg config`; `${VAR}` in a connection string.

## Table header fields

| Field | Editable | Description |
|---|---|---|
| `conn` | no | the connection |
| `table` | no | `schema.table` |
| `pk` | no | the primary key columns; `""` means no key (read-only) |
| `page` | yes | page number, from 1 |
| `limit` | yes | rows per page, 1–10,000 |
| `total` | no | how many rows match `where`; `~N` is an estimate |
| `width` | yes | the width values are cut at, `0` for no cutting |
| `apply` | yes | `manual` or `save` |
| `where` | yes | SQL condition |
| `order` | yes | SQL ordering; without it, by the key |
| `columns` | yes | visible columns, comma-separated |

Details: [Table page](05-table.md#header).

## Special values in cells

| You write | Meaning |
|---|---|
| `NULL` | SQL NULL |
| `DEFAULT` | the default |
| `+` in the key | a new row, default key |
| empty | the empty string `''` |
| `\NULL`, `\DEFAULT`, `\+` | the text `NULL`, `DEFAULT`, `+` |
| `\|`, `\\`, `\n`, `\t`, `\r`, `\s` | `\|`, `\`, line feed, tab, CR, a space at an edge |

## Files in `.zpg`

```
.zpg/
├── .gitignore                         # "*": the folder stays out of git
├── catalog.pgdb                       # the catalog
├── history.sql                        # history of runs from .sql
└── local/                             # a folder per connection
    ├── zpg_demo.orders.pgt            # a table page
    ├── zpg_demo.orders.preview.sql    # the last Preview SQL
    ├── zpg_demo.orders.changes.sql    # the Apply log and undo statements, newest on top
    ├── zpg_demo.orders.ddl.sql        # the last Show DDL
    ├── drop-zpg_demo.orders.sql       # the last Drop…
    ├── activity.pgres                 # server activity
    ├── databases.pgres                # the server's databases
    ├── schemas.pgres                  # the schemas
    ├── backups/
    │   └── zpg_demo.orders-20261018-021333.sql   # pg_dump backups (.sql, .dump, .tar)
    ├── exports/
    │   └── zpg_demo.orders.csv        # exports (.csv, .json, .sql)
    ├── imports/
    │   └── customers.csv              # files to import, put there by you
    ├── results/
    │   ├── report.pgres               # the result of the last run from report.sql
    │   └── report.plan.txt            # the last Explain from report.sql
    ├── zpg_demo.orders.rows/
    │   ├── 1.pgr                      # the row view of key 1
    │   └── .state/
    └── .state/
        └── zpg_demo.orders.pgt.json   # what was loaded from the database; edits are compared with it
```

- `.zpg` is created next to the nearest `.zed` folder or at the project root.
- `.state/*.json` is how zpg knows what exactly you changed. Don't delete it while the file has unwritten edits.
- The whole `.zpg` folder can be deleted when there are no unwritten edits: it is recreated on the next open. The change logs go with it.
- Table values, exports, the history and the change logs, including the values of deleted rows, are stored on disk in plain text. Keep that in mind for sensitive data.

## Errors and what to do

### In a table and the row view

| Message | Cause | What to do |
|---|---|---|
| `duplicate key: this row appears twice` | the same row (by key) appears twice | delete the copy, change the key or put `+` |
| `value is cut to fit the line…` | an edit of a cut value | open the row (F12 on the key), increase `width` or narrow `columns` |
| `row has N cells, the page shows M columns` | a `\|` was deleted or an extra one added | restore the row's markup |
| `a new row needs …: add to columns=` | a new row lacks a required hidden column | add it to `columns=` |
| `` `columns`: no such column … `` | a typo in `columns=` | fix the name (completion helps) |
| `rows are N characters and Zed wraps lines over 512…` | the table doesn't fit even after narrowing the columns | keep only the columns you need with `columns=` |
| `column row must stay …` | the line with the column names was changed | restore it |
| `expected field …`, `missing field …` | a field was deleted or moved in the row view | put the field back |
| `key fields can't be changed here` | the key was changed in the row view | the key is changed only in the table |
| `read-only: …` | no primary key, a view or a `read_only` connection | such a table can only be viewed |
| `row was changed or deleted since it was loaded` | the row was changed in the database after loading | **Refresh**, then edit again |
| `not saved: <Postgres error>` | the database rejected the statement (NOT NULL, FK, UNIQUE, CHECK, type) | fix the value in the row shown |
| `no saved state for this file` | the file wasn't opened from the catalog, or `.state` was deleted | **Refresh** or open it again from the catalog |
| `apply or discard changes first` | changing the page/sorting/filter with unwritten edits | **Apply changes** or **Refresh (discard changes)** |
| `no more rows` | the next page is empty | you're on the last page |
| `zpg: saved, but the change log wasn't written: …` | the write to the database succeeded, but the log file couldn't be written | check permissions and disk space; there is no undo for this write |

### CSV import

| Message | Cause | What to do |
|---|---|---|
| `` `x` is not a column of … `` | the file's header has a name the table doesn't have | fix the header |
| `` `x` is not on the page: add it to columns= `` | the column is hidden with `columns=` | add it to `columns=` |
| `` `x` is in the file twice `` | the column appears twice in the header | keep one |
| `line N: M fields, the header has K` | line N has a different number of fields | fix the line; check the quotes and the delimiter |
| `line N: a quote is not closed` | a quote opened on line N isn't closed | close it |
| `the file is not UTF-8…` | another encoding | save the file as UTF-8 |
| `the file has N rows, at most 2000…`, `the file is over 10 MB…` | the file is over the limit | use `psql \copy` |

### Connections

| Message | Cause | What to do |
|---|---|---|
| `! …` in the catalog | couldn't connect | check the connection string and the environment variables |
| `…/config.json: …` | an error in the settings file | fix the spot from the message; until then the previous settings apply |
| ``no connection `x` in …`` | the file refers to a connection the settings don't have | add it to `~/.config/zpg/config.json` |
| ``connection `x` is disabled (`enabled: false` in …)`` | `-- @conn x` or an open table belongs to a disabled connection | remove `"enabled": false`, see [Disabling a connection](02-connections.md#disabling-a-connection) |
| `environment variable X is not set` | the URL has `${X}`, but the variable isn't set | set the variable and restart Zed |
| `error communicating with the server`, `terminating connection…` when writing or running SQL | the connection broke while the statement ran | run it again: zpg reconnects; reading retries by itself, see [Dropped connections](02-connections.md#dropped-connections) |
| `invalid peer certificate: UnknownIssuer` | `verify-ca`/`verify-full`, but the certificate isn't signed by a trusted CA | set the CA in `sslrootcert`, see [TLS](02-connections.md#tls-and-certificate-verification); or `sslmode=require` to skip verification |
| `invalid peer certificate: certificate not valid for name …` | `verify-full`, but the certificate is issued for another name | connect by the name in the certificate (`host=name hostaddr=address`) or use `sslmode=verify-ca` |
| `sslrootcert …: No such file or directory` | the `sslrootcert` file doesn't exist | check the path |
| `ssh to …: Host key verification failed` | the server isn't in `known_hosts` | log in to the server with `ssh` from a terminal once |
| `ssh to …: Permission denied (publickey…)` | the key didn't fit or isn't in the agent | `ssh-add`, check `~/.ssh/config`; password login isn't supported |
| `ssh to …: no tunnel after 20 s` | ssh hung while connecting | check that the server is reachable and the `ProxyJump` |
| `can't run pg_dump: No such file or directory` | `pg_dump` isn't installed or isn't in `PATH` | install `postgresql-client` or set the path in `"pg_dump"` |
| `pg_dump failed: … server version mismatch` | a `pg_dump` older than the server can't dump it | set a version not older than the server's in `"pg_dump"` |
| `pg_dump's directory format can't be written to one file` | `pg_dump_args` has `-Fd` | use `-Fc` or the plain format |

### SQL

| Message | Cause | What to do |
|---|---|---|
| an underlined error after saving | [checking](08-sql.md#checking-statements) found an error | fix it and save |
| an error after ▶, `(rolled back)` | Postgres rejected the statement, everything was rolled back | fix the underlined spot and run again |
| a yellow underline, the question "Run on …?" | a [dangerous statement](08-sql.md#dangerous-statements) | check the statement, **Run** or **Cancel** |
| `` there is no parameter $1; give it a value with a `-- @param $1 = …` line… `` | `$1` has no value | add [`-- @param`](08-sql.md#parameters) right above the statement |
| `the file changed since this ▶ was drawn…` | the file was edited and zpg didn't find the statement | press ▶ again |
| `zpg: cancelled (rolled back)` | the statement was cancelled | — |
| `canceling statement due to statement timeout` | `statement_timeout_ms` was hit | increase the setting or speed up the statement |
| `read-only: query result…` in `.pgres` | results aren't edited here | **Edit rows in table** |
| `the query has a string spanning lines…` | for "Edit rows in table" the statement must fit on the header's single line | remove the line break from the string literal |
| no buttons above statements, the table header or the catalog | the SQL extension isn't installed, code lens is off, or there are no connections | [Installation](01-install.md) |

Server messages pop up in the bottom right corner of Zed. The detailed log: `ctrl-shift-p` → **dev: open language server logs** → `zpg`.

## Limitations

- It is text, not a grid: tables scroll horizontally. Zed wraps lines longer than 512 characters regardless, see [Wide tables](05-table.md#wide-tables).
- The server handles one action at a time: while a statement runs, hover and completion wait. A statement can be [cancelled](08-sql.md#cancelling-a-long-statement), and `statement_timeout_ms` limits how long it runs.
- A statement with `$1` runs only if the value is given in a [`-- @param`](08-sql.md#parameters) line. The value is put in as a literal, expressions aren't evaluated. `psql` commands (`\d`, `\copy`) are not supported.
- CSV import only adds rows and is limited to 2000 rows and 10 MB, see [Import](06-editing.md#importing-a-csv-file). Undoing a write is done by hand from the [change log](06-editing.md#change-log-and-undo) and doesn't restore what cascades did.
- A page is loaded into the buffer whole. For large tables use `where` and `limit` rather than paging through thousands of pages.
- The server's certificate is verified only with `sslmode=verify-ca` / `verify-full`; client certificates (`sslcert`, `sslkey`) are not supported; SSH works with keys only.
- With `apply=save` every save is written to the database right away. With Zed's autosave (`"autosave"`) use only `manual`.
- The DDL from **Show DDL** doesn't include owners, grants and storage options; for a full dump use `pg_dump`.

---
[← Export](10-export.md) · [Contents](README.md)
