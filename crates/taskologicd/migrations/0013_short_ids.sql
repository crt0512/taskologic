-- Boards, templates and programs get the six character short id tasks have
-- had all along, so a barcode can name them and the id survives an export
-- and import. Filled in by the daemon on the next start (SQLite has no
-- base36), unique once filled.
ALTER TABLE boards ADD COLUMN short_id TEXT;
ALTER TABLE templates ADD COLUMN short_id TEXT;
ALTER TABLE programs ADD COLUMN short_id TEXT;
CREATE UNIQUE INDEX boards_short ON boards(short_id) WHERE short_id IS NOT NULL;
CREATE UNIQUE INDEX templates_short ON templates(short_id) WHERE short_id IS NOT NULL;
CREATE UNIQUE INDEX programs_short ON programs(short_id) WHERE short_id IS NOT NULL;
