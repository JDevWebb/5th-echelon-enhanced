-- Whether each member server is where its listing says (host_check.rs): the last check's
-- result, as JSON ({target, ok, at, why}). A listing is in the directory only once it passed.
ALTER TABLE servers ADD COLUMN host_check TEXT;
