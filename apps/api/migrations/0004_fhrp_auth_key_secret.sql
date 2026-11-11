-- Plaintext conversion is an explicit offline operation, never a startup fallback.
LOCK TABLE ipam_fhrpgroup IN ACCESS EXCLUSIVE MODE;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM ipam_fhrpgroup WHERE auth_key <> '') THEN
        RAISE EXCEPTION 'FHRP auth keys require offline conversion before migration 0004';
    END IF;
END
$$;
ALTER TABLE ipam_fhrpgroup DROP COLUMN auth_key;
