//! MessagePack binary buffers instead of one integer marker per dependency byte.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

pub(super) fn serialize<S: Serializer>(
    values: &BTreeMap<String, Vec<u8>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    values
        .iter()
        .map(|(key, value)| (key, serde_bytes::Bytes::new(value)))
        .collect::<BTreeMap<_, _>>()
        .serialize(serializer)
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<u8>>, D::Error> {
    Ok(
        BTreeMap::<String, serde_bytes::ByteBuf>::deserialize(deserializer)?
            .into_iter()
            .map(|(key, value)| (key, value.into_vec()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Serialize, Deserialize)]
    struct Binary(#[serde(with = "super")] BTreeMap<String, Vec<u8>>);

    #[test]
    fn binary_frames_accept_legacy_arrays_and_preserve_json() {
        let values = BTreeMap::from([("image.png".into(), vec![200; 4096])]);
        let old = rmp_serde::to_vec(&values).expect("legacy frame");
        let binary = rmp_serde::to_vec(&Binary(values.clone())).expect("binary frame");
        assert_eq!(
            rmp_serde::from_slice::<Binary>(&binary)
                .expect("binary decode")
                .0,
            values
        );
        assert!(binary.len() < old.len() * 3 / 4);
        let legacy = rmp_serde::to_vec(&values).expect("legacy wrapper");
        assert_eq!(
            rmp_serde::from_slice::<Binary>(&legacy)
                .expect("old decode")
                .0,
            values
        );
        assert_eq!(
            serde_json::to_value(Binary(values.clone())).expect("json"),
            serde_json::to_value(values).expect("legacy json")
        );
    }
}
