#!/bin/sh
set -eu

# The application owns its database, without cluster administration privileges.
APODECT_DB_PASSWORD=$(cat /run/secrets/database_password)
export APODECT_DB_PASSWORD
psql --set ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres <<'SQL'
\getenv app_password APODECT_DB_PASSWORD
CREATE ROLE apodect LOGIN PASSWORD :'app_password';
CREATE DATABASE apodect OWNER apodect;
\connect apodect
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
SQL
