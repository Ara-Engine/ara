# ara-json

Strict JSON in, canonical JSON out. Every Ara text format (scenes, prefabs,
`reflection.json`, JSON Schema, CLI `--json` output) is written through
`to_canonical_string`, so output is byte-stable.

## Public API

- `parse(&str) -> Result<Value, ParseError>`: RFC 8259, rejects duplicate keys;
  `ParseError { message, line, column, duplicate_key }`.
- `to_canonical_string(&Value) -> String`: 2-space indent, LF, trailing newline,
  map keys in insertion order, scalar-only arrays inline (`[0, 1, 0]`).
- `format_f64` / `format_number`: shortest round-trip; integral floats print as
  integers below 2^53; `-0` prints `0`.
- `json_eq`: numeric-aware (`1 == 1.0`), key-order-free equality.
- `sort_keys`: recursively sorted copy, for values without a schema order.
- Re-exports `Value`, `Map`, `Number` from serde_json.

## Gotchas

- The workspace enables serde_json `preserve_order`: key order is whatever was
  inserted. Callers (the schema canonicalizer) insert in reflection order.
  With `preserve_order`, `Map::remove` is `swap_remove`; use `shift_remove` where
  order matters.
- `float_roundtrip` is enabled so parse → write is exact.
- Arrays of strings are written one item per line (merge-friendly), arrays of
  numbers/bools/nulls inline.

## Testing

`cargo test -p ara-json`: layout, numbers, duplicate keys, and property tests
(float round-trip; canonical text is a fixed point).
