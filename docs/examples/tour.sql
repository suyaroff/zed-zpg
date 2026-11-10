-- A walk through zpg's features in .sql files. Needs the demo schema: docs/examples/demo.sql.
-- Open this file in Zed (better: copy it into your project) and go section by section.
-- With several connections, uncomment and adjust the line below:
-- -- @conn local
--
-- Details: docs/en/08-sql.md


-- 1. Running ----------------------------------------------------------------
-- Every statement has a ▶ Run on … button above it (needs "code_lens": "on").
-- The result opens as a .pgres tab; running again updates the same tab.

select id, name, city, vip from zpg_demo.customers order by id;

-- A statement without rows: "zpg: UPDATE 1 (2 ms)" pops up at the bottom.
update zpg_demo.customers set city = 'Tashkent' where id = 7;

-- Select the two statements below → ctrl-. → "Run selection": one transaction.
update zpg_demo.products set price = price + 1 where sku = 'CB-03';
select sku, price from zpg_demo.products where sku = 'CB-03';

-- Values of $1, $2 are given by lines right above the statement and apply to it only.
-- @param $1 = 'paid'
-- @param $2 = 50
select id, status, total from zpg_demo.orders where status::text = $1 and total > $2;


-- 2. Editing a result -------------------------------------------------------
-- All columns come from one table and include its key → in the .pgres ctrl-. → "Edit rows in table".

select id, status, comment from zpg_demo.orders where status = 'new';


-- 3. Completion, hover, F12 -------------------------------------------------
-- Put the cursor after "o." and press ctrl-space: the columns of orders.
-- Hover customers to see its columns and types; F12 on it opens the table.

select o.id, c.name, o.total
from zpg_demo.orders o
join zpg_demo.customers c on c.id = o.customer_id
where o.status = 'paid';


-- 4. Checking statements (save the file: ctrl-s) -----------------------------
-- Three errors below: Postgres parses the statements without running them.

select id, emial from zpg_demo.customers;          -- typo: suggests email

select * from zpg_demo.customer;                    -- no such table

select name + 1 from zpg_demo.customers;            -- text + integer


-- 5. EXPLAIN ----------------------------------------------------------------
-- The Explain button above a statement shows the plan without running it.
-- ctrl-. → "Explain analyze" runs it, measures and rolls back.

select c.name, count(*) as orders, sum(o.total) as spent
from zpg_demo.customers c
join zpg_demo.orders o on o.customer_id = c.id
group by c.name
order by spent desc;


-- 6. Dangerous statements ---------------------------------------------------
-- Underlined in yellow; when run, zpg asks Run / Cancel.
-- Afterwards reset the data: run docs/examples/demo.sql again.

update zpg_demo.products set active = true;         -- no WHERE

delete from zpg_demo.events;                        -- no WHERE

update zpg_demo.products set price = (select 1 where true);   -- WHERE only in a subquery

truncate zpg_demo.events;


-- 7. Cancelling a long statement ----------------------------------------------
-- ▶, then ctrl-. in any file → "Cancel the statement running on …".

select pg_sleep(30);


-- 8. Server activity --------------------------------------------------------
-- Catalog (.zpg/catalog.pgdb): ctrl-. on a [connection] line → "Server activity".
-- Any result with a pid column has "Cancel the query of pid N" and "Terminate backend…":

select pid, application_name, state, now() - query_start as running, query
from pg_stat_activity
where backend_type = 'client backend' and pid <> pg_backend_pid();


-- 9. History ----------------------------------------------------------------
-- Everything that ran successfully is in .zpg/history.sql, with a ▶ button on each entry.
