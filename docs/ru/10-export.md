# Экспорт

Данные страницы таблицы или результата запроса можно сохранить в CSV, JSON или набор `INSERT`. Файлы ложатся в `.zpg/<подключение>/exports/` и сразу открываются.

## Что можно выгрузить

`ctrl-.` на странице таблицы (`.pgt`):

| Действие | Что выгружает | Файл |
|---|---|---|
| **zpg: Export page as CSV** | загруженную страницу | `exports/zpg_demo.products.csv` |
| **zpg: Export page as JSON** | загруженную страницу | `exports/zpg_demo.products.json` |
| **zpg: Export page as INSERT statements** | загруженную страницу | `exports/zpg_demo.products.sql` |
| **zpg: Export all rows matching where= as CSV** | **все** строки под `where` шапки, через `COPY` | `exports/zpg_demo.products.csv` |

`ctrl-.` в результате запроса (`.pgres`): **zpg: Export result as CSV / JSON** → `exports/<имя .sql-файла>.csv|.json`.

- Выгружаются **видимые колонки** (с учётом `columns=`; ключевые видны всегда) и **данные как в базе** — без незаписанных правок в буфере.
- «Export page» берёт только загруженную страницу (`limit` строк). Чтобы выгрузить всю таблицу или всю выборку, используйте **Export all rows** — он читает из базы через `COPY` в порядке `order` и не зависит от `limit`. Его можно [отменить](08-sql.md#отмена-долгого-запроса), как и запрос.
- Файл перезаписывается при каждом экспорте.
- Файлы больше 20 МБ сохраняются, но не открываются в редакторе.

## Форматы

На примере `zpg_demo.products` с `where="sku in ('KB-01', 'HD-01')"`:

### CSV

```csv
id,sku,title,price,tags,attrs,active
1,KB-01,Клавиатура,45.00,"{periphery,usb}","{""layout"": ""ru/en"", ""wireless"": false}",true
4,HD-01,Наушники,79.00,{audio},,true
```

- Первая строка — имена колонок.
- Значения в кавычках, только если нужно: есть запятая, кавычка, перевод строки, пробел по краям или значение пустое. Кавычка удваивается.
- NULL — пустое поле (`HD-01,…,,true`), пустая строка — `""`.

### JSON

```json
[
  {"id": 1, "sku": "KB-01", "title": "Клавиатура", "price": 45.00, "tags": "{periphery,usb}", "attrs": {"layout": "ru/en", "wireless": false}, "active": true},
  {"id": 4, "sku": "HD-01", "title": "Наушники", "price": 79.00, "tags": "{audio}", "attrs": null, "active": true}
]
```

- Массив объектов, по объекту на строку.
- Числа (`integer`, `bigint`, `numeric`, `real`…) и `boolean` остаются числами и `true`/`false`; `json`/`jsonb` вставляются как есть; всё остальное — строки (массивы Postgres тоже).
- NULL — `null`. `NaN` и `Infinity` пишутся строкой.

### INSERT

```sql
INSERT INTO zpg_demo.products (id, sku, title, price, tags, attrs, active) VALUES
  ('1', 'KB-01', 'Клавиатура', '45.00', '{periphery,usb}', '{"layout": "ru/en", "wireless": false}', 'true'),
  ('4', 'HD-01', 'Наушники', '79.00', '{audio}', NULL, 'true');
```

- По 500 строк в одном `INSERT`.
- Все значения — строковые литералы: Postgres сам приведёт их к типам колонок при вставке. NULL — `NULL`.
- Удобно для переноса нескольких строк между базами: экспорт со `stage`, файл открыть, добавить `-- @conn local` первой строкой и нажать ▶.
- Только для страниц таблиц (у результата запроса нет таблицы, в которую вставлять).

---
[← Администрирование](09-admin.md) · [Оглавление](README.md) · [Справочник →](11-reference.md)
