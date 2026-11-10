# Installation

zpg has two parts:

- **the Zed extension**: a small module that only finds and starts the server;
- **the `zpg` binary**: the language server and CLI, which does all the database work.

## 1. Binary

Build it from source:

```sh
cargo install --path crates/zpg
```

The binary lands in `~/.cargo/bin/zpg`. That folder has to be in the `PATH` Zed sees (it usually is when Zed is started from your session).

Without `zpg` in `PATH`, the extension downloads a prebuilt binary from the GitHub releases (the repository is set in `extension.toml`).

## 2. Extension

In Zed: `ctrl-shift-p` → **zed: install dev extension** → pick the folder of the zpg repository.

## 3. SQL extension

`.sql` files need the SQL language: `ctrl-shift-p` → **zed: extensions** → find **SQL** → Install. That extension only provides syntax highlighting and has no language server of its own, so it doesn't conflict with zpg. zpg attaches to the SQL language and adds running, completion, checking and the rest.

> If you later install an SQL extension with its own server (such as `sqls`), the suggestions of both servers get mixed. To keep only zpg: `"languages": { "SQL": { "language_servers": ["zpg", "!sqls"] } }`.

## 4. Zed settings

Add to `~/.config/zed/settings.json`:

```jsonc
{
  // ▶ Run and Explain buttons above statements in .sql
  "code_lens": "on",

  "languages": {
    // underline changed cells and new rows before they are written
    "Pg Table": { "semantic_tokens": "combined", "soft_wrap": "none" },
    "Pg Row": { "semantic_tokens": "combined" },
    // wide results read better without wrapping
    "Pg Result": { "soft_wrap": "none" },
    // the file's statements in the Outline panel and ctrl-shift-o
    "SQL": { "document_symbols": "on" }
  }
}
```

## 5. A task to open the catalog

The catalog is the entry point: the list of connections and tables. A task is the easiest way to open it. Add to `~/.config/zed/tasks.json`:

```json
[
  {
    "label": "zpg: open catalog",
    "command": "zpg",
    "args": ["open", "$ZED_WORKTREE_ROOT"],
    "reveal": "never",
    "hide": "on_success"
  }
]
```

Run it: `ctrl-shift-p` → **task: spawn** → **zpg: open catalog**. You can bind it to keys in `~/.config/zed/keymap.json`:

```json
[
  {
    "context": "Workspace",
    "bindings": { "ctrl-alt-q": ["task::Spawn", { "task_name": "zpg: open catalog" }] }
  }
]
```

`zpg open [path]` creates `.zpg/catalog.pgdb` in the project (if it isn't there) and opens it with the `zed` command. For another Zed CLI use the `--zed` flag or the `ZPG_ZED` variable.

## CLI commands

| Command | What it does |
|---|---|
| `zpg open [path]` | creates `.zpg/catalog.pgdb` in the project and opens it in Zed |
| `zpg config` | creates the settings file (if needed) and opens it in Zed |
| `zpg lsp` | the language server; the extension starts it, you don't need it by hand |

## After an update

- Rebuilt the binary (`cargo install` again): restart the server with `ctrl-shift-p` → **editor: restart language server**.
- Changed the grammar or highlighting in the repository: **zed: rebuild dev extension**.

## Checking it works

1. Open the catalog with the **zpg: open catalog** task. With no connections yet, it shows how to add them → [Connections](02-connections.md).
2. The server log: `ctrl-shift-p` → **dev: open language server logs** → `zpg`.

---
[← Contents](README.md) · [Connections →](02-connections.md)
