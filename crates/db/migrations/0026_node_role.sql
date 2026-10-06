ALTER TABLE nodes ADD COLUMN role TEXT CHECK (role IN ('dev', 'personal'));
