/// Get only the initials from a name.
pub fn get_initials(name: &str) -> String {
    if name.contains(' ') {
        let (first, second) = name.split_once(" ").unwrap();
        format!(
            "{}{}",
            first.chars().next().unwrap(),
            second.chars().next().unwrap()
        )
    } else {
        name.chars().next().unwrap().to_string()
    }
}

/// Extract phone number from JID/LID.
pub fn extract_phone_from_jid(jid: &str) -> String {
    jid.split('@')
        .next()
        .map(|s| s.to_string())
        .unwrap_or(jid.to_string())
}
