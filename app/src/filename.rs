//! Turning a name a person or a manifest chose into one a filesystem will take.
//!
//! Two places do it — a plugin's `owner/name/version`, a theme's name — and
//! the rule a name has to clear to be a directory or a file is the filesystem's
//! rather than either of theirs, so it lives here rather than in either.

/// Whether a name is one Windows keeps for a device rather than a file.
///
/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9` and `LPT1`-`LPT9` name the console,
/// a printer, the serial and parallel ports and the bit bucket — with any
/// extension or none, so `con.yaml` is as reserved as `con` — and a file or
/// directory cannot be created under one. Case does not matter to Windows, so
/// it does not matter here. It is worth minding on every platform, because a
/// name a file takes on one machine and not another is worse than one it takes
/// on neither.
pub(crate) fn windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_names_are_reserved_whatever_their_case_or_extension() {
        for name in ["con", "CON", "Nul", "com1", "LPT9", "aux.yaml", "prn.wasm"] {
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
            "lpt",
            "1.con",
            "auxiliary",
        ] {
            assert!(!windows_reserved(name), "{name} is not a device name");
        }
    }
}
