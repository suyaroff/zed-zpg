-- Обход возможностей zpg в .sql-файлах. Нужна демо-схема: docs/examples/demo.sql.
-- Откройте файл в Zed (лучше скопируйте в свой проект) и идите по разделам.
-- Если подключений несколько, раскомментируйте и поправьте строку ниже:
-- -- @conn local
--
-- Подробности: docs/ru/08-sql.md


-- 1. Запуск ---------------------------------------------------------------
-- Над каждым запросом кнопка ▶ Run on … (нужен "code_lens": "on").
-- Результат откроется вкладкой .pgres; повторный ▶ обновит ту же вкладку.

select id, name, city, vip from zpg_demo.customers order by id;

-- Запрос без строк: внизу всплывёт «zpg: UPDATE 1 (2 ms)».
update zpg_demo.customers set city = 'Ташкент' where id = 7;

-- Выделите два запроса ниже → ctrl-. → «Run selection»: одна транзакция.
update zpg_demo.products set price = price + 1 where sku = 'CB-03';
select sku, price from zpg_demo.products where sku = 'CB-03';


-- 2. Редактирование результата ---------------------------------------------
-- Все колонки из одной таблицы и есть её ключ → в .pgres ctrl-. → «Edit rows in table».

select id, status, comment from zpg_demo.orders where status = 'new';


-- 3. Автодополнение, hover, F12 --------------------------------------------
-- Поставьте курсор после «o.» и нажмите ctrl-space: колонки orders.
-- Наведите курсор на customers — колонки и типы; F12 на ней откроет таблицу.

select o.id, c.name, o.total
from zpg_demo.orders o
join zpg_demo.customers c on c.id = o.customer_id
where o.status = 'paid';


-- 4. Проверка запросов (сохраните файл: ctrl-s) ----------------------------
-- Ниже три ошибки — Postgres разбирает запросы, не выполняя их.

select id, emial from zpg_demo.customers;          -- опечатка: подскажет email

select * from zpg_demo.customer;                    -- нет такой таблицы

select name + 1 from zpg_demo.customers;            -- text + integer


-- 5. EXPLAIN ----------------------------------------------------------------
-- Кнопка Explain над запросом — план без выполнения.
-- ctrl-. → «Explain analyze» — выполнит, замерит и откатит.

select c.name, count(*) as orders, sum(o.total) as spent
from zpg_demo.customers c
join zpg_demo.orders o on o.customer_id = c.id
group by c.name
order by spent desc;


-- 6. Опасные запросы --------------------------------------------------------
-- Подчёркнуты жёлтым; при запуске zpg спросит Run / Cancel.
-- После них верните данные: перезапустите docs/examples/demo.sql.

update zpg_demo.products set active = true;         -- без WHERE

delete from zpg_demo.events;                        -- без WHERE

update zpg_demo.products set price = (select 1 where true);   -- WHERE только в подзапросе

truncate zpg_demo.events;


-- 7. Отмена долгого запроса -------------------------------------------------
-- ▶, затем ctrl-. в любом файле → «Cancel the statement running on …».

select pg_sleep(30);


-- 8. Активность сервера -----------------------------------------------------
-- Каталог (.zpg/catalog.pgdb): ctrl-. на строке [подключение] → «Server activity».
-- В любом результате с колонкой pid есть «Cancel the query of pid N» и «Terminate backend…»:

select pid, application_name, state, now() - query_start as running, query
from pg_stat_activity
where backend_type = 'client backend' and pid <> pg_backend_pid();


-- 9. История ------------------------------------------------------------------
-- Всё, что успешно выполнилось, есть в .zpg/history.sql — с кнопкой ▶ у каждой записи.
