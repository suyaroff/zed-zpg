# Quick start

Five minutes from an empty project to the first edit in the database. This assumes zpg is [installed](01-install.md).

## Demo schema

To keep your own data safe, start with the demo schema `zpg_demo`; every example in this guide uses it:

| Table | What's in it | Why it's interesting |
|---|---|---|
| `customers` | 8 customers | `identity` key, `boolean`, a multi-line note, a long note (cut) |
| `products` | 6 products | `tags` array, `jsonb attrs`, `CHECK (price >= 0)` |
| `orders` | 9 orders | foreign key to `customers`, status `enum`, `total` kept by a trigger |
| `order_items` | 14 items | composite key `(order_id, product_id)`, generated column `amount` |
| `events` | 40 events | no primary key: read-only |
| `paid_orders` | view | read-only |

To create it, open [`docs/examples/demo.sql`](../examples/demo.sql) in Zed, `ctrl-a` → `ctrl-.` → **zpg: Run selection on …** (zpg asks for confirmation before `DROP SCHEMA`: that's the demo schema being recreated). Or from a terminal: `psql -d shop -f docs/examples/demo.sql`.

## 1. Connection

Run `zpg config` (or open `~/.config/zpg/config.json`) and add the database:

```jsonc
{
  "connections": {
    "local": "postgresql:///shop?host=/var/run/postgresql",
  },
}
```

## 2. Catalog

`ctrl-shift-p` → **task: spawn** → **zpg: open catalog**. `.zpg/catalog.pgdb` opens:

```
# zpg v1 catalog
# Go to definition (F12) on a table opens it; code actions refresh.

[local]
zpg_demo.customers    table       ~8 rows
zpg_demo.events       table       ~40 rows
zpg_demo.order_items  table       ~14 rows
zpg_demo.orders       table       ~9 rows
zpg_demo.paid_orders  view
zpg_demo.products     table       ~6 rows
```

Hover `zpg_demo.orders` to see its columns, size and indexes.

## 3. Table

Cursor on `zpg_demo.customers` → **F12**:

```
# zpg v1 conn=local table=zpg_demo.customers pk=id page=1 limit=100 total=8 width=40 apply=manual
| id | email               | name             | city      | vip   | created_at                    | note                                     |
|----|---------------------|------------------|-----------|-------|-------------------------------|------------------------------------------|
| 1  | anna@example.com    | Anna Petrova     | Tashkent  | true  | 2026-10-18 01:40:44.321115+05 | NULL                                     |
| 2  | bob@example.com     | Bob Smith        | London    | false | 2026-10-18 01:40:44.321115+05 | Prefers morning delivery                 |
| 7  | gina@example.com    | Gina Rossi       | NULL      | false | 2026-10-18 01:40:44.321115+05 | NULL                                     |
| 8  | hiro@example.com    | Hiro Tanaka      | Osaka     | false | 2026-10-18 01:40:44.321115+05 | A very long note about the customer tha… |
```

## 4. Edit

1. In row `7`, replace `NULL` in the `city` column with `Milano`.
2. `ctrl-s`. The first line shows the hint `pending: 1 update; run the code action “Apply changes”`, and the cell is underlined (with semantic tokens on).
3. `ctrl-.` → **zpg: Apply changes (1 update)**. At the bottom: `zpg: saved 1 update`.

Add a row: cursor on any row → `ctrl-.` → **zpg: Insert row**. A template appears:

```
| + |  |  | NULL | DEFAULT | DEFAULT | NULL |
```

`+` means the key comes from the identity, `DEFAULT` the column's default, `NULL` is for optional columns, and the required ones (`email`, `name`) are left empty. Fill them in, `ctrl-s`, **Apply changes**, and the row gets `id` 9.

## 5. Relations

Open `zpg_demo.orders` and put the cursor on `customer_id` in the first row:

- **hover** shows the customer the order refers to;
- **F12** opens `customers` filtered to that customer.

## 6. A statement

Create `report.sql` in the project:

```sql
select c.name, count(*) as orders, sum(o.total) as spent
from zpg_demo.customers c
join zpg_demo.orders o on o.customer_id = c.id
group by c.name
order by spent desc;
```

**▶ Run on local** and **Explain** appear above the statement. Press ▶ and the result opens as a `report.pgres` tab. Try completion: after `o.` press `ctrl-space`.

## 7. Export

In `report.pgres`: `ctrl-.` → **zpg: Export result as CSV**. The file appears in `.zpg/local/exports/report.csv`.

## Next

- Every `.sql` feature step by step: [`docs/examples/tour.sql`](../examples/tour.sql).
- [Table page](05-table.md): filters, sorting, pages.
- [Editing](06-editing.md): special values, write modes, protection against concurrent changes.

---
[← Connections](02-connections.md) · [Contents](README.md) · [Catalog →](04-catalog.md)
