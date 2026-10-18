# SQL-файлы

zpg подключается к любому `.sql`-файлу проекта: запускает запросы, показывает результаты, дополняет имена таблиц и колонок, проверяет запросы до запуска и предупреждает об опасных. Нужны расширение Zed **SQL** и `"code_lens": "on"`, см. [Установка](01-install.md#3-расширение-sql).

Все примеры этой страницы собраны в [`docs/examples/tour.sql`](../examples/tour.sql).

## Запуск

Над каждым запросом — кнопки **▶ Run on local** и **Explain** (code lens):

```sql
-- @conn local
select id, name, city, vip from zpg_demo.customers order by id;

update zpg_demo.customers set city = 'Ташкент' where id = 7;
```

- **Кнопка ▶** выполняет запрос.
- **`ctrl-.` на запросе** → **zpg: Run statement on local** — то же без мыши.
- **Несколько запросов**: выделите их → `ctrl-.` → **zpg: Run selection on local**. Они выполнятся по очереди **в одной транзакции**: если упадёт любой, откатятся все. Так удобно запускать скрипты, например [`demo.sql`](../examples/demo.sql) целиком.
- Запросы разделяются `;`. Точки с запятой внутри строк, `$$`-тел функций и комментариев не считаются. Данные после `COPY … FROM stdin;` в дампах `pg_dump` пропускаются до строки `\.`, а строки, начинающиеся с `\` (мета-команды `psql`: `\restrict`, `\set`, `\connect`…), не считаются запросами — они не проверяются и не выполняются.
- Если нажать ▶ сразу после правки, пока Zed не перерисовал кнопки, zpg сам найдёт нужный запрос: по тексту или по месту, где он начинается. Если не найдёт, ничего не выполнится, и zpg попросит нажать снова.
- В файлах больше 5 МБ (полные дампы) кнопок нет, запускайте через `ctrl-.`.
- Каждый запуск идёт в транзакции с `statement_timeout` ([настройка](02-connections.md#все-параметры) `statement_timeout_ms`). Для подключений с `read_only` транзакция только на чтение.
- `VACUUM`, `CREATE INDEX CONCURRENTLY` и другие команды, которые не работают в транзакции, при запуске поодиночке выполняются без неё.

### Подключение

- Строка `-- @conn имя` задаёт подключение для всех запросов ниже неё, до следующей такой строки. В одном файле можно чередовать базы:

  ```sql
  -- @conn stage
  select count(*) from zpg_demo.orders;

  -- @conn prod
  select count(*) from zpg_demo.orders;
  ```

- Без `-- @conn`: если подключение одно, используется оно; если несколько — над запросом по кнопке на каждое (**▶ local**, **▶ prod**), а автодополнение и проверка берут первое по алфавиту.

## Результаты

**Запрос возвращает строки** (`SELECT`, `… RETURNING`) — результат откроется вкладкой `.zpg/local/results/<файл>.pgres`:

```
# zpg v1 result conn=local rows=8 time=2ms
# select id, name, city, vip from zpg_demo.customers order by id
# Rows of zpg_demo.customers: code action “Edit rows in table” opens them for editing.
| id | name             | city      | vip   |
|----|------------------|-----------|-------|
| 1  | Анна Петрова     | Ташкент   | true  |
| 2  | Bob Smith        | London    | false |
```

- Повторный запуск из того же `.sql` обновляет ту же вкладку.
- Показываются первые `max_result_rows` строк (по умолчанию 1000); остальные не выбираются из базы вовсе, в шапке будет `rows=1000+`.
- Результат только для чтения. Hover показывает тип колонки и полное значение обрезанной ячейки.
- `ctrl-.` → **zpg: Run again** перезапускает запрос.
- `ctrl-.` → **zpg: Export result as CSV / JSON** — см. [Экспорт](10-export.md).
- Если в результате есть колонка `pid` — **Cancel the query of pid N** / **Terminate backend pid N…**, см. [Администрирование](09-admin.md#прервать-запрос-или-сессию).

**Запрос не возвращает строк** (`UPDATE`, `INSERT`, DDL) — внизу всплывёт итог: `zpg: UPDATE 1 (2 ms)`. После `CREATE`/`DROP`/`ALTER` каталог обновится сам.

**Ошибка** — всё откатывается, ошибка подчёркивается в `.sql` в том месте, на которое указал Postgres (например, на имени несуществующей колонки), с подсказкой Postgres.

### Править строки результата

Если все колонки результата взяты из одной таблицы и среди них есть её первичный ключ, `ctrl-.` → **zpg: Edit rows in table** откроет обычную страницу этой таблицы, отфильтрованную вашим запросом:

```sql
select id, status, comment from zpg_demo.orders where status = 'new';
```

→ страница `zpg_demo.orders` с `where="id IN (SELECT id FROM (select id, status, comment from zpg_demo.orders where status = 'new') AS zpg_q)"` и `columns=` из колонок запроса (`id, status, comment`). Дальше правка идёт как обычно, см. [Редактирование](06-editing.md). Строковые литералы в запросе не должны переносить строку — шапка однострочная.

## Автодополнение, hover, F12

zpg знает таблицы и колонки подключения запроса.

- **Автодополнение** (`ctrl-space` или просто печатайте):
  - после `from`, `join`, `update`, `into`, `table` — таблицы и схемы;
  - после `o.`, где `o` — алиас или имя таблицы из запроса, — её колонки;
  - после `zpg_demo.` — таблицы схемы;
  - в остальных местах — колонки таблиц запроса, все таблицы и ключевые слова.

  Таблицы схемы `public` пишутся без схемы.

  ```sql
  select o.|            ← id, customer_id, status, created_at, total, comment
  from zpg_demo.orders o
  join zpg_demo.|       ← customers, events, order_items, orders, paid_orders, products
  ```

- **Hover** на таблице — её колонки с типами и комментариями; на колонке — тип и признаки (`not null`, `has default`) и комментарий.
- **F12** на таблице или колонке открывает страницу таблицы.
- Список таблиц читается из базы при первой подсказке и хранится 5 минут. DDL, выполненный из `.sql`, и **Refresh catalog** обновляют его сразу.

## Проверка запросов

При открытии и сохранении файла zpg отдаёт каждый запрос Postgres на разбор (`PREPARE`), **не выполняя** его, и подчёркивает ошибки:

```sql
select id, emial from zpg_demo.customers;
--         ~~~~~ column "emial" does not exist
--               hint: Perhaps you meant to reference the column "customers.email".

select * from zpg_demo.customer;
--            ~~~~~~~~~~~~~~~~~ relation "zpg_demo.customer" does not exist

select name + 1 from zpg_demo.customers;
--          ~ operator does not exist: text + integer
```

- Проверяются только запросы с понятным подключением: по `-- @conn` или единственному подключению.
- Если выше в файле есть `CREATE`/`ALTER`/`DROP`/`COMMENT`/`DO`, ошибки «does not exist» ниже не показываются: объект может создаваться этим же файлом.
- Параметры `$1` без типа ошибкой не считаются.
- Пока вы правите файл, подчёркивания снимаются; после сохранения проверка повторяется.
- Файлы больше 1 МБ или с более чем 300 запросами не проверяются.
- Выключить: `"check_sql": false` в [настройках](02-connections.md#все-параметры). Учтите, что проверка обращается к базе при каждом открытии и сохранении — для подключения через SSH это поднимет туннель.

## Опасные запросы

Запросы, которые меняют все строки или безвозвратно удаляют данные, подчёркиваются предупреждением:

| Запрос | Предупреждение |
|---|---|
| `update zpg_demo.products set active = true` | UPDATE without WHERE changes every row of zpg_demo.products |
| `delete from zpg_demo.events` | DELETE without WHERE deletes every row of zpg_demo.events |
| `update … set price = (select 1 where true)` | `WHERE` только в подзапросе не считается |
| `truncate zpg_demo.events` | TRUNCATE deletes every row of zpg_demo.events |
| `drop table …`, `drop schema …` | DROP TABLE can't be undone |
| `alter table … drop column …` | ALTER … DROP COLUMN deletes the column with its data |
| `explain analyze delete from zpg_demo.events` | то же, что для самого `DELETE`: `EXPLAIN ANALYZE`, запущенный через ▶, выполняет запрос и фиксирует изменения |
| `with old as (…) delete from zpg_demo.events`, `with d as (delete from zpg_demo.events returning *) select …` | проверяются и тела CTE, и основной запрос |

При запуске такого запроса (▶ или `ctrl-.`) zpg сначала спросит:

> zpg: UPDATE without WHERE changes every row of zpg_demo.products. Run on local?
> **Run** · **Cancel**

Без **Run** ничего не выполняется. На подключениях с `read_only` вопроса нет: запись там всё равно не пройдёт.

## EXPLAIN

- Кнопка **Explain** над `SELECT`/`INSERT`/`UPDATE`/`DELETE`/`WITH`/`VALUES`/`MERGE` выполняет `EXPLAIN` и открывает план текстом в `.zpg/local/results/<файл>.plan.txt`. Сам запрос не выполняется.
- `ctrl-.` → **zpg: Explain analyze statement on local** выполняет `EXPLAIN (ANALYZE, BUFFERS)`: запрос **действительно выполняется**, чтобы измерить время, но в транзакции, которая затем откатывается, так что `UPDATE`/`DELETE` ничего не меняют. Выполнение может быть долгим, последовательности (`nextval`) при откате не возвращаются.
- Если подключений несколько и нет `-- @conn`, кнопки нет — используйте `ctrl-.`.

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

## Отмена долгого запроса

Пока запрос выполняется, в строке состояния Zed крутится **zpg: Running on local**. Отменить:

- `ctrl-.` в любом файле → **zpg: Cancel the statement running on local**;
- или `ctrl-shift-p` → **editor: cancel language server work**.

Postgres получает запрос на отмену, транзакция откатывается, внизу — `zpg: cancelled (rolled back)`. Так же отменяются **Run again**, **Explain** и **Export all rows … as CSV**.

```sql
select pg_sleep(30);   -- попробуйте: ▶, затем ctrl-. → Cancel
```

Пока запрос идёт, остальные функции zpg (hover, автодополнение, кнопки) ждут его конца. Прервать чужой запрос — через [активность сервера](09-admin.md).

## История

Каждый успешный запуск (▶, `ctrl-.`, **Run again**) дописывается в `.zpg/history.sql`:

```sql
-- zpg: statements run from .sql files, newest last; ▶ runs one again.

-- 2026-10-18 01:45:12 · SELECT 8 · 2 ms
-- @conn local
select id, name, city, vip from zpg_demo.customers order by id;

-- 2026-10-18 01:45:20 · UPDATE 1 · 1 ms
-- @conn local
update zpg_demo.customers set city = 'Ташкент' where id = 7;
```

- Над каждой записью есть ▶ — запрос выполнится снова на том же подключении.
- Файл держится в пределах 1 МБ: при переполнении удаляется старая половина записей.
- EXPLAIN в историю не попадает.

---
[← Связи и просмотр строки](07-relations.md) · [Оглавление](README.md) · [Администрирование →](09-admin.md)
