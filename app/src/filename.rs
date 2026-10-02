//! Turning a name a person or a manifest chose into one a filesystem will take.
//!
//! Two places do it — a plugin's `owner/name/version`, a theme's name — and
//! the rule a name has to clear to be a directory or a file is the filesystem's
//! rather than either of theirs, so it lives here rather than in either.

/// Whether a name is one Windows keeps for a device rather than a file.
///
/// `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9` and `LPT0`-`LPT9` name the console,
/// a printer, the serial and parallel ports and the bit bucket — with any
/// extension or none, so `con.yaml` is as reserved as `con` — and a file or
/// directory cannot be created under one. Windows also reads the Latin-1
/// superscripts `¹`, `²` and `³` as port digits, so `COM¹` is a port too, and
/// the `0` ports are on Microsoft's list even though no machine has one. Case
/// does not matter to Windows, so it does not matter here. It is worth minding
/// on every platform, because a name a file takes on one machine and not
/// another is worse than one it takes on neither.
pub(crate) fn windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    match stem.as_str() {
        "con" | "prn" | "aux" | "nul" => true,
        _ => ["com", "lpt"].iter().any(|port| {
            stem.strip_prefix(port).is_some_and(|digit| {
                let mut digits = digit.chars();
                matches!(
                    (digits.next(), digits.next()),
                    (Some('0'..='9' | '¹' | '²' | '³'), None)
                )
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_names_are_reserved_whatever_their_case_or_extension() {
        for name in [
            "con",
            "CON",
            "Nul",
            "com1",
            "LPT9",
            "aux.yaml",
            "prn.wasm",
            "com0",
            "LPT0",
            "com¹",
            "LPT³.txt",
        ] {
            assert!(windows_reserved(name), "{name} is a device name");
        }
    }

    #[test]
    fn a_name_that_only_starts_like_one_is_not_reserved() {
        // The stem is the whole of what Windows minds — up to the first dot —
        // so a device name after the dot, or a longer word that begins with
        // one, is an ordinary file.
        for name in [
            "console",
            "connection",
            "com",
            "com10",
            "com⁴",
            "lpt-1",
            "lpt",
            "1.con",
            "auxiliary",
        ] {
            assert!(!windows_reserved(name), "{name} is not a device name");
        }
    }
}
