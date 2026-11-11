-- A message typed in the game (its overlay), not the launcher: the IN GAME tag beside it.
ALTER TABLE chat_messages ADD COLUMN from_game INTEGER NOT NULL DEFAULT 0;
