# Connections and settings

All zpg settings live in one user file, `~/.config/zpg/config.json`. It is shared by all projects and sits outside them, so passwords don't end up in git. zpg doesn't read Zed's settings (`lsp.zpg.settings`, `.zed/settings.json`).

## Settings file

The file is created on zpg's first start, with examples in comments. To open it:

- in the catalog: `ctrl-.` → **zpg: Open settings file**;
- from a terminal: `zpg config`.

Example:

```jsonc
{
  "connections": {
    // local database over a unix socket
    "local": "postgresql:///shop?host=/var/run/postgresql",
    // test server over the network
    "stage": "postgresql://app:secret@stage-db:5432/shop",
    // production: read-only, over SSH
    "prod": {
      "url": "postgresql:///shop?host=/var/run/postgresql",
      "ssh": "deploy@prod.example.com",
      "read_only": true
    },
  },
  "page_size": 100,
  "apply": "manual",
}
```

- Comments `//` and `/* */` and a comma after the last element are allowed, as in Zed's settings.
- Changes take effect on zpg's next action, nothing needs restarting. An open catalog refreshes by itself.
- If the file has an error, zpg shows it in a message and keeps working with the previous settings.
- The file's permissions are `600` (only the owner can read it).
- Another path is set with the `ZPG_CONFIG` variable. By default `XDG_CONFIG_HOME` is respected; on Windows the file is `%APPDATA%\zpg\config.json`.

## All settings

| Setting | Default | What it does |
|---|---|---|
| `connections` | — | connection name → connection string or object (see below) |
| `page_size` | `100` | how many rows to load per table page (1–10,000) |
| `max_cell_width` | `40` | the width after which a cell value is cut to `…` |
| `apply` | `"manual"` | write mode for newly opened tables: `manual` or `save`, see [Editing](06-editing.md#when-changes-go-to-the-database) |
| `max_result_rows` | `1000` | how many result rows to show when running SQL (1–100,000) |
| `statement_timeout_ms` | `60000` | timeout of one SQL run in milliseconds, `0` for none |
| `check_sql` | `true` | check statements in `.sql` on open and save, see [Checking statements](08-sql.md#checking-statements) |
| `pg_dump` | `"pg_dump"` | the program for [table backups](04-catalog.md#table-backup-pg_dump), such as `"/usr/lib/postgresql/16/bin/pg_dump"` |
| `pg_dump_args` | `[]` | extra `pg_dump` arguments, such as `["--column-inserts"]` |

A connection is a string or an object:

| Object field | What it does |
|---|---|
| `url` | the connection string |
| `enabled` | `false`: the connection is off but stays in the file, see [below](#disabling-a-connection); `true` by default |
| `read_only` | `true`: read-only, see [below](#read-only) |
| `ssh` | `user@host` or a name from `~/.ssh/config`: connect through an SSH tunnel |
| `ssh_options` | extra `ssh` arguments, such as `["-p", "2222"]` |

## Connection string

Both libpq formats work:

| Case | Example |
|---|---|
| local over a unix socket | `postgresql:///shop?host=/var/run/postgresql` |
| the same, key=value format | `host=/var/run/postgresql dbname=shop` |
| over the network with a password | `postgresql://app:secret@db.example.com:5432/shop` |
| without TLS | `postgresql://app@db.example.com/shop?sslmode=disable` |
| with certificate verification | `postgresql://app@db.example.com/shop?sslmode=verify-full` |

- **The user** defaults to `$USER`.
- **The password** can go right into the URL: the settings file is outside the projects. To keep it in an environment variable, use `${VAR}`: the value comes from the project's environment (direnv or the environment Zed was started from). If the variable isn't set, you get the error `environment variable X is not set`.

  ```json
  "stage": "postgresql://app:${STAGE_PG_PASSWORD}@stage-db/shop"
  ```

- **TLS**: see [below](#tls-and-certificate-verification).
- zpg identifies itself to the server as `application_name=zpg`, so its sessions are visible in [server activity](09-admin.md).

## TLS and certificate verification

`sslmode` works as in libpq:

| `sslmode` | Encryption | Certificate check |
|---|---|---|
| `disable` | no | — |
| `prefer` (default) | if the server supports it | no |
| `require` | always | no |
| `verify-ca` | always | signed by a trusted CA |
| `verify-full` | always | signed by a trusted CA and issued for the host in the URL |

Without verification the connection is encrypted, but a spoofed server (MITM) goes unnoticed. For databases on a network you don't control use `verify-full`; to turn verification off, go back to `require`.

Trusted root certificates:

- **`sslrootcert=/path/ca.pem`**: a PEM file with the server's CA (`~/…` works); `sslrootcert=system` means the system store;
- otherwise **`~/.postgresql/root.crt`**, as in libpq;
- otherwise **the system store**. That suits cloud databases with a certificate from a public CA; for your own or a cloud provider's CA (such as Amazon RDS) download it and set it in `sslrootcert`.

```json
"prod": "postgresql://app@db.example.com/shop?sslmode=verify-full&sslrootcert=~/certs/prod-ca.pem"
```

- **Over SSH**, the certificate's name is checked against the host in `url` (the one the server sees), not against the local end of the tunnel.
- **A host given by IP or another name.** `verify-full` checks the name written in `url`. If you connect by address and the certificate is issued for a name, give both: `host=db.example.com hostaddr=10.0.0.5`.
- `pg_dump` gets the same settings for a [backup](04-catalog.md#table-backup-pg_dump) (`PGSSLMODE`, `PGSSLROOTCERT`).
- Client certificates (`sslcert`, `sslkey`) are not supported.

## Disabling a connection

```json
"old": { "url": "postgresql://app@old-db/shop", "enabled": false }
```

With `enabled: false` the connection stays in the file, but zpg leaves it alone:

- the catalog doesn't show its section and doesn't try to connect;
- in `.sql` without `-- @conn` above a statement there are no buttons for this connection;
- referring to it explicitly (`-- @conn old`, a table opened earlier) gives the error ``connection `old` is disabled``, and no connection is opened.

Only an object connection has the `enabled` field; a string `"name": "postgresql://…"` can't be disabled this way: turn it into an object with `url`.

## Read-only

```json
"prod": { "url": "postgresql://app@prod-db/shop", "read_only": true }
```

With `read_only: true`:

- every table of the connection opens read-only: edits are marked as errors and are not written;
- every transaction is opened read-only (`default_transaction_read_only=on`), so statements from `.sql` can't change anything either: Postgres rejects `UPDATE`, `DELETE`, DDL;
- zpg doesn't ask before [dangerous statements](08-sql.md#dangerous-statements), since they wouldn't go through anyway.

Turn it on for production databases.

## Over SSH

If the database is reachable only from a server, add `ssh`. zpg opens a tunnel through the system `ssh` and closes it on exit:

```json
"prod": {
  "url": "postgresql:///shop?host=/var/run/postgresql",
  "ssh": "deploy@prod.example.com",
  "read_only": true
},
"stage": {
  "url": "postgresql://app:${STAGE_PG_PASSWORD}@10.0.0.5:5432/shop",
  "ssh": "stage-bastion",
  "ssh_options": ["-p", "2222"]
}
```

- **`url`** describes the database as the server sees it. `host=/var/run/postgresql` is Postgres's unix socket on the server: peer authentication as the SSH user then works, no database password needed. `10.0.0.5:5432` is a database on the server's internal network.
- **`ssh`** is `user@host` or a host name from `~/.ssh/config`. Keys, ssh-agent, `ProxyJump` through a bastion and the port come from there too.
- **Key or agent only.** zpg can't ask for an SSH password. The server must be in `known_hosts`: log in once with `ssh deploy@prod.example.com` from a terminal.
- **The tunnel went down** (network, laptop sleep): the next request brings it up again.
- **ssh errors** (`Permission denied`, `Host key verification failed`) show in the catalog under the connection name.

## Dropped connections

Postgres may close an idle connection: a server restart, `idle_session_timeout`, `pg_terminate_backend`, laptop sleep. zpg notices on the next request:

- **Reading** (the catalog, a table page, a row, completion, DDL) reconnects and retries by itself, with no error.
- **Writing edits and running SQL** are not retried: if the connection broke in the middle of `COMMIT`, a retry could run them twice. So before them zpg checks the connection with an empty query and reconnects if needed. If the connection breaks while the statement runs, you get an error: run it again.

## Several connections

- The catalog shows every connection, each in its own `[name]` section.
- In `.sql` files a `-- @conn name` line picks the connection; without it, with several connections there is a button for each one above a statement, see [SQL files](08-sql.md#connection).

---
[← Installation](01-install.md) · [Contents](README.md) · [Quick start →](03-quickstart.md)
