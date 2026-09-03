//! Themes on disk, in Warp's file format.
//!
//! ```yaml
//! name: Solarized Dark
//! background: "#002b36"
//! foreground: "#f8f8f2"
//! accent: "#cb4b16"
//! cursor: "#ffcc00"
//! terminal_colors:
//!   normal:
//!     black: "#073642"
//!     red: "#dc322f"
//!     green: "#859900"
//!     yellow: "#b58900"
//!     blue: "#268bd2"
//!     magenta: "#d33682"
//!     cyan: "#2aa198"
//!     white: "#eee8d5"
//!   bright:
//!     black: "#002b36"
//!     red: "#cb4b16"
//!     green: "#586e75"
//!     yellow: "#657b83"
//!     blue: "#839496"
//!     magenta: "#6c71c4"
//!     cyan: "#93a1a1"
//!     white: "#fdf6e3"
//! ```
//!
//! # Why Warp's format, exactly
//!
//! Because there are hundreds of these files already written. Warp publishes a
//! repository of them and every user who has ever made one has one of these on
//! disk; a format that differed by a key name would make all of them useless
//! for no gain at all. `background`, `foreground`, `accent`, `cursor`, `name`
//! and the two eight-colour blocks are Warp's spelling, and a file Warp reads
//! is a file this reads.
//!
//! What is *not* read is the rest of Warp's schema: `details` (ten opacity
//! knobs whose two presets are identical in Warp's own tree), `background_image`
//! and gradient fills. A file carrying them still loads — unknown keys are
//! skipped — it simply gets a flat background and the derived ladder.
//!
//! # Why a parser rather than a crate
//!
//! Crook takes no dependency it can avoid, and this is a two-level map of
//! strings: keys at the left margin, a nested block indented under
//! `terminal_colors`, and hex strings for values. The subset below is about a
//! hundred lines and refuses anything it does not understand instead of
//! guessing. A real YAML crate would accept anchors, flow mappings, multi-line
//! scalars and documents — none of which a theme file has ever contained — and
//! `serde_yaml`, the obvious candidate, is unmaintained.
//!
//! # Nothing here can cost a person their window
//!
//! A malformed theme is skipped with a warning, exactly as an unreadable
//! settings file falls back to the defaults. Warp does the same, and the
//! reason is the same: themes are the one part of a configuration people copy
//! off the internet, and one bad file must not be the difference between an
//! application that starts and one that does not.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use crookui_core::geometry::Color;

use super::{TerminalColors, Theme};

/// The directory user themes are read from.
const THEMES_DIRECTORY: &str = "themes";

/// The extensions a theme file may have. Warp accepts both.
const EXTENSIONS: [&str; 2] = ["yaml", "yml"];

/// How deep the walk goes.
///
/// Warp walks its themes directory recursively and unbounded; the pattern its
/// own tests document is one directory per collection —
/// `themes/catppuccin/catppuccin_mocha.yml` — so two levels is the shape in
/// use. The limit is here because a symlink loop in a configuration directory
/// should cost a warning rather than the process.
const MAX_DEPTH: usize = 4;

/// A theme read from a file: the palette, and what to call it.
#[derive(Clone, Debug)]
pub struct ThemeFile {
    /// The `name:` field, or the file's own name turned into words.
    pub name: String,
    /// Where it was read from, for the settings page to point at.
    pub path: PathBuf,
    /// The palette itself.
    pub theme: Theme,
}

/// `<configuration directory>/crook/themes`, where there is one.
///
/// Warp keeps its themes in the *data* directory rather than the
/// configuration one. Crook has one directory for everything it remembers, and
/// a second one whose only content is themes would be a directory people have
/// to be told about twice.
pub fn user_themes_directory() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("crook").join(THEMES_DIRECTORY))
}

/// Every theme file under the user's themes directory, sorted by name.
///
/// Failures are logged and skipped — a missing directory is the ordinary state
/// of a machine that has never written a theme, and an unparseable file is one
/// theme fewer rather than an error anybody has to act on.
pub fn load_user_themes() -> Vec<ThemeFile> {
    let Some(directory) = user_themes_directory() else {
        return Vec::new();
    };

    let mut themes = Vec::new();
    collect(&directory, 0, &mut themes);
    themes.sort_by(|left, right| left.name.cmp(&right.name));
    themes
}

/// Reads every theme in `directory`, and in the directories under it.
fn collect(directory: &Path, depth: usize, themes: &mut Vec<ThemeFile>) {
    if depth > MAX_DEPTH {
        log::warn!("{} is nested too deeply to search", directory.display());
        return;
    }

    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        // The ordinary state of a machine that has never written a theme.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            log::warn!("could not read {} ({error})", directory.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, depth + 1, themes);
            continue;
        }

        let is_theme = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| EXTENSIONS.contains(&extension));
        if !is_theme {
            continue;
        }

        match read(&path) {
            Ok(theme) => themes.push(theme),
            Err(error) => log::warn!("skipping {} ({error:#})", path.display()),
        }
    }
}

/// Reads one theme file.
pub fn read(path: &Path) -> Result<ThemeFile> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    let theme = parse(&contents)?;

    Ok(ThemeFile {
        name: theme.name.unwrap_or_else(|| name_from_path(path)),
        path: path.to_owned(),
        theme: theme.theme,
    })
}

/// A theme's display name from its file name.
///
/// Warp's rule: strip the extension, split on underscores, capitalise each
/// word. `solarized_dark.yaml` becomes "Solarized Dark", which is why `name:`
/// is optional in the first place.
fn name_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Theme");

    stem.split(['_', '-'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A parsed file: the palette, and the name it asked to be called.
#[derive(Debug)]
struct Parsed {
    name: Option<String>,
    theme: Theme,
}

/// Parses one theme file.
fn parse(contents: &str) -> Result<Parsed> {
    let document = Document::parse(contents)?;

    let background = document.color("background")?;
    let foreground = document.color("foreground")?;
    let accent = document.color("accent")?;
    // Warp's rule: a theme that names no cursor colour uses its accent.
    let cursor = document.optional_color("cursor")?.unwrap_or(accent);

    let terminal = TerminalColors {
        foreground,
        background,
        cursor,
        normal: document.ansi_block("normal")?,
        bright: document.ansi_block("bright")?,
    };

    Ok(Parsed {
        name: document.string("name"),
        theme: Theme::derived(accent, terminal),
    })
}

/// The eight colour names in a `normal` or `bright` block, in ANSI order.
const ANSI_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// The subset of YAML a theme file is written in.
///
/// Two levels and nothing else: `key: value` at the left margin, and indented
/// `key: value` under a key that had no value of its own. That is the whole
/// grammar, which is why the nesting is tracked as "which block am I in"
/// rather than as a tree.
struct Document {
    /// Top-level `key: value` pairs.
    top: Vec<(String, String)>,
    /// Pairs inside a block, as `(block, key, value)`.
    nested: Vec<(String, String, String)>,
}

impl Document {
    fn parse(contents: &str) -> Result<Self> {
        let mut top = Vec::new();
        let mut nested = Vec::new();
        let mut block: Option<String> = None;
        // The indent the current block's keys sit at, so a block inside a
        // block — `terminal_colors:` then `normal:` — is not mistaken for its
        // parent's sibling.
        let mut block_indent = 0;

        for (number, line) in contents.lines().enumerate() {
            let trimmed = line.trim();
            // A whole line of comment. A `#` anywhere *else* is a colour, not
            // a comment, which is the reason every theme file quotes its hex
            // values in the first place — and the reason stripping comments
            // before parsing, the obvious way round, turns `"#002b36"` into a
            // lone quote.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let indent = line.len() - line.trim_start().len();
            let Some((key, value)) = trimmed.split_once(':') else {
                bail!("line {} is not `key: value`: {trimmed:?}", number + 1);
            };

            let key = key.trim().to_owned();
            let value = value_of(value);

            if indent == 0 {
                block = value.is_empty().then(|| key.clone());
                block_indent = 0;
                if !value.is_empty() {
                    top.push((key, value));
                }
                continue;
            }

            match &block {
                // A block of its own inside a block: `normal:` under
                // `terminal_colors:`. The inner name is what the colours below
                // it belong to, which is all this parser needs — no theme file
                // has ever had two blocks with the same inner name.
                Some(_) if value.is_empty() => {
                    block = Some(key);
                    block_indent = indent;
                }
                Some(current) if indent > block_indent => {
                    nested.push((current.clone(), key, value));
                }
                _ => bail!("line {} is indented under nothing: {trimmed:?}", number + 1),
            }
        }

        Ok(Self { top, nested })
    }

    /// A top-level string value.
    fn string(&self, key: &str) -> Option<String> {
        self.top
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    /// A required top-level colour.
    fn color(&self, key: &str) -> Result<Color> {
        let value = self
            .string(key)
            .with_context(|| format!("the theme has no `{key}`"))?;
        parse_hex(&value).with_context(|| format!("`{key}` is not a colour"))
    }

    /// A top-level colour, if the file has one.
    fn optional_color(&self, key: &str) -> Result<Option<Color>> {
        match self.string(key) {
            Some(value) => parse_hex(&value)
                .map(Some)
                .with_context(|| format!("`{key}` is not a colour")),
            None => Ok(None),
        }
    }

    /// The eight colours of a `normal` or `bright` block, in ANSI order.
    fn ansi_block(&self, block: &str) -> Result<[Color; 8]> {
        let mut colors = [Color::BLACK; 8];
        for (index, name) in ANSI_NAMES.iter().enumerate() {
            let value = self
                .nested
                .iter()
                .find(|(in_block, key, _)| in_block == block && key == name)
                .map(|(_, _, value)| value.as_str())
                .with_context(|| format!("`terminal_colors.{block}` has no `{name}`"))?;
            colors[index] = parse_hex(value)
                .with_context(|| format!("`terminal_colors.{block}.{name}` is not a colour"))?;
        }
        Ok(colors)
    }
}

/// What is to the right of a `key:`, with its quotes and any trailing comment
/// taken off.
///
/// A quoted value ends at its closing quote and everything after it is
/// ignored; an unquoted one ends at a ` #`, which is the only place a comment
/// can start once the quoted case has been handled. `"#002b36"`, `'#002b36'`
/// and `darker  # a comment` all come out as what they say.
fn value_of(value: &str) -> String {
    let value = value.trim();

    for quote in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(quote) {
            return match rest.split_once(quote) {
                Some((inside, _)) => inside.to_owned(),
                // An opening quote and no closing one. Taking the rest of the
                // line is what every lenient parser does, and the value is
                // about to be checked for being a colour anyway.
                None => rest.to_owned(),
            };
        }
    }

    match value.split_once(" #") {
        Some((before, _)) => before.trim_end().to_owned(),
        None => value.to_owned(),
    }
}

/// `#rrggbb` or `#rgb`, the two forms Warp's parser accepts.
fn parse_hex(value: &str) -> Result<Color> {
    let digits = value
        .strip_prefix('#')
        .with_context(|| format!("{value:?} does not start with `#`"))?;

    let component = |from: usize, to: usize| -> Result<u8> {
        u8::from_str_radix(&digits[from..to], 16)
            .with_context(|| format!("{value:?} is not a hex colour"))
    };

    match digits.len() {
        6 => Ok(Color::rgb(
            component(0, 2)?,
            component(2, 4)?,
            component(4, 6)?,
        )),
        // `#abc` means `#aabbcc`, which is what every hex colour parser on the
        // web does and what Warp's does too.
        3 => {
            let double = |from: usize| -> Result<u8> {
                let digit = u8::from_str_radix(&digits[from..=from], 16)
                    .with_context(|| format!("{value:?} is not a hex colour"))?;
                Ok(digit * 17)
            };
            Ok(Color::rgb(double(0)?, double(1)?, double(2)?))
        }
        _ => bail!("{value:?} is not three or six hex digits"),
    }
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
