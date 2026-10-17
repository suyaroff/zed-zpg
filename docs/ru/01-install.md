# Установка

zpg состоит из двух частей:

- **расширение Zed** — маленький модуль, который только находит и запускает сервер;
- **бинарник `zpg`** — language server и CLI, в нём вся работа с базой.

## 1. Бинарник

Соберите из исходников:

```sh
cargo install --path crates/zpg
```

Бинарник попадёт в `~/.cargo/bin/zpg`. Эта папка должна быть в `PATH`, который видит Zed (обычно так и есть, если Zed запускается из вашей сессии).

Если `zpg` в `PATH` нет, расширение попробует скачать готовый бинарник из релизов на GitHub (репозиторий указан в `extension.toml`).

## 2. Расширение

В Zed: `ctrl-shift-p` → **zed: install dev extension** → выберите папку репозитория zpg.

## 3. Расширение SQL

Для работы с `.sql`-файлами нужен язык SQL: `ctrl-shift-p` → **zed: extensions** → найдите **SQL** → Install. Это расширение даёт только подсветку синтаксиса, своего языкового сервера у него нет, поэтому с zpg оно не конфликтует. zpg подключается к языку SQL и добавляет к нему запуск, автодополнение, проверку и остальное.

> Если позже поставите SQL-расширение со своим сервером (например, `sqls`), подсказки обоих серверов смешаются. Оставить только zpg: `"languages": { "SQL": { "language_servers": ["zpg", "!sqls"] } }`.

## 4. Настройки Zed

Добавьте в `~/.config/zed/settings.json`:

```jsonc
{
  // кнопки ▶ Run и Explain над запросами в .sql
  "code_lens": "on",

  "languages": {
    // подчёркивать изменённые ячейки и новые строки до записи в базу
    "Pg Table": { "semantic_tokens": "combined", "soft_wrap": "none" },
    "Pg Row": { "semantic_tokens": "combined" },
    // широкие результаты удобнее без переноса строк
    "Pg Result": { "soft_wrap": "none" }
  }
}
```

## 5. Задача «открыть каталог»

Каталог — точка входа: список подключений и таблиц. Удобнее всего открывать его задачей. Добавьте в `~/.config/zed/tasks.json`:

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

Запуск: `ctrl-shift-p` → **task: spawn** → **zpg: open catalog**. Можно повесить на клавиши в `~/.config/zed/keymap.json`:

```json
[
  {
    "context": "Workspace",
    "bindings": { "ctrl-alt-q": ["task::Spawn", { "task_name": "zpg: open catalog" }] }
  }
]
```

`zpg open [путь]` создаёт `.zpg/catalog.pgdb` в проекте (если его нет) и открывает его командой `zed`. Другой CLI Zed: флаг `--zed` или переменная `ZPG_ZED`.

## Команды CLI

| Команда | Что делает |
|---|---|
| `zpg open [путь]` | создаёт `.zpg/catalog.pgdb` в проекте и открывает его в Zed |
| `zpg config` | создаёт файл настроек (если нужно) и открывает его в Zed |
| `zpg lsp` | language server; его запускает расширение, вручную не нужен |

## После обновления

- Пересобрали бинарник (`cargo install` заново) — перезапустите сервер: `ctrl-shift-p` → **editor: restart language server**.
- Поменяли грамматику или подсветку в репозитории — **zed: rebuild dev extension**.

## Проверка

1. Откройте каталог задачей **zpg: open catalog**. Если подключений ещё нет, в нём будет подсказка, как их добавить → [Подключения](02-connections.md).
2. Лог сервера: `ctrl-shift-p` → **dev: open language server logs** → `zpg`.

---
[← Оглавление](README.md) · [Подключения →](02-connections.md)
