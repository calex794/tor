-- Bounded reverse inventory lookup of previously reserved author SINGLE IDs.
CREATE INDEX drop_inventory_save_command_version_idx
    ON chapter_save_commands(version_id,chapter_id)
    WHERE version_id IS NOT NULL AND chapter_id IS NOT NULL;
