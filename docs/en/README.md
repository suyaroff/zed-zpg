# zpg guide

Русская версия: [docs/ru](../ru/README.md).

zpg is a Zed extension for PostgreSQL. A table opens as a plain text file: edit cells as text, save and apply the changes, and zpg turns them into `UPDATE`, `INSERT` and `DELETE` and runs them in one transaction.

Besides tables, zpg runs statements from `.sql` files with completion, checking and EXPLAIN, shows table structure, exports data and shows server activity. All of it goes through Zed's usual actions: **F12** (go to definition), **hover**, **completion** and **`ctrl-.`** (code actions). Zed extensions can't add panels of their own, so zpg works through text buffers and a language server.

```
# zpg v1 conn=local table=zpg_demo.orders pk=id page=1 limit=100 total=9 width=40 apply=manual
| id | customer_id | status    | created_at                    | total  | comment                     |
|----|-------------|-----------|-------------------------------|--------|-----------------------------|
| 1  | 1           | paid      | 2026-09-28 01:40:44.321115+05 | 64.90  | NULL                        |
| 2  | 1           | shipped   | 2026-10-06 01:40:44.321115+05 | 289.00 | Gift wrapping               |
| 3  | 2           | new       | 2026-10-17 23:40:44.321115+05 | 39.40  | NULL                        |
```

## Contents

1. [Installation](01-install.md): the binary, the extension, Zed settings.
2. [Connections and settings](02-connections.md): `~/.config/zpg/config.json`, connection strings, `read_only`, SSH.
3. [Quick start](03-quickstart.md): from an empty project to the first edit in five minutes.
4. [Catalog](04-catalog.md): the list of tables, their structure (DDL), dropping.
5. [Table page](05-table.md): the header, filters, sorting, pages, wide tables.
6. [Editing](06-editing.md): changing, inserting and deleting rows, writing to the database, the change log and undo, CSV import.
7. [Relations and the row view](07-relations.md): following foreign keys both ways, the vertical row view.
8. [SQL files](08-sql.md): running, parameters, results, completion, checking, dangerous statements, EXPLAIN, history, cancelling.
9. [Administration](09-admin.md): server activity, stopping other sessions' queries.
10. [Export](10-export.md): CSV, JSON, INSERT.
11. [Reference](11-reference.md): every action, setting, header field, file, error and limitation.

## I want to…

| Task | Where |
|---|---|
| look at a table | [Catalog](04-catalog.md#opening-a-table) → F12 |
| find rows with a given value | [Filter by value](05-table.md#filtering-by-a-cells-value) or [`where`](05-table.md#header) |
| change a value and write it | [Editing](06-editing.md#changing-a-value) |
| add a row | [Insert row](06-editing.md#adding-a-row) |
| load rows from a CSV file | [Import](06-editing.md#importing-a-csv-file) |
| undo what I wrote to the database | [Change log](06-editing.md#change-log-and-undo) |
| go to a related record | [F12 on a foreign key](07-relations.md#following-a-foreign-key) |
| see who references a row | [Referencing rows](07-relations.md#who-references-a-row) |
| edit long text or JSON | [Row view](07-relations.md#row-view) |
| run a statement | [SQL files](08-sql.md#running) |
| run a statement with `$1` | [Parameters](08-sql.md#parameters) |
| understand why a query is slow | [EXPLAIN](08-sql.md#explain) |
| stop a hanging query | [Cancelling](08-sql.md#cancelling-a-long-statement), [someone else's: via activity](09-admin.md) |
| see a table's structure | [Show DDL](04-catalog.md#table-structure-ddl) |
| export data | [Export](10-export.md) |
| work with production safely | [`read_only`](02-connections.md#read-only), [dangerous statements](08-sql.md#dangerous-statements) |

## Examples

Every example in this guide uses the demo schema `zpg_demo` (customers, products, orders). Create it to follow along:

- [`docs/examples/demo.sql`](../examples/demo.sql) creates the `zpg_demo` schema with data; running it again resets the data, `DROP SCHEMA zpg_demo CASCADE` removes everything.
- [`docs/examples/tour.sql`](../examples/tour.sql) is a step-by-step walk through the features of `.sql` files.

The demo schema is described in the [quick start](03-quickstart.md#demo-schema).
