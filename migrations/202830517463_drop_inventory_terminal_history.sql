-- DROP-07f: completed/failed audit results are append-only evidence.
CREATE FUNCTION reject_closed_free_inventory_cycle_change()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status <> 'running'
       OR (NEW.id, NEW.bucket, NEW.direction, NEW.namespace, NEW.created_at)
          IS DISTINCT FROM
          (OLD.id, OLD.bucket, OLD.direction, OLD.namespace, OLD.created_at) THEN
        RAISE EXCEPTION 'DROP: inventory history cannot be modified'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER drop_inventory_terminal_history_immutable
    BEFORE UPDATE ON free_storage_inventory_cycles
    FOR EACH ROW EXECUTE FUNCTION reject_closed_free_inventory_cycle_change();

CREATE INDEX drop_inventory_recent_completed
    ON free_storage_inventory_cycles(bucket,direction,namespace,id DESC)
    WHERE status='completed';
