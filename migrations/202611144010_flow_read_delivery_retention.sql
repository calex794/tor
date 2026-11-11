-- I-reviewed: an optional FLOW consumer receipt is personal while its event
-- points to canonical C history. It shares the bounded C capture lifetime.
-- F locks current subject, then the exact capture rows FOR KEY SHARE before
-- recording deliveries. DELETE waits for that transaction; its trigger then
-- removes the committed FLOW marker atomically on TTL or consent erasure.
CREATE FUNCTION reader_flow_erase_read_delivery() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM event_deliveries
    WHERE consumer='flow-read-attribution-v1' AND event_id=OLD.event_id;
    RETURN NULL;
END
$$;
CREATE TRIGGER reader_flow_erase_captured_read_delivery
    AFTER DELETE ON reader_flow_read_receipts
    FOR EACH ROW EXECUTE FUNCTION reader_flow_erase_read_delivery();

COMMENT ON FUNCTION reader_flow_erase_read_delivery() IS
    'Erase only the F FLOW consumer marker with its C scoped capture; preserve canonical C events and every other consumer.';
