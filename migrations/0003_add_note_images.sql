ALTER TABLE notes ADD COLUMN image_type TEXT
    CHECK (image_type IS NULL OR image_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp'));
