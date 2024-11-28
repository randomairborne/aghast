-- Add migration script here
CREATE TABLE tickets (
    dm INT UNIQUE NOT NULL,
    thread INT UNIQUE NOT NULL
) STRICT;

CREATE INDEX ticket_dms ON tickets(dm);
CREATE INDEX ticket_threads ON tickets(thread);
