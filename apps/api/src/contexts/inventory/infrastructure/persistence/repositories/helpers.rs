pub fn derive_slug(value: &str) -> String {
    value.trim().to_lowercase().replace(' ', "-")
}
