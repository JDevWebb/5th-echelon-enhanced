-- An admin's role: an owner manages the admins and the admin UI's security settings; an admin
-- does everything else. Every admin from before roles is an owner, so nothing changes on
-- upgrade; new ones are given their role when added (the console adds owners).
ALTER TABLE admins ADD COLUMN role TEXT NOT NULL DEFAULT 'owner' CHECK (role IN ('owner', 'admin'));
