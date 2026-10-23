//! Reject oversized input/intent allocations while deserializing untrusted data.
use serde::{
    Deserializer,
    de::{Error, SeqAccess, Visitor},
};
pub(crate) fn bytes<'de, D: Deserializer<'de>, const LIMIT: usize>(
    deserializer: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    struct Bounded<const N: usize>;
    impl<'de, const N: usize> Visitor<'de> for Bounded<N> {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {N} bytes")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<u8>, A::Error> {
            if seq.size_hint().is_some_and(|size| size > N) {
                return Err(A::Error::custom("wire byte limit exceeded"));
            }
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(N));
            while let Some(byte) = seq.next_element()? {
                if out.len() == N {
                    return Err(A::Error::custom("wire byte limit exceeded"));
                }
                out.push(byte);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(Bounded::<LIMIT>)
}
pub(crate) fn text<'de, D: Deserializer<'de>, const LIMIT: usize>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    struct Text<const N: usize>;
    impl<'de, const N: usize> Visitor<'de> for Text<N> {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "text of at most {N} bytes")
        }
        fn visit_str<E: Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.len() > N {
                return Err(E::custom("wire text limit exceeded"));
            }
            Ok(value.to_owned())
        }
        fn visit_string<E: Error>(self, value: String) -> std::result::Result<String, E> {
            if value.len() > N {
                return Err(E::custom("wire text limit exceeded"));
            }
            Ok(value)
        }
    }
    deserializer.deserialize_str(Text::<LIMIT>)
}
