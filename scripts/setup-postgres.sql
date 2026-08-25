-- Nightlio: one-time setup for a bring-your-own PostgreSQL server.
--
-- Run as a superuser (or any role with CREATEROLE + CREATEDB), for example:
--   psql -h your-db-host -U postgres -f scripts/setup-postgres.sql
--
-- Change the password below before running, then point the api at it:
--   DATABASE_URL=postgres://nightlio:change-this-password@your-db-host:5432/nightlio?sslmode=require
--
-- Requirements & scope:
--   * PostgreSQL 16 or newer (the schema uses pg_input_is_valid).
--   * This script only creates the role and an empty database owned by it.
--     The api creates everything else itself on first boot — all tables,
--     functions, and indexes (migration 0001_baseline) plus the default
--     self-host user and tag groups. Nothing to run by hand.

CREATE ROLE nightlio LOGIN PASSWORD 'change-this-password';
CREATE DATABASE nightlio OWNER nightlio;

-- Alternative: reusing an EXISTING database instead of a dedicated one.
-- Skip the CREATE DATABASE above and grant the role the right to create
-- objects in that database's public schema. On PostgreSQL 15+ this grant is
-- required — ordinary roles lost the implicit CREATE on public — e.g.:
--
--   GRANT CONNECT ON DATABASE shared_db TO nightlio;
--   \connect shared_db
--   GRANT CREATE ON SCHEMA public TO nightlio;
--
-- A dedicated database (the default above) is strongly recommended: the
-- migrate-to-postgres import refuses to run unless the target is empty, and
-- backups/resets stay a simple dropdb/createdb away.
