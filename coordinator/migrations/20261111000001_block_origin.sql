-- The server that set each block (lib.rs, the "block" change): a block is lifted only there,
-- or on a server the blocking player is online on. Blocks from before have none: lifted only
-- where the blocking player is online.
ALTER TABLE blocks ADD COLUMN origin TEXT;
