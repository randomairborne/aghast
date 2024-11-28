-- Add migration script here
CREATE TABLE ticket_messages (
    dm_channel INT NOT NULL REFERENCES tickets(dm) ON DELETE CASCADE,
    thread_channel INT NOT NULL REFERENCES tickets(thread) ON DELETE CASCADE,
    dm_message INT NOT NULL UNIQUE,
    thread_message INT NOT NULL UNIQUE
) STRICT;

CREATE INDEX ticket_dm_messages ON ticket_messages(dm_message);
CREATE INDEX ticket_thread_messages ON ticket_messages(thread_message);
CREATE INDEX ticket_dm_channels ON ticket_messages(dm_channel);
CREATE INDEX ticket_thread_channels ON ticket_messages(thread_channel);
