//! The managed crash handler is compiled into every shipping C# assembly inside
//! `namespace Ara.Shipping`, where a bare `Environment` resolves to the generated
//! `Ara.Environment` component rather than `System.Environment`. Exports of C# games
//! failed with CS0117 until its BCL calls were fully qualified.

#[test]
fn managed_crash_handler_fully_qualifies_system_environment() {
    let source = include_str!("../src/managed-crash.cs");
    for (number, line) in source.lines().enumerate() {
        let bare = line
            .match_indices("Environment.")
            .any(|(at, _)| !line[..at].ends_with("global::System."));
        assert!(
            !bare,
            "managed-crash.cs:{}: write global::System.Environment, not Environment: {line}",
            number + 1
        );
        // In an interpolation hole `::` starts a format specifier: `{global::X}` fails
        // to compile (CS0103), so qualified names there need parentheses.
        assert!(
            !line.contains("{global::"),
            "managed-crash.cs:{}: parenthesize qualified names in interpolations: {line}",
            number + 1
        );
    }
}
