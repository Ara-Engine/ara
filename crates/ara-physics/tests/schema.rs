use ara_physics::*;
use ara_schema::{Schema, SchemaDb, Validator};
use serde::Serialize;

fn defaults<T: Schema + Serialize + serde::de::DeserializeOwned>(db: &SchemaDb, value: T) {
    let data = serde_json::to_value(value).expect("fixture serializes");
    let mut diagnostics = Vec::new();
    Validator::new(db, &mut diagnostics).value(
        &ara_schema::ValueType::Named(T::describe().name),
        None,
        &data,
        "",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let defaults: T = serde_json::from_value(serde_json::Value::Object(
        T::describe()
            .fields()
            .iter()
            .map(|f| (f.name.to_owned(), db.field_default(f)))
            .collect(),
    ))
    .expect("schema defaults deserialize into their Rust type");
    assert_eq!(
        data,
        serde_json::to_value(defaults).expect("defaults serialize"),
        "{} defaults",
        T::describe().name
    );
}
#[test]
fn schemas_verify_and_defaults_match_serde() {
    let mut db = SchemaDb::new();
    register_schemas(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    defaults(&db, PhysicsId::default());
    defaults(&db, PhysicsPose::default());
    defaults(&db, RigidBody::default());
    defaults(&db, Collider::default());
    defaults(&db, Velocity::default());
    defaults(&db, CharacterController::default());
    defaults(&db, CharacterInput::default());
    defaults(&db, Vehicle::default());
    defaults(&db, VehicleInput::default());
    defaults(&db, Joint::default());
    defaults(&db, CharacterMotion::default());
    defaults(&db, VehicleTelemetry::default());
    assert_eq!(
        db.get("CharacterController")
            .expect("registered")
            .field("height")
            .expect("height")
            .unit,
        Some(ara_schema::Unit::Meters)
    );
}
