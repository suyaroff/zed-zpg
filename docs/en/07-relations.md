# Relations and the row view

## Following a foreign key

**F12** (or `ctrl`+click) on a foreign key cell opens the referenced table, filtered to the row it points to.

In `zpg_demo.orders`, cursor on `customer_id` of the first row → F12:

```
# zpg v1 conn=local table=zpg_demo.customers pk=id page=1 limit=100 total=1 width=40 apply=manual where="id = '1'"
| id | email            | name         | city     | vip  | created_at                    | note |
|----|------------------|--------------|----------|------|-------------------------------|------|
| 1  | anna@example.com | Anna Petrova | Tashkent | true | 2026-10-18 01:40:44.321115+05 | NULL |
```

- To see the whole table again: `ctrl-.` → **zpg: Clear filter**, or delete `where` from the header and save.
- If the referenced table has unwritten edits, it opens as it is, without the filter.
- **Hover** on a foreign key cell shows the row it points to right away, no need to go there:

  ```
  customer_id bigint · not null · → zpg_demo.customers(id)

  → zpg_demo.customers (F12 opens it)
  - id: 1
  - email: anna@example.com
  - name: Anna Petrova
  - city: Tashkent
  …
  ```

Composite foreign keys work the same way: the filter covers every column of the key.

## Who references a row

The other direction: which rows of other tables refer to this one.

- **Hover** on a key cell shows the foreign keys that point to the table:

  ```
  id bigint · primary key · not null · has default

  ← referenced by zpg_demo.orders(customer_id)
  ```

- **`ctrl-.` on a data row** → **zpg: Rows of zpg_demo.orders referencing this (customer_id)** opens `zpg_demo.orders` with `where="customer_id = '1'"`: all the orders of this customer.

  An order shows its items the same way: on a `zpg_demo.orders` row → **Rows of zpg_demo.order_items referencing this (order_id)**.

These actions come at the end of the `ctrl-.` menu, one for each foreign key that points to the table. Popular tables (such as `users`) can have dozens of them.

## Row view

**F12** on a primary key cell (or `ctrl-.` → **zpg: Open row**) opens the row vertically, one field per line of text. The file is `.zpg/local/zpg_demo.customers.rows/4.pgr`.

```
# zpg v1 row conn=local table=zpg_demo.customers key=id=4
id: 4
email: dilnoza@example.com
name: Dilnoza Karimova
city: Samarkand
vip: true
created_at: 2026-10-18 01:40:44.321115+05
note: |
  Two lines:
  the second line
```

For `jsonb` (`zpg_demo.products`, row 3):

```
# zpg v1 row conn=local table=zpg_demo.products key=id=3
id: 3
sku: MN-27
title: Monitor 27"
price: 289.00
tags: {display}
attrs: |
  {
      "hz": 144,
      "size": 27,
      "panel": "IPS"
  }
active: true
```

Rules:

- A value with line breaks is written as a block: `name: |`, then lines indented by two spaces. Add and delete lines inside the block as ordinary text.
- `jsonb` is shown formatted (`jsonb_pretty`); it is sent to the database as is, and Postgres parses it.
- Single-line values are written as in the table: `NULL`, `DEFAULT`, `\n` escapes and so on. `name:` with no value is an empty string.
- Values are not cut: this is the way to edit what the table shows with `…`.
- Key fields can't be changed, nor can the order and set of fields.
- Changed fields are underlined, and the first line shows `pending: 1 update`.
- It is written by the same rules as the table, in the mode of the table the row was opened from: with `manual`, `ctrl-.` → **zpg: Apply changes**; with `save`, `ctrl-s`. After the write the table page refreshes too, unless it has unwritten edits of its own.
- Hover and F12 on a foreign key work here as well, and so does **Rows of … referencing this**.
- F12 on the same key again rereads the row from the database. The exception: if the `.pgr` has unwritten edits, it opens as it is, and the edits aren't lost. To reread by hand: `ctrl-.` → **zpg: Refresh row (discard changes)**.

---
[← Editing](06-editing.md) · [Contents](README.md) · [SQL files →](08-sql.md)
