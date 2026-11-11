//! Supported user-owned Ansible options. Inventory, variables, limits and
//! credentials are separate platform inputs, never argv escape hatches.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnsibleOptions {
    pub tags: Option<String>,
    pub skip_tags: Option<String>,
    pub check: bool,
    pub diff: bool,
    pub verbosity: u8,
    pub become_enabled: bool,
    pub become_user: Option<String>,
    pub become_method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentError {
    Invalid(&'static str),
    Conflict(&'static str),
    NeedsInput,
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Conflict(message) => formatter.write_str(message),
            Self::NeedsInput => {
                formatter.write_str("Interactive Ansible prompts are not supported")
            }
        }
    }
}

impl std::error::Error for ArgumentError {}

impl AnsibleOptions {
    /// Canonical argv uses attached values, so a value cannot become another flag.
    pub fn argv(&self) -> Vec<String> {
        let mut args = Vec::new();
        for (name, value) in [
            ("--tags", &self.tags),
            ("--skip-tags", &self.skip_tags),
            ("--become-user", &self.become_user),
            ("--become-method", &self.become_method),
        ] {
            if let Some(value) = value {
                args.push(format!("{name}={value}"));
            }
        }
        for (enabled, name) in [
            (self.check, "--check"),
            (self.diff, "--diff"),
            (self.become_enabled, "--become"),
        ] {
            if enabled {
                args.push(name.to_string());
            }
        }
        if self.verbosity > 0 {
            args.push(format!("-{}", "v".repeat(usize::from(self.verbosity))));
        }
        args
    }
}

fn set_flag(flag: &mut bool) -> Result<(), ArgumentError> {
    if *flag {
        return Err(ArgumentError::Invalid("Ansible option is repeated"));
    }
    *flag = true;
    Ok(())
}

/// Decode only the documented launch matrix. Both the API and runner call this
/// before accepting a request; unknown options do not reach Ansible.
pub fn parse_ansible_options(raw: &[String]) -> Result<AnsibleOptions, ArgumentError> {
    let mut options = AnsibleOptions::default();
    let mut index = 0;
    while index < raw.len() {
        let argument = &raw[index];
        if argument.contains('\0') || argument == "--" || !argument.starts_with('-') {
            return Err(ArgumentError::Invalid(
                "Launch arguments must contain supported options only",
            ));
        }
        let (name, attached) = if argument.starts_with("--") {
            argument
                .split_once('=')
                .map_or((argument.as_str(), None), |(name, value)| {
                    (name, Some(value))
                })
        } else if argument.len() > 2 && matches!(argument.as_bytes()[1], b't' | b'i' | b'e' | b'l')
        {
            (&argument[..2], Some(&argument[2..]))
        } else {
            (argument.as_str(), None)
        };
        if matches!(
            name,
            "-k" | "-K"
                | "-J"
                | "--ask-pass"
                | "--ask-become-pass"
                | "--ask-vault-pass"
                | "--ask-vault-password"
                | "--step"
        ) || (attached.is_none()
            && !argument.starts_with("--")
            && argument[1..]
                .bytes()
                .any(|byte| matches!(byte, b'k' | b'K' | b'J')))
        {
            return Err(ArgumentError::NeedsInput);
        }
        match name {
            "-i" | "--inventory" | "--inventory-file" => {
                return Err(ArgumentError::Conflict(
                    "Inventory is owned by the job inventory selection",
                ));
            }
            "-e" | "--extra-vars" => {
                return Err(ArgumentError::Conflict(
                    "Variables are owned by the parameter editor",
                ));
            }
            "-l" | "--limit" => {
                return Err(ArgumentError::Conflict(
                    "Limit is owned by the job limit field",
                ));
            }
            "-C" | "--check" if attached.is_none() => set_flag(&mut options.check)?,
            "-D" | "--diff" if attached.is_none() => set_flag(&mut options.diff)?,
            "-b" | "--become" if attached.is_none() => set_flag(&mut options.become_enabled)?,
            "--verbose" if attached.is_none() => {
                if options.verbosity != 0 {
                    return Err(ArgumentError::Invalid("Verbosity is repeated"));
                }
                options.verbosity = 1;
            }
            value if value.starts_with("-v") && value[1..].bytes().all(|byte| byte == b'v') => {
                if options.verbosity != 0 || value.len() > 5 {
                    return Err(ArgumentError::Invalid(
                        "Verbosity must be set once, from 0 to 4",
                    ));
                }
                options.verbosity = (value.len() - 1) as u8;
            }
            "-t" | "--tags" | "--skip-tags" | "--become-user" | "--become-method" => {
                let value = if let Some(value) = attached {
                    value
                } else {
                    index += 1;
                    raw.get(index)
                        .ok_or(ArgumentError::Invalid("Ansible option needs a value"))?
                };
                if value.is_empty() || value.chars().any(char::is_control) {
                    return Err(ArgumentError::Invalid(
                        "Ansible option value must be nonempty and contain no control characters",
                    ));
                }
                let slot = match name {
                    "-t" | "--tags" => &mut options.tags,
                    "--skip-tags" => &mut options.skip_tags,
                    "--become-user" => &mut options.become_user,
                    _ => &mut options.become_method,
                };
                if slot.is_some() {
                    return Err(ArgumentError::Invalid("Ansible option is repeated"));
                }
                *slot = Some(value.to_string());
            }
            _ => {
                return Err(ArgumentError::Invalid(
                    "Unsupported Ansible option; use the supported launch settings",
                ));
            }
        }
        index += 1;
    }
    if !options.become_enabled && (options.become_user.is_some() || options.become_method.is_some())
    {
        return Err(ArgumentError::Invalid(
            "Become user and method require explicit become",
        ));
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn preserves_supported_values_without_shell_splitting() {
        let input = strings(&[
            "-t",
            "deploy,odd ' $(id);",
            "--skip-tags=slow",
            "-C",
            "-D",
            "-vv",
            "--become",
            "--become-user",
            "service",
            "--become-method=sudo",
        ]);
        let parsed = parse_ansible_options(&input).unwrap();
        assert_eq!(parsed.tags.as_deref(), Some("deploy,odd ' $(id);"));
        assert_eq!(parsed.verbosity, 2);
        assert_eq!(parse_ansible_options(&parsed.argv()).unwrap(), parsed);
        assert!(
            parsed
                .argv()
                .contains(&"--tags=deploy,odd ' $(id);".to_string())
        );
    }

    #[test]
    fn rejects_all_platform_owned_forms_even_without_saved_values() {
        for input in [
            strings(&["-ihosts"]),
            strings(&["--inventory=hosts"]),
            strings(&["--inventory-file", "hosts"]),
            strings(&["-efoo=bar"]),
            strings(&["--extra-vars", "@file"]),
            strings(&["-lhost"]),
            strings(&["--limit=host"]),
        ] {
            assert!(matches!(
                parse_ansible_options(&input),
                Err(ArgumentError::Conflict(_))
            ));
        }
    }

    #[test]
    fn rejects_interactive_security_and_ambiguous_options() {
        for input in [
            strings(&["-vK"]),
            strings(&["--step"]),
            strings(&["--vault-id=prod@prompt"]),
            strings(&["--vault-password-file=/secrets/file"]),
            strings(&["--private-key=/key"]),
            strings(&["--connection=local"]),
            strings(&["--tags"]),
            strings(&["--check=false"]),
            strings(&["--tags=a", "--tags=b"]),
            strings(&["-vvvvv"]),
            strings(&["--become-user=root"]),
            strings(&["other.yml"]),
        ] {
            assert!(parse_ansible_options(&input).is_err());
        }
        assert_eq!(
            parse_ansible_options(&strings(&["-vK"])),
            Err(ArgumentError::NeedsInput)
        );
    }
}
