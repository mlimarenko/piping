-- Windows targets use the same connection-config records as SSH: the record
-- protocol is the single explicit transport selection for a playbook target.
ALTER TABLE public.connconf_record
    DROP CONSTRAINT connconf_record_protocol_check;
ALTER TABLE public.connconf_record
    ADD CONSTRAINT connconf_record_protocol_check
    CHECK (protocol = ANY (ARRAY['ssh'::text, 'winrm'::text, 'psrp'::text, 'rdp'::text, 'vnc'::text, 'app'::text, 'cctv'::text]));
