-- A deletion travels like a message: a row naming the message it deletes (`deletes`), in the
-- same room and conversation, so servers and players drop it by the same ids they read by.
-- History and unread counts leave these out.
ALTER TABLE chat_messages ADD COLUMN deletes INTEGER;
