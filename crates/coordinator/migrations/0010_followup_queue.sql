-- Follow-ups are durable immediately, but a session executes one command at a time.
DROP INDEX one_pending_command;
CREATE UNIQUE INDEX one_claimed_command ON dispatches(group_id,thread_id) WHERE state='claimed';
