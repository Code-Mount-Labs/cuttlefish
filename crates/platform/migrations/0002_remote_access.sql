ALTER TABLE installations ADD COLUMN address_revision BIGINT NOT NULL DEFAULT 0 CHECK(address_revision >= 0);
ALTER TABLE installations ADD COLUMN address_origin TEXT;
