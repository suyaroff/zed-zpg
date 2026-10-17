# Быстрый старт

Пять минут от пустого проекта до первой правки в базе. Предполагается, что zpg [установлен](01-install.md).

## Демо-схема

Чтобы ничего не испортить в своих данных, начните с демо-схемы `zpg_demo` — на ней построены все примеры руководства:

| Таблица | Что в ней | Чем интересна |
|---|---|---|
| `customers` | 8 покупателей | `identity`-ключ, `boolean`, многострочная заметка, длинная заметка (обрезается) |
| `products` | 6 товаров | массив `tags`, `jsonb attrs`, `CHECK (price >= 0)` |
| `orders` | 9 заказов | внешний ключ на `customers`, `enum` статуса, `total` считает триггер |
| `order_items` | 14 позиций | составной ключ `(order_id, product_id)`, generated-колонка `amount` |
| `events` | 40 событий | нет первичного ключа — только чтение |
| `paid_orders` | view | только чтение |

Создать: откройте [`docs/examples/demo.sql`](../examples/demo.sql) в Zed, `ctrl-a` → `ctrl-.` → **zpg: Run selection on …** (zpg спросит подтверждение перед `DROP SCHEMA` — это пересоздание самой демо-схемы). Или из терминала: `psql -d shop -f docs/examples/demo.sql`.

## 1. Подключение

`zpg config` (или откройте `~/.config/zpg/config.json`) и пропишите базу:

```jsonc
{
  "connections": {
    "local": "postgresql:///shop?host=/var/run/postgresql",
  },
}
```

## 2. Каталог

`ctrl-shift-p` → **task: spawn** → **zpg: open catalog**. Откроется `.zpg/catalog.pgdb`:

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

Наведите курсор на `zpg_demo.orders` — увидите колонки, размер и индексы.

## 3. Таблица

Курсор на `zpg_demo.customers` → **F12**:

```
# zpg v1 conn=local table=zpg_demo.customers pk=id page=1 limit=100 total=8 width=40 apply=manual
| id | email               | name             | city      | vip   | created_at                    | note                                     |
|----|---------------------|------------------|-----------|-------|-------------------------------|------------------------------------------|
| 1  | anna@example.com    | Анна Петрова     | Ташкент   | true  | 2026-10-18 01:40:44.321115+05 | NULL                                     |
| 2  | bob@example.com     | Bob Smith        | London    | false | 2026-10-18 01:40:44.321115+05 | Предпочитает доставку утром              |
| 7  | gina@example.com    | Gina Rossi       | NULL      | false | 2026-10-18 01:40:44.321115+05 | NULL                                     |
| 8  | hiro@example.com    | Hiro Tanaka      | Osaka     | false | 2026-10-18 01:40:44.321115+05 | Очень длинная заметка о клиенте, котора… |
```

## 4. Правка

1. В строке `7` замените `NULL` в колонке `city` на `Milano`.
2. `ctrl-s`. На первой строке появится подсказка `pending: 1 update; run the code action “Apply changes”`, а ячейка подчеркнётся (если включены semantic tokens).
3. `ctrl-.` → **zpg: Apply changes (1 update)**. Внизу: `zpg: saved 1 update`.

Добавьте строку: курсор на любую строку → `ctrl-.` → **zpg: Insert row**. Появится заготовка:

```
| + |  |  | NULL | DEFAULT | DEFAULT | NULL |
```

`+` — ключ возьмётся из identity, `DEFAULT` — значение по умолчанию, `NULL` — для необязательных колонок, а обязательные (`email`, `name`) оставлены пустыми. Впишите их, `ctrl-s`, **Apply changes** — строка получит `id` 9.

## 5. Связи

Откройте `zpg_demo.orders`, поставьте курсор на `customer_id` в первой строке:

- **hover** покажет покупателя, на которого ссылается заказ;
- **F12** откроет `customers`, отфильтрованных до этого покупателя.

## 6. Запрос

Создайте в проекте `report.sql`:

```sql
select c.name, count(*) as orders, sum(o.total) as spent
from zpg_demo.customers c
join zpg_demo.orders o on o.customer_id = c.id
group by c.name
order by spent desc;
```

Над запросом появятся **▶ Run on local** и **Explain**. Нажмите ▶ — результат откроется вкладкой `report.pgres`. Попробуйте автодополнение: после `o.` нажмите `ctrl-space`.

## 7. Экспорт

В `report.pgres`: `ctrl-.` → **zpg: Export result as CSV**. Файл появится в `.zpg/local/exports/report.csv`.

## Дальше

- Все возможности `.sql` по шагам — [`docs/examples/tour.sql`](../examples/tour.sql).
- [Страница таблицы](05-table.md): фильтры, сортировка, страницы.
- [Редактирование](06-editing.md): специальные значения, режимы записи, защита от чужих изменений.

---
[← Подключения](02-connections.md) · [Оглавление](README.md) · [Каталог →](04-catalog.md)
