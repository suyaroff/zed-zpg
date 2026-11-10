# Export

The data of a table page or a query result can be saved as CSV, JSON or a set of `INSERT`s. The files go to `.zpg/<connection>/exports/` and open right away.

## What can be exported

`ctrl-.` on a table page (`.pgt`):

| Action | What it exports | File |
|---|---|---|
| **zpg: Export page as CSV** | the loaded page | `exports/zpg_demo.products.csv` |
| **zpg: Export page as JSON** | the loaded page | `exports/zpg_demo.products.json` |
| **zpg: Export page as INSERT statements** | the loaded page | `exports/zpg_demo.products.sql` |
| **zpg: Export all rows matching where= as CSV** | **all** rows matching the header's `where`, via `COPY` | `exports/zpg_demo.products.csv` |

`ctrl-.` in a query result (`.pgres`):

| Action | What it exports | File |
|---|---|---|
| **zpg: Export result as CSV / JSON** | the rows shown, at most `max_result_rows` | `exports/<name of the .sql file>.csv\|.json` |
| **zpg: Export all rows of the query as CSV (runs it again)** | **all** rows of the query, via `COPY`; offered only when the result was cut (`rows=1000+`) | `exports/<name of the .sql file>.csv` |

- The export has the **visible columns** (respecting `columns=`; key columns are always visible) and **the data as in the database**, without unwritten edits in the buffer.
- "Export page" takes only the loaded page (`limit` rows). To export the whole table or the whole selection, use **Export all rows**: it reads from the database with `COPY` in `order` and doesn't depend on `limit`. It can be [cancelled](08-sql.md#cancelling-a-long-statement) like a statement.
- **Export all rows of the query** runs the query again, so the data may differ from what was shown. It runs in a read-only transaction: a statement that changes data (`UPDATE … RETURNING`) returns an error and isn't run again. Such results are exported only within `max_result_rows`.
- The file is overwritten on every export.
- Files over 20 MB are saved but not opened in the editor.

## Formats

Using `zpg_demo.products` with `where="sku in ('KB-01', 'HD-01')"`:

### CSV

```csv
id,sku,title,price,tags,attrs,active
1,KB-01,Keyboard,45.00,"{periphery,usb}","{""layout"": ""ru/en"", ""wireless"": false}",true
4,HD-01,Headphones,79.00,{audio},,true
```

- The first line is the column names.
- Values are quoted only when needed: a comma, a quote, a line break, a space at an edge or an empty value. A quote is doubled.
- NULL is an empty field (`HD-01,…,,true`), an empty string is `""`.

### JSON

```json
[
  {"id": 1, "sku": "KB-01", "title": "Keyboard", "price": 45.00, "tags": "{periphery,usb}", "attrs": {"layout": "ru/en", "wireless": false}, "active": true},
  {"id": 4, "sku": "HD-01", "title": "Headphones", "price": 79.00, "tags": "{audio}", "attrs": null, "active": true}
]
```

- An array of objects, one object per row.
- Numbers (`integer`, `bigint`, `numeric`, `real`…) and `boolean` stay numbers and `true`/`false`; `json`/`jsonb` are put in as they are; everything else is a string (Postgres arrays too).
- NULL is `null`. `NaN` and `Infinity` are written as strings.

### INSERT

```sql
INSERT INTO zpg_demo.products (id, sku, title, price, tags, attrs, active) VALUES
  ('1', 'KB-01', 'Keyboard', '45.00', '{periphery,usb}', '{"layout": "ru/en", "wireless": false}', 'true'),
  ('4', 'HD-01', 'Headphones', '79.00', '{audio}', NULL, 'true');
```

- 500 rows per `INSERT`.
- Every value is a string literal: Postgres casts them to the column types on insert. NULL is `NULL`.
- Handy for moving a few rows between databases: export from `stage`, open the file, add `-- @conn local` as the first line and press ▶.
- Only for table pages (a query result has no table to insert into).

---
[← Administration](09-admin.md) · [Contents](README.md) · [Reference →](11-reference.md)
