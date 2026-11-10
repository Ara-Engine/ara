//! Portable snapshots and asset digests use identical lowercase hexadecimal bytes.

#[test]
fn every_byte_round_trips_in_order_with_two_lowercase_digits() {
    let bytes = (0..=255).collect::<Vec<u8>>();
    let expected = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let encoded = ara_play::hex(&bytes);
    assert_eq!(encoded, expected);
    assert_eq!(
        ara_play::unhex_bytes(&encoded).expect("hex round trip"),
        bytes
    );
    assert_eq!(ara_play::hex(&[]), "");
}
