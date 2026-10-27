//! The one engine schema world: every component, document kind and authoring command
//! the engine ships. The CLI, the daemon, the editor and the player all register
//! exactly this, so a document that validates in one validates in all of them. C#
//! module types join at load time (`ScriptHost`), project types from `*.types.json`.
use ara_cmd::Registry;
use ara_diag::Diagnostic;
use ara_schema::SchemaDb;

/// Registers every engine component, value type and document kind. Returns the
/// problems (type name clashes, invalid kinds); an engine build must return none.
pub fn register_schemas(db: &mut SchemaDb) -> Vec<Diagnostic> {
    let mut problems = Vec::new();
    ara_doc::register(db);
    ara_components::register(db);
    crate::components::register_schemas(db);
    if let Err(e) = ara_anim::document::register(db) {
        problems.push(Diagnostic::error(
            ara_diag::codes::DUPLICATE_TYPE,
            e.to_string(),
        ));
    }
    // Brains and perception (Brain, Perceiver, Stimulus).
    ara_ai::register(db);
    ara_audio::register(db);
    ara_acoustics::register(db);
    ara_2d::register(db);
    ara_physics2d::register(db);
    ara_world::register(db);
    ara_l10n::register_schema(db);
    ara_save::register_schema(db);
    for result in [
        ara_terrain::register(db),
        ara_foliage::register(db),
        ara_material::register(db),
        ara_audio_mix::register(db),
        ara_music::register(db),
    ] {
        problems.extend(result.err());
    }
    problems.extend(db.verify().into_iter().filter(Diagnostic::is_error));
    problems
}

/// Registers every engine authoring command, query and job (and their params) in a
/// host's registry, next to the kernel's builtin commands.
pub fn register_commands(registry: &mut Registry, db: &mut SchemaDb) {
    ara_anim::commands::register(registry, db);
    ara_music::commands::register(registry, db);
    ara_audio_mix::commands::register(registry, db);
    ara_material::commands::register(registry, db);
    ara_vfx::commands::register(registry, db);
    ara_2d::commands::register(registry, db);
    ara_physics2d::commands::register(registry, db);
    crate::physics::commands::register(registry, db);
    ara_terrain::commands::register(registry, db);
    ara_foliage::commands::register(registry, db);
    ara_l10n::tools::register(registry, db);
    ara_audio_graph::commands::register(registry, db);
    ara_ai::commands::register(registry, db);
}
