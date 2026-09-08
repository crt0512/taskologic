-- What a board is for, shown on its card.
ALTER TABLE boards ADD COLUMN description TEXT NOT NULL DEFAULT '';
