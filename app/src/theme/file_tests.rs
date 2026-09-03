//! What a theme file is allowed to be, and what happens to one that is not.

use super::*;

/// A theme in Warp's format, complete.
///
/// Solarized Dark's values, which is the example Warp's own documentation and
/// its round-trip tests use — so this is also the assertion that a file
/// written for Warp is a file Crook reads.
const SOLARIZED: &str = r##"
name: Solarized Dark
background: "#002b36"
foreground: "#f8f8f2"
accent: "#cb4b16"
cursor: "#ffcc00"
details: darker
terminal_colors:
  normal:
    black: "#073642"
    red: "#dc322f"
    green: "#859900"
    yellow: "#b58900"
    blue: "#268bd2"
    magenta: "#d33682"
    cyan: "#2aa198"
    white: "#eee8d5"
  bright:
    black: "#002b36"
    red: "#cb4b16"
    green: "#586e75"
    yellow: "#657b83"
    blue: "#839496"
    magenta: "#6c71c4"
    cyan: "#93a1a1"
    white: "#fdf6e3"
"##;

#[test]
fn a_warp_theme_file_reads_as_a_theme() {
    let parsed = parse(SOLARIZED).expect("a complete theme should parse");

    assert_eq!(parsed.name.as_deref(), Some("Solarized Dark"));
    assert_eq!(parsed.theme.terminal.background, Color::hex(0x002b36));
    assert_eq!(parsed.theme.terminal.foreground, Color::hex(0xf8f8f2));
    assert_eq!(parsed.theme.accent, Color::hex(0xcb4b16));
    assert_eq!(parsed.theme.terminal.cursor, Color::hex(0xffcc00));
    assert_eq!(parsed.theme.terminal.normal[1], Color::hex(0xdc322f));
    assert_eq!(parsed.theme.terminal.bright[7], Color::hex(0xfdf6e3));

    // Everything the file did not say, said by the derivation.
    assert!(!parsed.theme.is_light, "Solarized Dark is a dark theme");
    assert_eq!(
        parsed.theme.surface, parsed.theme.terminal.background,
        "the chrome and the grid have to be one surface"
    );
    assert_ne!(parsed.theme.border, parsed.theme.surface);
}

/// The same theme as [`SOLARIZED`], written the way Warp's own serializer
/// writes one: single quotes, keys in alphabetical order, `bright` before
/// `normal`, and no leading blank line.
///
/// Written out rather than copied from Warp's test data — the shape is what is
/// being asserted, and the colours are this file's own.
const AS_WARP_WRITES_IT: &str = "\
accent: '#01a0e4'
background: '#090300'
details: darker
foreground: '#a5a2a2'
terminal_colors:
  bright:
    black: '#5c5855'
    blue: '#807d7c'
    cyan: '#cdab53'
    green: '#3a3432'
    magenta: '#d6d5d4'
    red: '#e8bbd0'
    white: '#f7f7f7'
    yellow: '#4a4543'
  normal:
    black: '#090300'
    blue: '#01a0e4'
    cyan: '#b5e4f4'
    green: '#01a252'
    magenta: '#a16a94'
    red: '#db2d20'
    white: '#a5a2a2'
    yellow: '#fded02'
";

#[test]
fn the_order_and_the_quoting_a_theme_is_written_in_do_not_matter() {
    // Warp writes its own themes with single quotes and every key in
    // alphabetical order — `bright` before `normal`, `blue` before `cyan` —
    // and a file it wrote has to read here exactly as one written by hand
    // does. The eight colours land in ANSI order whichever order the file
    // lists them in, which is the half of this that could quietly be wrong.
    let parsed = parse(AS_WARP_WRITES_IT).expect("Warp's own spelling should parse");

    assert_eq!(parsed.name, None, "this one names itself by its file");
    assert_eq!(
        parsed.theme.terminal.normal[0],
        Color::hex(0x090300),
        "black"
    );
    assert_eq!(parsed.theme.terminal.normal[1], Color::hex(0xdb2d20), "red");
    assert_eq!(
        parsed.theme.terminal.normal[4],
        Color::hex(0x01a0e4),
        "blue"
    );
    assert_eq!(
        parsed.theme.terminal.bright[3],
        Color::hex(0x4a4543),
        "bright yellow"
    );
    // No `cursor:` in this one either, so the accent stands in for it.
    assert_eq!(parsed.theme.terminal.cursor, Color::hex(0x01a0e4));
}

#[test]
fn a_comment_after_a_value_is_not_part_of_it() {
    let commented = AS_WARP_WRITES_IT.replace(
        "details: darker",
        "# a whole line of comment\ndetails: darker  # and a trailing one",
    );

    assert!(parse(&commented).is_ok(), "comments should be ignored");
}

#[test]
fn the_fields_warp_has_and_crook_does_not_are_skipped_rather_than_refused() {
    // `details` is in the file above and nothing here reads it. A theme file
    // is something people copy off the internet: one that carries a key this
    // build has never heard of is a theme, not an error.
    let with_extras = format!(
        "{SOLARIZED}\nbackground_image:\n  path: mybg.jpg\n  opacity: 30\nsome_future_key: 12\n"
    );

    assert!(parse(&with_extras).is_ok());
}

#[test]
fn a_cursor_that_is_not_named_is_the_accent() {
    // Warp's rule, and the reason `cursor:` is optional in half the themes
    // people have written.
    let without_cursor: String = SOLARIZED
        .lines()
        .filter(|line| !line.starts_with("cursor:"))
        .collect::<Vec<_>>()
        .join("\n");

    let parsed = parse(&without_cursor).expect("the cursor is optional");
    assert_eq!(parsed.theme.terminal.cursor, parsed.theme.accent);
}

#[test]
fn both_hex_forms_are_read_and_anything_else_is_refused() {
    assert_eq!(
        parse_hex("#abcdef").expect("six digits"),
        Color::hex(0xabcdef)
    );
    assert_eq!(
        parse_hex("#abc").expect("three digits"),
        Color::hex(0xaabbcc),
        "#abc means #aabbcc, the way it does everywhere else"
    );

    assert!(parse_hex("abcdef").is_err(), "the # is not optional");
    assert!(parse_hex("#ab").is_err());
    assert!(parse_hex("#abcde").is_err());
    assert!(parse_hex("#gggggg").is_err());
    assert!(
        parse_hex("rebeccapurple").is_err(),
        "named colours are not a thing here"
    );
}

#[test]
fn a_theme_missing_a_colour_it_cannot_do_without_says_which() {
    // The error is what a person sees in the log next to the file that did not
    // load, so it has to name the key rather than the line.
    let without_accent: String = SOLARIZED
        .lines()
        .filter(|line| !line.starts_with("accent:"))
        .collect::<Vec<_>>()
        .join("\n");
    let error = format!(
        "{:#}",
        parse(&without_accent).expect_err("accent is required")
    );
    assert!(
        error.contains("accent"),
        "the error does not name the key: {error}"
    );

    let without_red: String = SOLARIZED.replacen("    red: \"#dc322f\"\n", "", 1);
    let error = format!(
        "{:#}",
        parse(&without_red).expect_err("the eight are required")
    );
    assert!(
        error.contains("normal") && error.contains("red"),
        "the error does not say which colour of which block: {error}"
    );
}

#[test]
fn a_file_that_is_not_a_theme_at_all_is_refused_rather_than_half_read() {
    assert!(parse("just some words\n").is_err());
    assert!(
        parse("  black: \"#000000\"\n").is_err(),
        "a colour indented under nothing has no block to belong to"
    );
    assert!(parse("").is_err(), "an empty file names no colours");
}

#[test]
fn a_theme_names_itself_after_its_file_when_it_does_not_name_itself() {
    // Warp's rule: `solarized_dark.yaml` is "Solarized Dark", which is why
    // `name:` is optional at all.
    assert_eq!(
        name_from_path(Path::new("/themes/solarized_dark.yaml")),
        "Solarized Dark"
    );
    assert_eq!(
        name_from_path(Path::new("/themes/catppuccin/catppuccin_mocha.yml")),
        "Catppuccin Mocha"
    );
    assert_eq!(name_from_path(Path::new("/themes/nord.yaml")), "Nord");
}

#[test]
fn a_directory_of_themes_is_walked_into_its_subdirectories() {
    // The pattern Warp's own tests document: one directory per collection,
    // `themes/catppuccin/catppuccin_mocha.yml`.
    let root = std::env::temp_dir().join(format!("crook-themes-{}", std::process::id()));
    let nested = root.join("catppuccin");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&nested).expect("the scratch directory should be creatable");

    fs::write(root.join("solarized_dark.yaml"), SOLARIZED).expect("writable");
    fs::write(nested.join("catppuccin_mocha.yml"), SOLARIZED).expect("writable");
    // Neither of these is a theme: one is the wrong extension, and one is a
    // theme file that does not parse.
    fs::write(root.join("notes.txt"), SOLARIZED).expect("writable");
    fs::write(root.join("broken.yaml"), "background: \"#fff\"\n").expect("writable");

    let mut themes = Vec::new();
    collect(&root, 0, &mut themes);
    let mut names: Vec<&str> = themes.iter().map(|theme| theme.name.as_str()).collect();
    names.sort_unstable();

    assert_eq!(
        names,
        ["Solarized Dark", "Solarized Dark"],
        "the walk found {names:?} rather than the two themes"
    );

    let _ = fs::remove_dir_all(&root);
}
