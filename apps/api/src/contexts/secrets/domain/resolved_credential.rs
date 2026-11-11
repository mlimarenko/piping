//! A credential value together with the revision of the stored row it came from.

use uuid::Uuid;

/// Credential the secret owner resolved for one consumer field.
///
/// The plaintext and [`Self::revision`] are produced from the same stored
/// ciphertext inside one locked read, so a concurrent rotation can never pair an
/// old value with the revision of the new one. The value is not `Debug`ged, not
/// `Display`ed and not `Serialize`d; a consumer has to ask for it explicitly with
/// [`Self::expose_value`], which keeps plaintext out of config dumps, ledger rows
/// and log lines by construction.
pub struct ResolvedCredential {
    secret_id: Uuid,
    revision: String,
    value: String,
}

impl ResolvedCredential {
    pub(crate) fn new(secret_id: Uuid, revision: String, value: String) -> Self {
        Self {
            secret_id,
            revision,
            value,
        }
    }

    /// Secret whose stored value produced both the plaintext and the revision.
    pub fn secret_id(&self) -> Uuid {
        self.secret_id
    }

    /// Opaque revision of the stored ciphertext this value was decrypted from.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The decrypted credential. Audit, log and serialization paths must not call
    /// this; only the client that has to authenticate with it may.
    pub fn expose_value(&self) -> &str {
        &self.value
    }
}

impl core::fmt::Debug for ResolvedCredential {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ResolvedCredential")
            .field("secret_id", &self.secret_id)
            .field("revision", &self.revision)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::ResolvedCredential;

    #[test]
    fn debug_never_prints_the_plaintext() {
        let credential = ResolvedCredential::new(
            uuid::Uuid::nil(),
            "revision-1".to_string(),
            "plaintext-token-1".to_string(),
        );
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("plaintext-token-1"), "{rendered}");
        assert!(rendered.contains("revision-1"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert_eq!(credential.expose_value(), "plaintext-token-1");
        assert_eq!(credential.revision(), "revision-1");
    }
}
