# Administration

## Server activity

Who is connected to the server and what they run. In the [catalog](04-catalog.md), cursor on any line of a connection's section (such as `[local]`) → `ctrl-.` → **zpg: Server activity of local**. `.zpg/local/activity.pgres` opens (an illustrative example: three sessions, one waiting for a lock):

```
# zpg v1 result conn=local rows=3 time=3ms
# SELECT pid, usename AS user, datname AS db, application_name AS app,
| pid   | user  | db   | app  | client    | state               | xact_age | query_age | waiting             | blocked_by | query                                       |
|-------|-------|------|------|-----------|---------------------|----------|-----------|---------------------|------------|---------------------------------------------|
| 70412 | app   | shop | api  | 10.0.0.12 | active              | 00:02:41 | 00:02:41  | Lock: transactionid | {70388}    | update zpg_demo.orders set status = 'paid'… |
| 70388 | admin | shop | psql | local     | idle in transaction | 00:05:10 | 00:05:02  | Client: ClientRead  | NULL       | update zpg_demo.orders set comment = 'x'…   |
| 70063 | jamal | shop | psql | local     | active              | 00:00:01 | 00:00:01  | Timeout: PgSleep    | NULL       | select pg_sleep(60)                         |
```

| Column | Meaning |
|---|---|
| `pid` | the session's server process number |
| `user`, `db`, `app` | the user, the database, the client's `application_name` (zpg identifies itself as `zpg`) |
| `client` | the client's address, or `local` for a unix socket |
| `state` | `active`: running a statement; `idle`: waiting for a command; `idle in transaction`: opened a transaction and does nothing (holding locks!) |
| `xact_age`, `query_age` | how long the transaction and the current statement have been running |
| `waiting` | what the session waits for: `Lock: …` a lock, `IO: …` the disk, `Client: ClientRead` the client |
| `blocked_by` | pids of the sessions holding the lock it needs (`pg_blocking_pids`) |
| `query` | the current or last statement, up to 300 characters |

- Client sessions of all the server's databases are shown, except zpg's own session that reads this list.
- Active ones first, the longest transactions at the top, `idle` at the end.
- To refresh: `ctrl-.` → **zpg: Run again**.

### Example: who holds the lock

In the table above, session `70412` waits (`waiting = Lock: transactionid`), and `blocked_by = {70388}` names the culprit: session `70388` has been `idle in transaction` for five minutes, so someone in `psql` started a transaction with an `UPDATE` and forgot to `COMMIT`. The fix is to terminate `70388` (see below) or find its author.

## Databases

In the [catalog](04-catalog.md), on any line of a connection's section: `ctrl-.` → **zpg: Databases of local**. `.zpg/local/databases.pgres` opens, like `\l` in psql; template databases (`template0`, `template1`) aren't shown:

```
| name   | owner | encoding | size   | sessions |
|--------|-------|----------|--------|----------|
| shop   | app   | UTF8     | 41 MB  | 3        |
| stage  | app   | UTF8     | 12 MB  | 0        |
```

- `size` is empty if you have no `CONNECT` privilege on the database.
- `sessions` is how many sessions `pg_stat_activity` shows, zpg's own session included.
- To work with another database, add a connection with the same server and another `dbname`, see [Connections](02-connections.md).
- To refresh: `ctrl-.` → **zpg: Run again**.

## Schemas

In the same place: `ctrl-.` → **zpg: Schemas of local** opens `.zpg/local/schemas.pgres`:

```
| name     | owner | relations | readable |
|----------|-------|-----------|----------|
| public   | app   | 12        | 12       |
| zpg_demo | app   | 0         | 0        |
```

- `relations` counts the schema's tables, views, matviews and foreign tables (partitions aren't counted); `readable` is those of them you have `SELECT` on, that is, the ones visible in the [catalog](04-catalog.md).
- Empty schemas, which the catalog doesn't show, are listed too. System ones (`pg_*`, `information_schema`) are hidden.
- To refresh: `ctrl-.` → **zpg: Run again**.

## Cancelling a query or a session

`ctrl-.` on an activity row:

| Action | What it does |
|---|---|
| **zpg: Cancel the query of pid N** | `pg_cancel_backend(N)`: stops the current statement, the session stays. If the statement was inside a transaction, that transaction can no longer write anything: the client has to `ROLLBACK` |
| **zpg: Terminate backend pid N…** | `pg_terminate_backend(N)`: closes the whole session, an open transaction is rolled back. zpg asks **Run** / **Cancel** first |

After the action the list refreshes. If the session has already ended, you get `zpg: no backend N (it may have ended)`.

These actions are available **in any result with a `pid` column**, so you can write your own query:

```sql
-- long-running statements
select pid, now() - query_start as running, query
from pg_stat_activity
where state = 'active' and now() - query_start > interval '1 minute';

-- who holds locks on a table
select l.pid, l.mode, l.granted, a.query
from pg_locks l join pg_stat_activity a using (pid)
where l.relation = 'zpg_demo.orders'::regclass;
```

and stop any row in the resulting `.pgres`.

**Privileges.** Any user can stop their own sessions. Other users' sessions can be stopped only by a superuser or a role with `pg_signal_backend` (superusers' sessions only by a superuser). Without the privilege the action returns Postgres's error.

## Your own statement and `statement_timeout`

Your own long statement from `.sql` is cancelled without the activity list: `ctrl-.` → **zpg: Cancel the statement running on …**, see [Cancelling](08-sql.md#cancelling-a-long-statement). Besides, every run is limited by `statement_timeout_ms` (60 s by default), see [settings](02-connections.md#all-settings).

---
[← SQL files](08-sql.md) · [Contents](README.md) · [Export →](10-export.md)
