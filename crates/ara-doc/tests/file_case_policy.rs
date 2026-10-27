//! Import planners can simulate a target filesystem without duplicating its Unicode rules.
use ara_doc::file_state::{path_key, path_key_with_case};

#[test]
fn explicit_case_policy_is_available_independently_of_the_native_filesystem() {
    let key = |path| path_key_with_case(path, true);
    assert_eq!(path_key_with_case("Game/Σ.cs", false), "Game/Σ.cs");
    assert_eq!(key("Game/Σ.cs"), key("game/ς.CS"));
    assert_eq!(key("game/σ.cs"), key("game/Σ.cs"));
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(key("game/ı.cs"), key("GAME/I.CS"));
        assert_ne!(key("game/İ.cs"), key("game/i.cs"));
        assert_ne!(key("game/ß.cs"), key("game/SS.cs"));
        assert_ne!(key("game/é.cs"), key("game/e\u{301}.cs"));
    }
    #[cfg(target_os = "macos")]
    {
        assert_eq!(key("game/ß.cs"), key("game/SS.cs"));
        assert_eq!(key("game/é.cs"), key("game/e\u{301}.cs"));
    }
    #[cfg(any(windows, target_os = "macos"))]
    assert_eq!(path_key("Game/ς.cs"), key("Game/ς.cs"));
    #[cfg(not(any(windows, target_os = "macos")))]
    assert_eq!(path_key("Game/ς.cs"), "Game/ς.cs");
}
