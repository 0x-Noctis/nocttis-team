ALTER TABLE tool_call_reservations
    ADD COLUMN tool_name text,
    ADD CONSTRAINT tool_call_reservations_tool_identity CHECK (
        tool_name IS NOT NULL
        AND octet_length(tool_name) BETWEEN 1 AND 255
        AND tool_name !~ '[[:cntrl:]]'
        AND tool_name !~ U&'[\0080-\009F]'
    ) NOT VALID;
