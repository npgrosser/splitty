//! Keeps the keyboard reference of the documentation equal to the registry.
use super::{defaults, Category, Command, Platform};

/// The default keys of every command as the documentation lists them: one
/// table per category.
fn reference() -> String {
    let keys = |platform, command| {
        let labels: Vec<_> = defaults(platform, command)
            .iter()
            .map(|chord| format!("`{}`", chord.label(platform)))
            .collect();
        if labels.is_empty() {
            "none".to_owned()
        } else {
            labels.join(", ")
        }
    };
    let mut text = String::new();
    let mut category: Option<Category> = None;
    for command in Command::all() {
        if category != Some(command.category()) {
            category = Some(command.category());
            text.push_str(&format!(
                "\n### {}\n\n| Command | Identifier | Linux, Windows | macOS |\n|---|---|---|---|\n",
                command.category().title()
            ));
        }
        text.push_str(&format!(
            "| {} | `{}` | {} | {} |\n",
            command.title(),
            command.id(),
            keys(Platform::Linux, command),
            keys(Platform::Mac, command),
        ));
    }
    text
}

/// `docs/keyboard.md` holds the tables between its two markers. Run with
/// `SPLITTY_WRITE_KEYBOARD_REFERENCE=1` to rewrite them after a change of
/// the registry.
#[test]
fn the_keyboard_reference_lists_the_registry() {
    const BEGIN: &str = "<!-- keys:begin -->\n";
    const END: &str = "<!-- keys:end -->";
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/keyboard.md");
    let document = std::fs::read_to_string(path).expect("docs/keyboard.md");
    let start = document.find(BEGIN).expect("begin marker") + BEGIN.len();
    let end = document.find(END).expect("end marker");
    let expected = reference();
    if std::env::var_os("SPLITTY_WRITE_KEYBOARD_REFERENCE").is_some() {
        let updated = format!("{}{expected}\n{}", &document[..start], &document[end..]);
        std::fs::write(path, updated).expect("write docs/keyboard.md");
        return;
    }
    assert_eq!(
        document[start..end].trim(),
        expected.trim(),
        "docs/keyboard.md differs from the command registry"
    );
}
