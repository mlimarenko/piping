//! Typed taxonomy of device families.
//!
//! A *family* selects which native driver speaks to a device. It is a typed
//! enum — never a keyword/marker dictionary — so the resolver stays structural
//! and domain-neutral. Family binding is set by the operator (structurally, on
//! the platform/device) or inferred from a structural fingerprint, never from
//! natural-language matching on a brand string.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The protocol family a device belongs to.
///
/// New families are additive; downstream `match`es must stay exhaustive, so the
/// enum is intentionally *not* `#[non_exhaustive]` within this crate — the
/// compiler is the tool that finds every site needing an update when a family
/// is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFamily {
    /// Sofia / XMeye / NetSurveillance — JSON-over-TCP on 34567. Covers the
    /// large Xiongmai OEM tree (Polyvision, Hunter, Optimus, Tantos, Jasun, …).
    DvripSofia,
    /// Dahua — HTTP CGI (`/cgi-bin/...`) with digest auth, plus native DHIP on
    /// 37777. Covers Amcrest / Lorex / EmpireTech OEMs.
    Dahua,
    /// Hikvision — ISAPI (`/ISAPI/...`) over HTTP with digest auth. Covers
    /// HiLook and Hikvision OEMs.
    Hikvision,
    /// Generic ONVIF — the standardized fallback for the long tail. Reliable for
    /// streaming/snapshot; administration coverage is partial and device-dependent.
    Onvif,
    /// A Linux host managed agentlessly over the existing SSH proxy path. Not a
    /// video device: its management plane is local user administration
    /// (`getent`/`useradd`/`chpasswd`/`usermod`/`userdel`), so it must never be
    /// treated as a cctv camera (see [`DeviceFamily::is_video`]).
    Linux,
}

impl DeviceFamily {
    /// Stable wire identifier, matching the `serde` `snake_case` rename.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceFamily::DvripSofia => "dvrip_sofia",
            DeviceFamily::Dahua => "dahua",
            DeviceFamily::Hikvision => "hikvision",
            DeviceFamily::Onvif => "onvif",
            DeviceFamily::Linux => "linux",
        }
    }

    /// Whether this family is a video (cctv) device.
    ///
    /// The `cctv` resource/launch protocol applies to cameras and recorders only;
    /// a Linux host rides the ordinary `ssh` launch path. Every cctv-ness
    /// derivation must use this predicate rather than "family is set", so adding
    /// a non-video family cannot silently classify it as a camera.
    #[must_use]
    pub fn is_video(&self) -> bool {
        match self {
            DeviceFamily::DvripSofia
            | DeviceFamily::Dahua
            | DeviceFamily::Hikvision
            | DeviceFamily::Onvif => true,
            DeviceFamily::Linux => false,
        }
    }

    /// Default TCP management port for the family.
    ///
    /// This is the port a driver dials for control operations when the device
    /// record does not override it. It is a per-family constant, not a value
    /// baked into shared logic.
    #[must_use]
    pub fn default_management_port(&self) -> u16 {
        match self {
            // Raw TCP DVRIP.
            DeviceFamily::DvripSofia => 34567,
            // Management APIs for these families are HTTP on :80 (Dahua CGI,
            // Hikvision ISAPI, ONVIF SOAP). The binary DHIP 37777 / SDK 8000
            // ports are fingerprint signals, not what the HTTP drivers dial.
            DeviceFamily::Dahua | DeviceFamily::Hikvision | DeviceFamily::Onvif => 80,
            // SSH, the Linux host's agentless management transport.
            DeviceFamily::Linux => 22,
        }
    }

    /// Parse a wire identifier, returning `None` for anything unrecognized.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        parse_device_family(value).ok()
    }
}

/// Error returned when a string cannot be parsed into a [`DeviceFamily`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DeviceFamilyParseError {
    /// The input was empty or whitespace-only.
    #[error("device family cannot be empty")]
    Empty,
    /// The input did not match any known family.
    #[error("invalid device family: {0}")]
    Invalid(String),
}

/// Parse a wire identifier into a [`DeviceFamily`].
///
/// # Errors
///
/// Returns [`DeviceFamilyParseError::Empty`] for blank input and
/// [`DeviceFamilyParseError::Invalid`] for an unrecognized identifier.
pub fn parse_device_family(value: &str) -> Result<DeviceFamily, DeviceFamilyParseError> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(DeviceFamilyParseError::Empty);
    }
    match normalized.as_str() {
        "dvrip_sofia" => Ok(DeviceFamily::DvripSofia),
        "dahua" => Ok(DeviceFamily::Dahua),
        "hikvision" => Ok(DeviceFamily::Hikvision),
        "onvif" => Ok(DeviceFamily::Onvif),
        "linux" => Ok(DeviceFamily::Linux),
        _ => Err(DeviceFamilyParseError::Invalid(normalized)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_parse() {
        for family in [
            DeviceFamily::DvripSofia,
            DeviceFamily::Dahua,
            DeviceFamily::Hikvision,
            DeviceFamily::Onvif,
            DeviceFamily::Linux,
        ] {
            assert_eq!(DeviceFamily::parse(family.as_str()), Some(family));
        }
    }

    #[test]
    fn only_video_families_are_recognized_as_cctv() {
        assert!(DeviceFamily::DvripSofia.is_video());
        assert!(DeviceFamily::Dahua.is_video());
        assert!(DeviceFamily::Hikvision.is_video());
        assert!(DeviceFamily::Onvif.is_video());
        assert!(!DeviceFamily::Linux.is_video());
    }

    #[test]
    fn parse_is_case_insensitive_and_trims() {
        assert_eq!(
            DeviceFamily::parse("  Hikvision "),
            Some(DeviceFamily::Hikvision)
        );
    }

    #[test]
    fn parse_rejects_empty_and_unknown() {
        assert_eq!(
            parse_device_family("   "),
            Err(DeviceFamilyParseError::Empty)
        );
        assert_eq!(
            parse_device_family("acme"),
            Err(DeviceFamilyParseError::Invalid("acme".to_owned()))
        );
    }

    #[test]
    fn default_ports_are_the_driver_dial_ports() {
        assert_eq!(DeviceFamily::DvripSofia.default_management_port(), 34567);
        assert_eq!(DeviceFamily::Dahua.default_management_port(), 80);
        assert_eq!(DeviceFamily::Hikvision.default_management_port(), 80);
        assert_eq!(DeviceFamily::Linux.default_management_port(), 22);
    }
}
