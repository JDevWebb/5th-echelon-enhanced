-- 0.5.2: reboots for the system's updates, timed by the coordinator (reboots.rs). An admin's
-- quiet hours ("4-7", UTC; none: learned from play), and "reboot when empty, any hour" until.
ALTER TABLE servers ADD COLUMN quiet_hours TEXT;
ALTER TABLE servers ADD COLUMN reboot_any_hour_until INTEGER;
