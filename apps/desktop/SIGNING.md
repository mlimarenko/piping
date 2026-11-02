# Desktop Signing

Desktop artifacts belong to the GitLab release pipeline. macOS packaging must
fail closed when signing is unavailable: unsigned DMGs must not be published,
because macOS Gatekeeper rejects downloaded unsigned apps as damaged.

## Apple Developer ID

- Confirm the legal entity that will own the Apple Developer Program account.
- Complete or verify D-U-N-S and organization identity records.
- Assign an account holder and backup admin for certificate management.
- Enroll or renew the Apple Developer Program membership.
- Generate a Developer ID Application certificate and export it as `.p12`.
- Provision the signing material as protected GitLab CI variables, never in
  source files or public GitHub settings.
- Store the base64-encoded `.p12` as `APPLE_CERTIFICATE`.
- Store the `.p12` password as `APPLE_CERTIFICATE_PASSWORD`.
- Store a random CI keychain password as `APPLE_KEYCHAIN_PASSWORD`.
- Prepare notarization credentials, using App Store Connect API keys where possible:
  `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_PRIVATE_KEY_B64`.
- If Apple ID notarization is used instead, define `APPLE_ID`, `APPLE_PASSWORD`
  and `APPLE_TEAM_ID`.
- Record Apple team ID, certificate expiry dates, and renewal owner.
- Run a signed and notarized DMG smoke test on a clean macOS machine.

## Windows

Unsigned Windows installers show a SmartScreen warning. Public Windows
distribution needs an explicit signing decision, including the certificate
authority, key storage and protected GitLab CI credentials. The removed GitHub
workflow is not a packaging or signing fallback.
