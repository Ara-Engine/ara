use crate::{PersistenceSelection, PersistentState, Result, codes, error};
use ara_diag::Diagnostic;
use ara_schema::{Schema, SchemaDb, Validator, ValueType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Storage class referenced by a persistence declaration, independent of replication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "SaveStorage", module = "save")]
#[serde(rename_all = "camelCase")]
pub enum Storage {
    /// Authoritative gameplay data attached to a stable entity.
    Component,
    /// Registered service state; gameplay state still belongs to components.
    Resource,
}
/// Reflected persistence flag. This extension descriptor references the Schema DB;
/// it does not duplicate fields, defaults, versions or migration implementations.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(name = "SavePersistDeclaration", module = "save")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistDeclaration {
    /// Stable reflected type name.
    pub type_name: String,
    /// Component or infrastructure resource.
    pub storage: Storage,
    /// Explicit opt-in. False declarations document excluded runtime/presentation data.
    pub persist: bool,
}
/// Registers the save policy vocabulary in an engine Schema DB.
pub fn register_schema(db: &mut SchemaDb) {
    db.register::<PersistDeclaration>();
}

/// Verified persistence policy bound to the engine's one Schema DB.
#[derive(Clone)]
pub struct SaveSchema {
    db: SchemaDb,
    declarations: BTreeMap<String, PersistDeclaration>,
    selection: PersistenceSelection,
}

impl SaveSchema {
    /// Validates declarations; unflagged types are always excluded.
    pub fn new(
        db: SchemaDb,
        declarations: impl IntoIterator<Item = PersistDeclaration>,
    ) -> Result<Self> {
        if let Some(d) = db.verify().into_iter().find(Diagnostic::is_error) {
            return Err(d);
        }
        let mut selected = BTreeMap::new();
        let mut selection = PersistenceSelection::default();
        for declaration in declarations {
            let desc = db.get(&declaration.type_name).ok_or_else(|| {
                error(
                    codes::SCHEMA,
                    format!("unknown persistent type {}", declaration.type_name),
                )
            })?;
            if (declaration.storage == Storage::Component) != desc.is_component() {
                return Err(error(
                    codes::SCHEMA,
                    "persistence storage class disagrees with reflection",
                ));
            }
            if desc.fields().is_empty() {
                return Err(error(
                    codes::SCHEMA,
                    "persistent type must be a nonempty reflected struct",
                ));
            }
            if declaration.persist {
                match declaration.storage {
                    Storage::Component => &mut selection.components,
                    Storage::Resource => &mut selection.resources,
                }
                .insert(declaration.type_name.clone());
            }
            if selected
                .insert(declaration.type_name.clone(), declaration)
                .is_some()
            {
                return Err(error(codes::SCHEMA, "duplicate persistence declaration"));
            }
        }
        Ok(Self {
            db,
            declarations: selected,
            selection,
        })
    }
    /// The shared reflection database, including version/upgrade hooks.
    pub fn db(&self) -> &SchemaDb {
        &self.db
    }
    /// Exact opt-in component/resource sets consumed by ara-sim's save bridge.
    pub fn selection(&self) -> &PersistenceSelection {
        &self.selection
    }
    /// Schema-visible persistence flags for inspectors, C# generators and agents.
    pub fn declarations(&self) -> impl Iterator<Item = &PersistDeclaration> {
        self.declarations.values()
    }
    /// Current version of every selected type, even if no instance is present.
    pub fn versions(&self) -> BTreeMap<String, u32> {
        self.selection
            .components
            .iter()
            .chain(&self.selection.resources)
            .map(|n| {
                (
                    n.clone(),
                    self.db.get(n).expect("verified persistent type").version,
                )
            })
            .collect()
    }

    /// Captures a validated persistent projection at a tick boundary.
    pub fn capture(&self, session: &ara_sim::Session) -> Result<PersistentState> {
        let state = session
            .persistent_state(&self.selection)
            .map_err(|e| error(codes::SCHEMA, e.to_string()))?;
        self.validate(&state)?;
        Ok(state)
    }
    /// Validates before applying; server authority is checked by SlotStore/tools.
    pub fn restore(&self, session: &mut ara_sim::Session, state: &PersistentState) -> Result<()> {
        self.validate(state)?;
        session
            .restore_persistent(state, &self.selection)
            .map_err(|e| error(codes::SCHEMA, e.to_string()))
    }
    /// Rejects unselected data, dangling persistent references and invalid field values.
    pub fn validate(&self, state: &PersistentState) -> Result<()> {
        if !state.context.dt.is_finite()
            || state.context.dt <= 0.0
            || state.context.tick == u64::MAX
        {
            return Err(error(codes::SCHEMA, "invalid persistent simulation cursor"));
        }
        if state
            .resources
            .keys()
            .collect::<std::collections::BTreeSet<_>>()
            != self.selection.resources.iter().collect()
        {
            return Err(error(codes::SCHEMA, "save resource policy mismatch"));
        }
        for (id, entity) in &state.entities {
            if entity.components.is_empty()
                && !state
                    .entities
                    .values()
                    .any(|e| e.parent.as_deref() == Some(id))
            {
                return Err(error(
                    codes::SCHEMA,
                    "empty persistent entity is not a hierarchy anchor",
                ));
            }
            if !ara_schema::grammar::is_entity_address(id) {
                return Err(error(codes::SCHEMA, "invalid stable entity identity"));
            }
            let mut seen = std::collections::BTreeSet::new();
            let mut parent = entity.parent.as_deref();
            while let Some(p) = parent {
                if p == id || !seen.insert(p) {
                    return Err(error(codes::SCHEMA, "persistent hierarchy cycle"));
                }
                parent = state
                    .entities
                    .get(p)
                    .ok_or_else(|| error(codes::SCHEMA, "persistent parent is absent"))?
                    .parent
                    .as_deref();
            }
            for (name, value) in &entity.components {
                if !self.selection.components.contains(name) {
                    return Err(error(
                        codes::SCHEMA,
                        format!("{name} is not a persistent component"),
                    ));
                }
                self.validate_value(name, value, state)?;
            }
        }
        for (name, value) in &state.resources {
            if let Some(value) = value {
                self.validate_value(name, value, state)?;
            }
        }
        Ok(())
    }
    fn validate_value(
        &self,
        name: &str,
        value: &serde_json::Value,
        state: &PersistentState,
    ) -> Result<()> {
        let ty = ValueType::Named(name.into());
        let mut diagnostics = Vec::new();
        Validator::new(&self.db, &mut diagnostics).value(&ty, None, value, name);
        if let Some(d) = diagnostics.into_iter().find(Diagnostic::is_error) {
            return Err(d);
        }
        let mut dangling = None;
        self.db.visit(&ty, value, name, &mut |ty, v, path| {
            if *ty == ValueType::Entity
                && v.as_str()
                    .is_some_and(|id| !id.is_empty() && !state.entities.contains_key(id))
            {
                dangling = Some(path.to_string());
            }
        });
        if let Some(path) = dangling {
            return Err(error(
                codes::SCHEMA,
                "persistent reference points to excluded/missing state",
            )
            .path(path));
        }
        Ok(())
    }

    /// Runs Schema DB normalize_component version steps and alias normalization on
    /// every selected component/resource. Future versions always fail closed.
    pub fn migrate(
        &self,
        state: &mut PersistentState,
        versions: &BTreeMap<String, u32>,
    ) -> Result<Vec<Diagnostic>> {
        if versions.keys().collect::<std::collections::BTreeSet<_>>()
            != self.versions().keys().collect()
        {
            return Err(error(
                codes::SCHEMA,
                "save persistence type set differs; explicit policy migration required",
            ));
        }
        for (name, version) in versions {
            if *version == 0 || *version > self.versions()[name] {
                return Err(error(
                    codes::SCHEMA,
                    "invalid or future persistent schema version",
                ));
            }
        }
        let mut candidate = state.clone();
        let mut diagnostics = Vec::new();
        for entity in candidate.entities.values_mut() {
            for (name, value) in &mut entity.components {
                self.upgrade(name, value, versions, &mut diagnostics)?;
            }
        }
        for (name, value) in &mut candidate.resources {
            if let Some(value) = value {
                self.upgrade(name, value, versions, &mut diagnostics)?;
            }
        }
        self.validate(&candidate)?;
        candidate
            .materialize(&self.db)
            .map_err(|e| error(codes::SCHEMA, e.to_string()))?;
        self.validate(&candidate)?;
        *state = candidate;
        Ok(diagnostics)
    }
    fn upgrade(
        &self,
        name: &str,
        value: &mut serde_json::Value,
        versions: &BTreeMap<String, u32>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<()> {
        let desc = self
            .db
            .get(name)
            .ok_or_else(|| error(codes::SCHEMA, "unknown saved type"))?;
        let version = versions
            .get(name)
            .ok_or_else(|| error(codes::SCHEMA, "saved type has no version entry"))?;
        if !self
            .db
            .normalize_component(desc, *version, value, name, diagnostics)
        {
            return Err(diagnostics
                .last()
                .cloned()
                .unwrap_or_else(|| error(codes::SCHEMA, "schema migration failed")));
        }
        Ok(())
    }
}
