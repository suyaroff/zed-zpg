# Администрирование

## Активность сервера

Кто подключён к серверу и что выполняет. В [каталоге](04-catalog.md) курсор на любой строке секции подключения (например, на `[local]`) → `ctrl-.` → **zpg: Server activity of local**. Откроется `.zpg/local/activity.pgres` (условный пример: три сессии, одна ждёт блокировку):

```
# zpg v1 result conn=local rows=3 time=3ms
# SELECT pid, usename AS user, datname AS db, application_name AS app,
| pid   | user  | db   | app  | client    | state               | xact_age | query_age | waiting             | blocked_by | query                                       |
|-------|-------|------|------|-----------|---------------------|----------|-----------|---------------------|------------|---------------------------------------------|
| 70412 | app   | shop | api  | 10.0.0.12 | active              | 00:02:41 | 00:02:41  | Lock: transactionid | {70388}    | update zpg_demo.orders set status = 'paid'… |
| 70388 | admin | shop | psql | local     | idle in transaction | 00:05:10 | 00:05:02  | Client: ClientRead  | NULL       | update zpg_demo.orders set comment = 'x'…   |
| 70063 | jamal | shop | psql | local     | active              | 00:00:01 | 00:00:01  | Timeout: PgSleep    | NULL       | select pg_sleep(60)                         |
```

| Колонка | Что значит |
|---|---|
| `pid` | номер серверного процесса сессии |
| `user`, `db`, `app` | пользователь, база, `application_name` клиента (zpg представляется как `zpg`) |
| `client` | адрес клиента или `local` для unix-сокета |
| `state` | `active` — выполняет запрос; `idle` — ждёт команды; `idle in transaction` — открыла транзакцию и ничего не делает (держит блокировки!) |
| `xact_age`, `query_age` | сколько идёт транзакция и текущий запрос |
| `waiting` | чего ждёт сессия: `Lock: …` — блокировку, `IO: …` — диск, `Client: ClientRead` — клиента |
| `blocked_by` | pid'ы сессий, которые держат нужную ей блокировку (`pg_blocking_pids`) |
| `query` | текущий или последний запрос, до 300 символов |

- Показываются клиентские сессии всех баз сервера, кроме собственной сессии zpg, которая читает этот список.
- Сначала активные, самые долгие транзакции выше, `idle` — в конце.
- Обновить: `ctrl-.` → **zpg: Run again**.

### Пример: кто держит блокировку

В таблице выше сессия `70412` ждёт (`waiting = Lock: transactionid`), а `blocked_by = {70388}` указывает виновника: сессия `70388` пять минут висит `idle in transaction` — кто-то в `psql` начал транзакцию с `UPDATE` и забыл сделать `COMMIT`. Решение — прервать `70388` (см. ниже) или найти автора.

## Прервать запрос или сессию

`ctrl-.` на строке активности:

| Действие | Что делает |
|---|---|
| **zpg: Cancel the query of pid N** | `pg_cancel_backend(N)`: прерывает текущий запрос, сессия остаётся. Если запрос был внутри транзакции, она уже ничего не запишет — клиенту придётся сделать `ROLLBACK` |
| **zpg: Terminate backend pid N…** | `pg_terminate_backend(N)`: закрывает сессию целиком, открытая транзакция откатывается. zpg сначала спросит **Run** / **Cancel** |

После действия список обновится. Если сессия уже закончилась, придёт `zpg: no backend N (it may have ended)`.

Эти действия есть **в любом результате с колонкой `pid`** — можно написать свой запрос:

```sql
-- долгие запросы
select pid, now() - query_start as running, query
from pg_stat_activity
where state = 'active' and now() - query_start > interval '1 minute';

-- кто держит блокировки на таблице
select l.pid, l.mode, l.granted, a.query
from pg_locks l join pg_stat_activity a using (pid)
where l.relation = 'zpg_demo.orders'::regclass;
```

и в получившемся `.pgres` прервать любую строку.

**Права.** Свои сессии может прерывать любой пользователь. Чужие — только суперпользователь или роль с `pg_signal_backend` (сессии суперпользователей — только суперпользователь). Без прав действие вернёт ошибку Postgres.

## Свой запрос и `statement_timeout`

Свой долгий запрос из `.sql` отменяется без активности: `ctrl-.` → **zpg: Cancel the statement running on …**, см. [Отмена](08-sql.md#отмена-долгого-запроса). Кроме того, каждый запуск ограничен `statement_timeout_ms` (по умолчанию 60 с), см. [настройки](02-connections.md#все-параметры).

---
[← SQL-файлы](08-sql.md) · [Оглавление](README.md) · [Экспорт →](10-export.md)
