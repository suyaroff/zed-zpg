# Table page

A page is the file `.zpg/<connection>/<schema>.<table>.pgt`: one page of a table's rows as a text table. It opens with **F12** in the [catalog](04-catalog.md#opening-a-table), on a [foreign key](07-relations.md) or from `.sql` (**F12** on a table name).

```
# zpg v1 conn=local table=zpg_demo.customers pk=id page=1 limit=100 total=8 width=40 apply=manual
| id | email               | name             | city      | vip   | created_at                    | note                                     |
|----|---------------------|------------------|-----------|-------|-------------------------------|------------------------------------------|
| 1  | anna@example.com    | Anna Petrova     | Tashkent  | true  | 2026-10-18 01:40:44.321115+05 | NULL                                     |
| 4  | dilnoza@example.com | Dilnoza Karimova | Samarkand | true  | 2026-10-18 01:40:44.321115+05 | Two lines:\nthe second line              |
| 8  | hiro@example.com    | Hiro Tanaka      | Osaka     | false | 2026-10-18 01:40:44.321115+05 | A very long note about the customer tha… |
```

- **Line 1 is the header**: which rows are shown and how they are written, see [below](#header).
- **Line 2 is the column names.** They can't be changed.
- **Line 3 is the separator.** It aligns itself.
- **Then come the data rows.** Values are in Postgres's text form: `true`/`false`, dates `2026-10-18 01:40:44+05`, JSON as is, arrays `{periphery,usb}`. `NULL` is SQL NULL; a line break inside a value shows as `\n`, see [Special values](06-editing.md#special-values).
- **`…` at the end**: the value is longer than `width` and was cut. Hover or the [row view](07-relations.md#row-view) shows it in full.

## Buttons above the header

Above the first line there are buttons (code lens, needs `"code_lens": "on"`, see [Installation](01-install.md#4-zed-settings)): **Apply changes (…)** when there are edits, **Next page** and **Previous page**, **Refresh**. They run the same actions as `ctrl-.`. The same **Apply changes** and **Refresh row** buttons are above a [row](07-relations.md#row-view), and **Refresh catalog** and **Open settings file** above the [catalog](04-catalog.md).

## Header

The first line controls the page. Change it and save (`ctrl-s`), and the page reloads.

```
# zpg v1 conn=local table=zpg_demo.events pk="" page=1 limit=20 total=10 width=60 apply=manual where="level = 'error'" order="at desc" columns="at, message"
```

| Field | What it does | Example |
|---|---|---|
| `page` | page number, from 1 | `page=3` |
| `limit` | rows per page, up to 10,000 | `limit=500` |
| `where` | SQL filter condition | `where="created_at > now() - interval '1 day'"` |
| `order` | SQL ordering; without it, by the key | `order="created_at desc, id"` |
| `columns` | which columns to show; without it, all | `columns="name, email"` |
| `width` | the width values are cut at, `0` to not cut | `width=0` |
| `apply` | when to write to the database: `manual` or `save` | `apply=save` |
| `conn`, `table`, `pk`, `total` | information only, can't be changed | |

- `where`, `order` and `columns` can be written without quotes: the value runs up to the next header field. `order=id desc` and `where=vip and city = 'Tashkent'` work as they are; on save zpg quotes them itself.
- Other values with spaces go in double quotes, a quote inside is `\"`. SQL's single quotes are written as is.
- To remove a filter or ordering, delete the whole field.
- An error in `where`/`order` comes back as Postgres's message, and the page stays as it was.
- Pages are read in a read-only transaction, so nothing can be changed through `where`.
- If the page has unwritten edits: in `manual` mode a new header isn't applied until you write or discard the edits; in `save` mode the edits are written first, then the new page loads.
- Completion in the header: inside `where="…"`, columns and `and`, `or`, `is null`, `like`…; inside `order="…"`, columns and `asc`, `desc`, `nulls last`; outside values, field names.

### Header examples

```
# last week's orders, newest first
… where="created_at > now() - interval '7 days'" order="created_at desc"

# only VIPs from Tashkent, three columns
… where="vip and city = 'Tashkent'" columns="name, email, city"

# products whose attrs have "wireless": true
… where="attrs @> '{\"wireless\": true}'"

# orders of customers with more than one order
… where="customer_id in (select customer_id from zpg_demo.orders group by 1 having count(*) > 1)"

# show long values in full
… width=0
```

## How many rows in total

`total` in the header is how many rows match `where`:

- on load zpg counts them with `count(*)`; if counting takes over a second, it shows the planner's estimate with `~`, such as `total=~1200000`;
- paging and sorting don't recount; **Refresh** and a new `where` do;
- after a write zpg adds the inserted rows and subtracts the deleted ones; with a `where`, the number becomes an estimate (`~`), since new rows may not match it.

## Paging

`ctrl-.` → **zpg: Next page (2/37)**, **Previous page**, **First page**, **Last page (37/37)**. The title has the page number out of the total (from `total`). *Next* and *Last* are offered only when the current page is full up to `limit`. If the next page turns out empty, you get "no more rows" and the current page stays.

Or by hand: `page=5` in the header and `ctrl-s`.

## Sorting

Cursor on a column name in the header row → `ctrl-.` → **zpg: Sort by created_at ↑** (ascending) or **↓** (descending). The page reloads from the first page, and `order=created_at desc` appears in the header.

## Filtering by a cell's value

Cursor on a cell → `ctrl-.`:

| Cell | Actions |
|---|---|
| `Tashkent` in the `city` column | **zpg: Filter: city = 'Tashkent'**, **zpg: Filter: city <> 'Tashkent'** |
| `NULL` in the `city` column | **zpg: Filter: city IS NULL**, **zpg: Filter: city IS NOT NULL** |

- If `where` is already set, the condition is added with `AND` and the action is called **Also filter: …**. That way you can narrow down step by step: first `city = 'Tashkent'`, then `vip = 'true'`.
- The full value from the database is used, even if the cell shows it cut.
- Values longer than 200 characters and multi-line ones are not offered.
- For `json`, `xml` and geometric types, which have no `=`, the text is compared: `attrs::text = '…'`.
- **zpg: Clear filter (where=)** removes the filter entirely.

## Choosing columns

`columns="name, email"` shows only these columns (plus the key columns, which are always visible):

- columns come in table order, not list order;
- hidden columns aren't touched on write and get their default in new rows;
- if a required column (NOT NULL without a default) is hidden, a new row is marked with the error "a new row needs …: add to columns=";
- `columns` and `width` only redraw the rows already loaded; the database isn't read.

## Wide tables

Zed always wraps lines longer than 512 characters, whatever `soft_wrap` says. So zpg keeps a table row under 500 characters: if the columns don't fit, the widest are cut to the total width (key columns never are). If it still doesn't fit, the header row shows a hint: pick columns with `columns=`.

For `.pgt` it helps to turn wrapping off: `"Pg Table": { "soft_wrap": "none" }` in Zed's settings.

## Highlighting and hover

Numbers, `true`/`false`, `NULL`/`DEFAULT` and cut values are colored. With semantic tokens on ([Installation](01-install.md#4-zed-settings)), changed cells and new rows are underlined before they are written.

**Hover** on a cell shows the column and the value:

```
vip boolean · not null · has default

10% discount on all orders
```

That is the name, the type, the flags (`primary key`, `not null`, `has default`, `generated`), the foreign key (`→ zpg_demo.customers(id)`), the column's comment and, for a key, who references the row (`← referenced by zpg_demo.orders(customer_id)`). For a cut value, the full value, and for some types more than fits in a cell:

| Type | What hover shows |
|---|---|
| `json`, `jsonb` | formatted, highlighted JSON (if the value is cut) |
| `timestamptz` | weekday, "3 hours ago" / "in 2 days", time in UTC and in your time zone |
| `timestamp` | weekday and "how long ago" two ways: if the time was written in your zone and if in UTC |
| `date` | weekday, "today", "tomorrow", "5 days ago" |
| `integer`, `bigint` of 10 or 13 digits | the date, if it is Unix time in seconds or milliseconds (2001–2100) |
| arrays | one element per line: `[1] a`, `[2] b`, `[1][2]` for multi-dimensional ones |
| `xml` | indented and highlighted |
| `bytea` | the size; the text if it is UTF-8, otherwise a hex dump of the first 256 bytes |

Values over 20,000 characters are cut in hover; the [row view](07-relations.md#row-view) shows them in full. Hover on a foreign key cell shows the row it references.

## Structure and backup

`ctrl-.` → **zpg: Show DDL** gives this table's `CREATE TABLE`, **zpg: Back up … (pg_dump)** a backup of its structure and data. Details are in the [catalog](04-catalog.md#table-structure-ddl).

## Refresh

`ctrl-.` → **zpg: Refresh** rereads the page and the table's structure from the database. With unwritten edits the action is called **Refresh (discard changes)** and drops them.

## Read-only

A table opens read-only if it has no primary key (like `zpg_demo.events`), if it is a view or a materialized view (`zpg_demo.paid_orders`), or if its connection has [`read_only`](02-connections.md#read-only). The header `pk=""` marks a table without a key. Such tables can be viewed, filtered, sorted and exported, but not edited.

---
[← Catalog](04-catalog.md) · [Contents](README.md) · [Editing →](06-editing.md)
