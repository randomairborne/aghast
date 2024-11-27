-- Add migration script here
CREATE TABLE tickets (
    dm INT8 UNIQUE NOT NULL,
    thread INT8 UNIQUE NOT NULL
);

CREATE INDEX ticket_dms ON tickets(dm);
CREATE INDEX ticket_threads ON tickets(thread);