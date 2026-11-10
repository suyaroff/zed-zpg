# SQL files

zpg attaches to every `.sql` file in the project: it runs statements, shows results, completes table and column names, checks statements before they run and warns about dangerous ones. It needs Zed's **SQL** extension and `"code_lens": "on"`, see [Installation](01-install.md#3-sql-extension).

All the examples of this page are collected in [`docs/examples/tour.sql`](../examples/tour.sql).

## Running

Above every statement there are **▶ Run on local** and **Explain** buttons (code lens):

```sql
-- @conn local
select id, name, city, vip from zpg_demo.customers order by id;

update zpg_demo.customers set city = 'Tashkent' where id = 7;
```

- **The ▶ button** runs the statement.
- **`ctrl-.` on a statement** → **zpg: Run statement on local** does the same without the mouse.
- **Several statements**: select them → `ctrl-.` → **zpg: Run selection on local**. They run one after another **in one transaction**: if any fails, all are rolled back. That's handy for scripts, such as the whole [`demo.sql`](../examples/demo.sql).
- Statements are separated by `;`. Semicolons inside strings, `$$` function bodies and comments don't count. The data after `COPY … FROM stdin;` in `pg_dump` dumps is skipped up to the `\.` line, and lines starting with `\` (`psql` meta-commands: `\restrict`, `\set`, `\connect`…) are not statements: they are neither checked nor run.
- If you press ▶ right after an edit, before Zed redraws the buttons, zpg finds the statement itself: by its text or by where it starts. It always runs the whole statement as it is written now: if `… where id = 1` was edited to `… where id = 10`, `id = 10` runs. If it isn't found, nothing runs and zpg asks you to press again.
- Files over 5 MB (full dumps) have no buttons; run from `ctrl-.`.
- Every run is a transaction with `statement_timeout` (the `statement_timeout_ms` [setting](02-connections.md#all-settings)). For `read_only` connections the transaction is read-only.
- `VACUUM`, `CREATE INDEX CONCURRENTLY` and other commands that can't run in a transaction run without one when run on their own.

### Connection

- A `-- @conn name` line sets the connection for every statement below it, up to the next such line. One file can switch between databases:

  ```sql
  -- @conn stage
  select count(*) from zpg_demo.orders;

  -- @conn prod
  select count(*) from zpg_demo.orders;
  ```

- Without `-- @conn`: with one connection, that one is used; with several, there is a button for each above a statement (**▶ local**, **▶ prod**), and completion and checking use the first one alphabetically.

### Parameters

A statement with `$1`, `$2`, say one copied from code, can be run by giving the values in `-- @param` lines right above it:

```sql
-- @param $1 = 42
-- @param $2 = 'it''s'
-- @param $3 = NULL
select * from zpg_demo.orders where id = $1 and comment = $2 or customer_id = $3;
```

- The lines apply only to the statement under them, so an old value can't leak into another statement. There may be empty lines and `--` comments between them and the statement.
- A value is `NULL`, a string in single quotes (`''` inside is a quote) or the text up to the end of the line. It always becomes a literal: `now()` isn't evaluated but is put in as text. A multi-line value can't be given.
- The literal is put into the statement's text and Postgres decides the type: `where id = '42'`. History, `.pgres` and **Run again** keep the text with the values put in; to change them, run the statement from the `.sql` again.
- `$n` inside strings, comments and `$$` function bodies isn't replaced. A `$n` without a value stays as is: `PREPARE` and `CREATE FUNCTION … RETURN $1` work as usual, and otherwise Postgres returns an error and zpg tells you which line to add.
- Works with ▶, **Run selection**, **Explain** and **Explain analyze**.
- [Checking statements](#checking-statements) and the warnings about [dangerous statements](#dangerous-statements) look at the original text with `$1`.
- An error while running a statement with values put in is underlined at the start of the statement, not at the exact spot: Postgres's position refers to the changed text.

## Results

**A statement returns rows** (`SELECT`, `… RETURNING`): the result opens as a `.zpg/local/results/<file>.pgres` tab:

```
# zpg v1 result conn=local rows=8 time=2ms
# select id, name, city, vip from zpg_demo.customers order by id
# Rows of zpg_demo.customers: code action “Edit rows in table” opens them for editing.
| id | name             | city      | vip   |
|----|------------------|-----------|-------|
| 1  | Anna Petrova     | Tashkent  | true  |
| 2  | Bob Smith        | London    | false |
```

- Running again from the same `.sql` updates the same tab.
- The first `max_result_rows` rows are shown (1000 by default); the rest aren't fetched from the database at all, and the header says `rows=1000+`. To export them all: [**Export all rows of the query**](10-export.md#what-can-be-exported).
- The result is read-only. Hover shows a column's type and the full value of a cut cell.
- `ctrl-.` → **zpg: Run again** reruns the statement.
- `ctrl-.` → **zpg: Export result as CSV / JSON**, see [Export](10-export.md).
- If the result has a `pid` column: **Cancel the query of pid N** / **Terminate backend pid N…**, see [Administration](09-admin.md#cancelling-a-query-or-a-session).

**A statement returns no rows** (`UPDATE`, `INSERT`, DDL): a summary pops up at the bottom: `zpg: UPDATE 1 (2 ms)`. After `CREATE`/`DROP`/`ALTER` the catalog refreshes by itself.

**An error**: everything is rolled back, and the error is underlined in the `.sql` at the spot Postgres pointed to (for example, the name of a column that doesn't exist), with Postgres's hint.

### Editing the rows of a result

If every column of a result comes from one table and its primary key is among them, `ctrl-.` → **zpg: Edit rows in table** opens an ordinary page of that table, filtered by your statement:

```sql
select id, status, comment from zpg_demo.orders where status = 'new';
```

→ a `zpg_demo.orders` page with `where="id IN (SELECT id FROM (select id, status, comment from zpg_demo.orders where status = 'new') AS zpg_q)"` and `columns=` from the statement's columns (`id, status, comment`). Then edit as usual, see [Editing](06-editing.md). String literals in the statement must not span lines, since the header is a single line.

## Completion, hover, F12

zpg knows the tables and columns of the statement's connection.

- **Completion** (`ctrl-space` or just type):
  - after `from`, `join`, `update`, `into`, `table`: tables and schemas;
  - after `o.`, where `o` is an alias or a table name from the statement: its columns;
  - after `zpg_demo.`: the schema's tables;
  - elsewhere: the columns of the statement's tables, all tables and keywords.

  Tables of the `public` schema are written without the schema.

  ```sql
  select o.|            ← id, customer_id, status, created_at, total, comment
  from zpg_demo.orders o
  join zpg_demo.|       ← customers, events, order_items, orders, paid_orders, products
  ```

- **Hover** on a table shows its columns with types and comments; on a column, its type, flags (`not null`, `has default`) and comment.
- **F12** on a table or column opens the table's page.
- The list of tables is read from the database on the first suggestion and kept for 5 minutes. DDL run from `.sql` and **Refresh catalog** update it at once.
- **The Outline panel** and `ctrl-shift-o` show the file's statements by their first line, which helps in long files and `history.sql`. It needs `"document_symbols": "on"` for the SQL language, see [Zed settings](01-install.md#4-zed-settings). In files over 5 MB the list is empty.

## Checking statements

On open and save, zpg hands every statement to Postgres to parse (`PREPARE`) **without running** it, and underlines the errors:

```sql
select id, emial from zpg_demo.customers;
--         ~~~~~ column "emial" does not exist
--               hint: Perhaps you meant to reference the column "customers.email".

select * from zpg_demo.customer;
--            ~~~~~~~~~~~~~~~~~ relation "zpg_demo.customer" does not exist

select name + 1 from zpg_demo.customers;
--          ~ operator does not exist: text + integer
```

- Only statements with a clear connection are checked: by `-- @conn` or a single connection.
- If the file has a `CREATE`/`ALTER`/`DROP`/`COMMENT`/`DO` above, "does not exist" errors below it aren't shown: the object may be created by the same file.
- Untyped `$1` parameters are not errors.
- While you edit the file, the underlines are removed; after saving the check runs again.
- Files over 1 MB or with more than 300 statements aren't checked.
- To turn it off: `"check_sql": false` in the [settings](02-connections.md#all-settings). Mind that checking queries the database on every open and save; for a connection over SSH that brings up the tunnel.

## Dangerous statements

Statements that change every row or delete data for good are underlined with a warning:

| Statement | Warning |
|---|---|
| `update zpg_demo.products set active = true` | UPDATE without WHERE changes every row of zpg_demo.products |
| `delete from zpg_demo.events` | DELETE without WHERE deletes every row of zpg_demo.events |
| `update … set price = (select 1 where true)` | a `WHERE` only in a subquery doesn't count |
| `truncate zpg_demo.events` | TRUNCATE deletes every row of zpg_demo.events |
| `drop table …`, `drop schema …` | DROP TABLE can't be undone |
| `alter table … drop column …` | ALTER … DROP COLUMN deletes the column with its data |
| `explain analyze delete from zpg_demo.events` | the same as for the `DELETE` itself: `EXPLAIN ANALYZE` run with ▶ runs the statement and commits the changes |
| `with old as (…) delete from zpg_demo.events`, `with d as (delete from zpg_demo.events returning *) select …` | both the CTE bodies and the main statement are checked |

When such a statement is run (▶ or `ctrl-.`), zpg asks first:

> zpg: UPDATE without WHERE changes every row of zpg_demo.products. Run on local?
> **Run** · **Cancel**

Without **Run** nothing is run. On `read_only` connections there is no question: the write wouldn't go through anyway.

## EXPLAIN

- The **Explain** button above `SELECT`/`INSERT`/`UPDATE`/`DELETE`/`WITH`/`VALUES`/`MERGE` runs `EXPLAIN` and opens the plan as text in `.zpg/local/results/<file>.plan.txt`. The statement itself isn't run.
- `ctrl-.` → **zpg: Explain analyze statement on local** runs `EXPLAIN (ANALYZE, BUFFERS)`: the statement **really runs** so it can be timed, but in a transaction that is then rolled back, so `UPDATE`/`DELETE` change nothing. It may take long, and sequences (`nextval`) aren't reset by the rollback.
- With several connections and no `-- @conn`, there is no button: use `ctrl-.`.

```
-- EXPLAIN on local · 2 ms
-- select c.name, count(*) as orders, sum(o.total) as spent

Sort  (cost=2.59..2.61 rows=8 width=56)
  Sort Key: (sum(o.total)) DESC
  ->  HashAggregate  (cost=2.37..2.47 rows=8 width=56)
        Group Key: c.name
        ->  Hash Join  (cost=1.18..2.30 rows=9 width=21)
              Hash Cond: (o.customer_id = c.id)
              ->  Seq Scan on orders o  (cost=0.00..1.09 rows=9 width=13)
              ->  Hash  (cost=1.08..1.08 rows=8 width=24)
                    ->  Seq Scan on customers c  (cost=0.00..1.08 rows=8 width=24)
```

## Cancelling a long statement

While a statement runs, Zed's status bar spins **zpg: Running on local**. To cancel:

- `ctrl-.` in any file → **zpg: Cancel the statement running on local**;
- or `ctrl-shift-p` → **editor: cancel language server work**.

Postgres gets a cancel request, the transaction is rolled back, and at the bottom you see `zpg: cancelled (rolled back)`. **Run again**, **Explain** and **Export all rows … as CSV** are cancelled the same way.

```sql
select pg_sleep(30);   -- try it: ▶, then ctrl-. → Cancel
```

While a statement runs, zpg's other features (hover, completion, buttons) wait for it to finish. To stop someone else's query, use [server activity](09-admin.md).

## History

Every successful run (▶, `ctrl-.`, **Run again**) is appended to `.zpg/history.sql`:

```sql
-- zpg: statements run from .sql files, newest last; ▶ runs one again.

-- 2026-10-18 01:45:12 · SELECT 8 · 2 ms
-- @conn local
select id, name, city, vip from zpg_demo.customers order by id;

-- 2026-10-18 01:45:20 · UPDATE 1 · 1 ms
-- @conn local
update zpg_demo.customers set city = 'Tashkent' where id = 7;
```

- Every entry has a ▶ above it that runs the statement again on the same connection.
- The file is kept within 1 MB: when it overflows, the older half of the entries is removed.
- EXPLAIN isn't recorded in the history.

---
[← Relations and the row view](07-relations.md) · [Contents](README.md) · [Administration →](09-admin.md)
