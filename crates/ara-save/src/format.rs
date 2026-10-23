use crate::{PersistentState, Result, SaveSchema, StateHash, codes, error};
use ara_diag::Diagnostic;
use ara_schema::Schema;
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
};

const MAGIC: &[u8; 8] = b"ARASAVE\0";
const VERSION: u16 = 1;
const HEADER: usize = 60;

/// Cloud-visible slot metadata. Timestamp/playtime come from the host, never simulation.
#[derive(Debug, Clone, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "SaveSlotMetadata", module = "save")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SlotMetadata {
    /// Portable slot identifier, restricted by SlotStore to a safe single path segment.
    pub slot: String,
    /// UTC Unix timestamp in milliseconds, supplied by the host clock.
    pub timestamp_millis: u64,
    /// Accumulated game playtime in milliseconds; excludes paused/background time.
    pub playtime_millis: u64,
    /// Optional thumbnail asset reference (`@path#id`), prepared by a capture hook.
    pub thumbnail: Option<ara_schema::AssetRef>,
    /// Human-readable slot name, independent of its storage identifier.
    pub label: String,
}

/// Uncompressed, versioned save payload; metadata is covered by the envelope checksum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveGame {
    /// Slot metadata suitable for cloud conflict/version listings.
    pub metadata: SlotMetadata,
    /// Exact versions of persistent types, independent of network/replay schemas.
    pub versions: BTreeMap<String, u32>,
    /// Only explicitly persistent component/resource state plus the engine cursor.
    pub state: PersistentState,
    /// Persistent projection hash before saving, verified before any migration.
    pub state_hash: StateHash,
}

impl SaveGame {
    /// Builds an owned worker-ready save from a tick-boundary capture.
    pub fn capture(
        schema: &SaveSchema,
        session: &ara_sim::Session,
        metadata: SlotMetadata,
    ) -> Result<Self> {
        Self::from_state(schema, schema.capture(session)?, metadata)
    }
    /// Builds a save from an already-captured owned projection, without world IO.
    pub fn from_state(
        schema: &SaveSchema,
        mut state: PersistentState,
        metadata: SlotMetadata,
    ) -> Result<Self> {
        schema.validate(&state)?;
        state
            .materialize(schema.db())
            .map_err(|e| error(codes::SCHEMA, e.to_string()))?;
        if metadata
            .thumbnail
            .as_ref()
            .is_some_and(|r| ara_schema::grammar::parse_ref(&r.0).is_err())
        {
            return Err(error(
                codes::SCHEMA,
                "thumbnail must be a standard asset reference",
            ));
        }
        let state_hash = state
            .state_hash()
            .map_err(|e| error(codes::SCHEMA, e.to_string()))?;
        Ok(Self {
            metadata,
            versions: schema.versions(),
            state,
            state_hash,
        })
    }
}

/// Allocation limits applied before reading/decompressing untrusted cloud/local data.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Maximum complete envelope bytes accepted/read from disk.
    pub file_bytes: usize,
    /// Maximum decompressed CBOR bytes; enforced with a bounded streaming decoder.
    pub decoded_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            file_bytes: 64 * 1024 * 1024,
            decoded_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Validated save plus migration diagnostics and cloud conflict identity.
#[derive(Debug)]
pub struct DecodedSave {
    /// Current-schema save, ready for a boundary restore.
    pub game: SaveGame,
    /// Original file identity; changes if metadata or any state changes.
    pub file_hash: StateHash,
    /// Schema upgrade information, including per-version hook diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Hash recorded before migration; the current projection may have changed shape.
    pub original_state_hash: StateHash,
}

/// Encodes a compressed CBOR payload in the ARASAVE v1 little-endian envelope:
/// magic/version/codec/reserved/raw-size/packed-size/BLAKE3, then zlib bytes.
/// The checksum covers the header prefix and compressed payload, including metadata.
pub fn encode(game: &SaveGame, limits: Limits) -> Result<Vec<u8>> {
    if game
        .state
        .state_hash()
        .map_err(|e| error(codes::SCHEMA, e.to_string()))?
        != game.state_hash
    {
        return Err(error(codes::SCHEMA, "save state hash does not match state"));
    }
    let mut raw = Vec::new();
    ciborium::ser::into_writer(game, &mut raw).map_err(|e| error(codes::CORRUPT, e.to_string()))?;
    if raw.len() > limits.decoded_bytes {
        return Err(error(codes::CORRUPT, "save exceeds decoded size limit"));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&raw)
        .map_err(|e| error(codes::IO, e.to_string()))?;
    let packed = encoder
        .finish()
        .map_err(|e| error(codes::IO, e.to_string()))?;
    if packed
        .len()
        .checked_add(HEADER)
        .is_none_or(|n| n > limits.file_bytes)
    {
        return Err(error(codes::CORRUPT, "save exceeds file size limit"));
    }
    let mut bytes = Vec::with_capacity(HEADER + packed.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&(raw.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(packed.len() as u64).to_le_bytes());
    let mut digest = blake3::Hasher::new();
    digest.update(&bytes);
    digest.update(&packed);
    bytes.extend_from_slice(digest.finalize().as_bytes());
    bytes.extend_from_slice(&packed);
    Ok(bytes)
}

/// Verifies bounds, checksum, zlib length/trailing data, strict CBOR payload and
/// pre-migration state hash, then upgrades through the shared Schema DB hooks.
pub fn decode(bytes: &[u8], schema: &SaveSchema, limits: Limits) -> Result<DecodedSave> {
    if bytes.len() < HEADER
        || bytes.len() > limits.file_bytes
        || &bytes[..8] != MAGIC
        || u16::from_le_bytes([bytes[8], bytes[9]]) != VERSION
        || bytes[10..12] != [1, 0]
    {
        return Err(error(
            codes::CORRUPT,
            "invalid save envelope, version, codec or size",
        ));
    }
    let read_len = |start| {
        let mut value = [0; 8];
        value.copy_from_slice(&bytes[start..start + 8]);
        u64::from_le_bytes(value)
    };
    let raw_len =
        usize::try_from(read_len(12)).map_err(|_| error(codes::CORRUPT, "save length overflow"))?;
    let packed_len =
        usize::try_from(read_len(20)).map_err(|_| error(codes::CORRUPT, "save length overflow"))?;
    if raw_len > limits.decoded_bytes || packed_len != bytes.len() - HEADER {
        return Err(error(
            codes::CORRUPT,
            "save size mismatch or limit exceeded",
        ));
    }
    let mut digest = blake3::Hasher::new();
    digest.update(&bytes[..28]);
    digest.update(&bytes[HEADER..]);
    if digest.finalize().as_bytes() != &bytes[28..HEADER] {
        return Err(error(codes::CORRUPT, "save checksum mismatch"));
    }
    let mut decoder = ZlibDecoder::new(&bytes[HEADER..]);
    let mut raw = Vec::new();
    decoder
        .by_ref()
        .take(raw_len as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| error(codes::CORRUPT, format!("save decompression failed: {e}")))?;
    if raw.len() != raw_len || decoder.total_in() != packed_len as u64 {
        return Err(error(
            codes::CORRUPT,
            "save decompression size or trailing bytes mismatch",
        ));
    }
    // Decode through ciborium::Value first to reject duplicate map keys before
    // serde can discard an earlier field. Re-encode only after structural checks.
    let mut cursor = Cursor::new(&raw);
    let value: ciborium::Value =
        ciborium::de::from_reader(&mut cursor).map_err(|e| error(codes::CORRUPT, e.to_string()))?;
    if cursor.position() != raw.len() as u64 {
        return Err(error(codes::CORRUPT, "trailing CBOR save data"));
    }
    check_cbor(&value, 0)?;
    let mut game: SaveGame = value
        .deserialized()
        .map_err(|e| error(codes::CORRUPT, e.to_string()))?;
    if game
        .state
        .state_hash()
        .map_err(|e| error(codes::CORRUPT, e.to_string()))?
        != game.state_hash
    {
        return Err(error(codes::CORRUPT, "persistent state hash mismatch"));
    }
    let original_state_hash = game.state_hash;
    let diagnostics = schema.migrate(&mut game.state, &game.versions)?;
    game.versions = schema.versions();
    game.state_hash = game
        .state
        .state_hash()
        .map_err(|e| error(codes::SCHEMA, e.to_string()))?;
    Ok(DecodedSave {
        game,
        diagnostics,
        file_hash: StateHash::of_bytes(bytes),
        original_state_hash,
    })
}

fn check_cbor(value: &ciborium::Value, depth: usize) -> Result<()> {
    if depth > 64 {
        return Err(error(codes::CORRUPT, "save nesting exceeds 64"));
    }
    match value {
        ciborium::Value::Map(entries) => {
            let mut seen = std::collections::BTreeSet::new();
            for (key, value) in entries {
                let ciborium::Value::Text(key) = key else {
                    return Err(error(codes::CORRUPT, "save maps require string keys"));
                };
                if !seen.insert(key) {
                    return Err(error(codes::CORRUPT, "duplicate CBOR save key"));
                }
                check_cbor(value, depth + 1)?;
            }
        }
        ciborium::Value::Array(values) => {
            for value in values {
                check_cbor(value, depth + 1)?;
            }
        }
        ciborium::Value::Float(value) if !value.is_finite() => {
            return Err(error(codes::CORRUPT, "non-finite saved number"));
        }
        ciborium::Value::Tag(..) => {
            return Err(error(
                codes::CORRUPT,
                "CBOR tags are not part of the save format",
            ));
        }
        _ => {}
    }
    Ok(())
}
