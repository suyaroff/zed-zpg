# Editing

A table page is plain text: change cells, add and delete rows as in any file. zpg compares the buffer with what it loaded from the database and turns the difference into statements.

## Changing a value

Retype the text of a cell. There's no need to align the columns: the table aligns itself on save.

```
| 7  | gina@example.com    | Gina Rossi       | NULL      | false | …
                                                ↓
| 7  | gina@example.com    | Gina Rossi       | Milano | false | …
```

→ `UPDATE zpg_demo.customers SET city = 'Milano' WHERE id = 7` (only the changed columns are set).

## Adding a row

- **`ctrl-.` → zpg: Insert row** puts a template under the cursor: `+` in the key, `DEFAULT` in columns with a default, `NULL` in optional ones, empty cells in required ones:

  ```
  | + |  |  | NULL | DEFAULT | DEFAULT | NULL |
  ```

- **By hand**: write a row and put `+` in the key column; the key comes from the sequence or identity.
- **As a copy**: a row with a key that isn't on the page is new too. It's handy to duplicate a row (`ctrl-alt-shift-down`) and change the key.
- **From a file**: [CSV import](#importing-a-csv-file).

A composite key (`zpg_demo.order_items`, key `order_id, product_id`) is written in full:

```
| order_id | product_id | qty | price  | amount  |
| 3        | 4          | 1   | 79.00  | DEFAULT |
```

`amount` is a generated column; only `DEFAULT` is allowed in it. After the write the trigger fires and the `total` of order 3 is recalculated.

## Deleting a row

Delete the whole line from the file (`ctrl-shift-k`) → `DELETE … WHERE id = …`.

## Special values

| You write | The database gets |
|---|---|
| `NULL` | SQL NULL |
| `DEFAULT` | the column's default |
| `+` (key column only) | a new row, default key |
| empty cell | the empty string `''` |
| `\NULL`, `\DEFAULT`, `\+` | the text `NULL`, `DEFAULT`, `+` |

## Escapes inside a value

| You write | Meaning |
|---|---|
| `\|` | the character `\|` (otherwise it's a cell border) |
| `\\` | a backslash |
| `\n`, `\t`, `\r` | line feed, tab, carriage return |
| `\s` | a space at the start or end of a value (spaces at the cell edges are trimmed) |

Example: `Two lines:\nthe second line` in a cell is a two-line value. Long text and JSON are easier to edit in the [row view](07-relations.md#row-view): line breaks look like ordinary lines there, and JSON is formatted.

Values are sent to the database as text and cast by Postgres to the column's type, so any type is edited in its usual text form: `{periphery,usb}` for an array, `{"dpi": 1600}` for jsonb, `2026-10-18 12:00+05` for timestamptz, `new`/`paid` for an enum (completion suggests them).

## Completion in cells

`ctrl-space` in a cell suggests what fits the column: `NULL` (if the column allows it), `DEFAULT` (if it has a default), `+` in the key, `true`/`false` for `boolean`, enum values (`new`, `paid`, `shipped`, `cancelled` for `orders.status`).

In a single-column foreign key, the values of the referenced table are suggested too, with a label. The same works in the [row view](07-relations.md#row-view):

```
| 3  | An|                 ← 1  Anna Petrova
```

- The label is the referenced table's `name`, `title` or `label` column, or else its first text column. Only the key value goes into the cell.
- What you typed is matched against both the value and the label, ignoring case. Up to 50 values are shown; keep typing to see the others.
- The database query gets 0.5 seconds. If the referenced table is large or locked, its suggestions just don't appear.

## What you see before writing

While you edit, zpg checks the buffer on the fly:

- the first line shows what will be written: `pending: 1 update, 1 insert, 1 delete; run the code action “Apply changes”`;
- changed cells and new rows are underlined (with semantic tokens on);
- errors are underlined right away: a wrong number of cells, a duplicate key, an edit of a cut value, text in a generated column… The list is in the [reference](11-reference.md#errors-and-what-to-do).

## When changes go to the database

### `apply=manual` (default)

`ctrl-s` only saves the file; nothing goes to the database. To write: `ctrl-.` → **zpg: Apply changes (1 update, 1 insert)**. The file must be saved. That way an accidentally deleted row or a typo doesn't reach the database.

### `apply=save`

Every `ctrl-s` writes the changes right away. Handy for quick edits, but **what was written can't be undone with `ctrl-z`**, only through the [change log](#change-log-and-undo). With Zed's autosave (`"autosave"`) use only `manual`.

To switch the mode: `apply=` in the header, `ctrl-.` → **zpg: Apply changes manually / on save**, or `"apply"` in the [settings](02-connections.md#all-settings) for newly opened tables.

## Previewing the SQL

`ctrl-.` → **zpg: Preview SQL** opens `<table>.preview.sql` with what will run:

```sql
-- What saving would run, in one transaction.
BEGIN;
UPDATE "zpg_demo"."customers" SET "city" = CAST('Milano' AS text) WHERE "id" = CAST('7' AS bigint) AND xmin = CAST('1234' AS xid) RETURNING …;
INSERT INTO "zpg_demo"."customers" ("email", "name", "city", "note") VALUES (CAST('ivan@example.com' AS text), CAST('Ivan' AS text), NULL, NULL) RETURNING …;
COMMIT;
```

## How changes are written

- All changes of a page go in **one transaction**: changed cells → `UPDATE` (only the changed columns), new rows → `INSERT`, deleted ones → `DELETE`.
- After success the page is redrawn with data from the database: new `id`s, filled-in `DEFAULT`s, values recalculated by triggers. At the bottom: `zpg: saved 1 update, 1 insert`.
- If any statement fails (NOT NULL, FK, `CHECK`, wrong type), the transaction is rolled back and **nothing is written to the database**. Postgres's error shows on the row that caused it, the edits stay in the file: fix them and apply again.

Example: `price = -5` in `zpg_demo.products` → at the bottom `zpg: nothing written, transaction rolled back: new row for relation "products" violates check constraint "products_price_check"`, and on the row itself the error `not saved: …` with Postgres's details.

## Change log and undo

Every successful write to the database (from a table page, from the row view, in `apply=save`) goes to the top of the file `.zpg/<connection>/<table>.changes.sql`. To open it: `ctrl-.` → **zpg: Open change log**. The action appears after the first write.

```sql
-- zpg: changes written by Apply to this table, newest first.

-- 2026-11-06 14:03:11 · zpg_demo.customers · 1 update
-- Applied:
--   UPDATE "zpg_demo"."customers" SET "city" = CAST('Milano' AS text) WHERE "id" = CAST('7' AS bigint) AND xmin = CAST('1234' AS xid);
-- Undo: select the statements below and run them; UPDATE 0 or DELETE 0 means the row changed since.
-- @conn local
UPDATE "zpg_demo"."customers" SET "city" = NULL WHERE "id" = CAST('7' AS bigint) AND xmin = CAST('1240' AS xid);
```

What was written stays in comments, so only the undo statements can be run.

**To undo a write:**

1. Open the log and select the statements under `-- @conn` of the entry you want.
2. `ctrl-.` → **zpg: Run selection on local**: the statements run in one transaction. An entry with one statement can be run with the ▶ button.
3. Press **Refresh** on the table page.

Keep in mind:

- Undo from the top down: the newest entries first.
- `UPDATE 0` or `DELETE 0` means the row was changed after the write, and that statement did nothing. The other statements still run.
- `UPDATE` restores the old values of the changed columns, an inserted row is deleted, a deleted one is inserted again with its old key (`OVERRIDING SYSTEM VALUE`), generated columns are recalculated.
- Undo reverts only what zpg itself wrote. Rows deleted by a cascade (`ON DELETE CASCADE`) and other changes made by foreign keys are not restored, and triggers fire again.
- The log is no substitute for a backup: the file is kept within 1 MB per table, and the oldest entries are removed. The newest entry is kept in full even if it is over the limit.
- The log keeps the values of changed and deleted rows in plain text. Keep that in mind for sensitive data.

## Importing a CSV file

The file's rows are added to the page as new rows and written with the usual **Apply changes**, so you can check and fix them before writing.

1. Put the file in `.zpg/<connection>/imports/`. `ctrl-.` → **zpg: Import CSV…** creates the folder and shows it while it has no files.
2. `ctrl-.` on the table page → **zpg: Import rows from customers.csv**. The menu shows up to 20 files.
3. Check the rows or open **Preview SQL**, then **Apply changes**. If you change your mind, **Refresh (discard changes)** removes the added rows.

```csv
email;name;city
ivan@example.com;Ivan;
olga@example.com;Olga;""
```

→ on the page:

```
| + | ivan@example.com | Ivan | NULL | DEFAULT | DEFAULT | NULL |
| + | olga@example.com | Olga |      | DEFAULT | DEFAULT | NULL |
```

- **Header:** the first line has the column names, in any order. Case doesn't matter as long as the name is unambiguous.
- **Format:** the delimiter (`,`, `;` or tab) is detected from the first line; a BOM and CRLF are understood. UTF-8 only.
- **Values, as in [export](10-export.md#csv):** an empty field is NULL, `""` is an empty string. The word `NULL` in the file is ordinary text.
- **What the file lacks:** such a column is filled as in **Insert row**. An empty key becomes `+`. Values of generated columns are ignored, so an exported file can be loaded back.
- **Hidden columns:** a column from the file hidden with `columns=` is an error, and nothing is added. Add it to `columns=`.
- **Import only adds rows.** If a key from the file is already on the page, zpg shows a duplicate error; if it is only in the database, Postgres refuses on write. Either way nothing reaches the database.
- **Limits:** 10 MB and 2000 rows, because the write goes row by row and can't be undone. For larger files use `psql \copy`.

## Protection against concurrent changes

Every `UPDATE` and `DELETE` checks that the row hasn't changed since it was loaded (by the system column `xmin`). If someone else changed it in the meantime, the write is cancelled with the error "row was changed or deleted since it was loaded; refresh to see the current data". Do **Refresh** and make the edit again.

## What can't be edited

- **Cut values (`…`).** zpg can't know what you meant. Open the [row view](07-relations.md#row-view), increase `width` in the header (`width=0` turns cutting off) or show fewer columns with `columns=`.
- **The key of an existing row.** Changing a key is deleting the old row and inserting a new one. It works, but mind foreign keys with `ON DELETE CASCADE`: deleting an order deletes its items.
- **Generated columns**: only `DEFAULT`.
- **Tables without a primary key, views, materialized views, `read_only` connections**: read-only.

---
[← Table page](05-table.md) · [Contents](README.md) · [Relations and the row view →](07-relations.md)
