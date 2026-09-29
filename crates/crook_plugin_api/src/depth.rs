//! A decode that counts how deep it has gone, and stops at [`MAX_DEPTH`].
//!
//! serde decodes a value by recursing into it, one call deeper for every list,
//! struct, variant and option it is inside, and postcard sets no limit on how
//! far that goes. A [`Node`](crate::Node) holds other nodes, so a guest can
//! hand the host a tree as deep as its answer is long — and a decode that runs
//! out of stack is not an `Err`, it is an abort of the whole process.
//!
//! [`Nested`] is the whole fix. It wraps each of serde's decoding traits in
//! turn — the deserializer, the visitor it hands a value to, the access a
//! visitor walks a list or a variant with, and the seed an element is read
//! with — so that whatever is decoded *inside* something is decoded through a
//! wrapper told one level more. Every level passes through here on its way
//! down, so the count cannot be skipped, and the level past the limit is an
//! error the format's own caller gets back like any other.
//!
//! # Why a wrapper and not a pre-scan
//!
//! Neither of the two other ways out fits. The usual one grows the stack
//! instead (`serde_stacker`), which needs `psm` and its build script, and this
//! workspace builds none. And a scan over the bytes before decoding them would
//! have to know where every field of every variant ends — a second copy of
//! the [`Node`](crate::Node) schema, written by hand, that goes on compiling
//! after a variant gains a field and then measures the wrong bytes.
//!
//! This one knows nothing about any type. It counts serde's own nesting, which
//! is what a derive emits for whatever the enum says today, so there is no
//! list of variants in it to fall out of step — and it bounds every type the
//! crate decodes, not only the one known to recurse.

use core::fmt;

use alloc::string::String;
use alloc::vec::Vec;

use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};

use crate::MAX_DEPTH;

/// One of serde's decoding traits, and how many things what it decodes is
/// already inside.
///
/// One type for all of them rather than one per trait, because what each
/// does with the count is the same: pass it on unchanged to whatever it hands
/// the next part of the value to, and add one only where a new level opens —
/// which is always a visitor being handed something to walk.
pub(crate) struct Nested<T> {
    inner: T,
    depth: usize,
}

impl<T> Nested<T> {
    /// `inner`, at the top of a value: inside nothing yet.
    pub(crate) fn top(inner: T) -> Self {
        Self { inner, depth: 0 }
    }

    /// Something else, at the same depth as this.
    fn beside<U>(&self, inner: U) -> Nested<U> {
        Nested {
            inner,
            depth: self.depth,
        }
    }

    /// Something else, one level inside this — or the error that says there
    /// is no room for another.
    fn inside<U, E: de::Error>(&self, inner: U) -> Result<Nested<U>, E> {
        if self.depth >= MAX_DEPTH {
            return Err(E::custom(TOO_DEEP));
        }
        Ok(Nested {
            inner,
            depth: self.depth + 1,
        })
    }
}

/// What the refusal says, for a format that keeps what it is told: postcard
/// does not, and its error for this is the one it gives any custom refusal.
const TOO_DEEP: &str = "a value nested deeper than MAX_DEPTH";

/// Asks for a value with a visitor that knows its depth.
macro_rules! asking {
    ($($method:ident($($argument:ident: $type:ty),*)),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(
            self,
            $($argument: $type,)*
            visitor: V,
        ) -> Result<V::Value, D::Error> {
            let visitor = self.beside(visitor);
            self.inner.$method($($argument,)* visitor)
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Nested<D> {
    type Error = D::Error;

    asking!(
        deserialize_any(),
        deserialize_bool(),
        deserialize_i8(),
        deserialize_i16(),
        deserialize_i32(),
        deserialize_i64(),
        deserialize_i128(),
        deserialize_u8(),
        deserialize_u16(),
        deserialize_u32(),
        deserialize_u64(),
        deserialize_u128(),
        deserialize_f32(),
        deserialize_f64(),
        deserialize_char(),
        deserialize_str(),
        deserialize_string(),
        deserialize_bytes(),
        deserialize_byte_buf(),
        deserialize_option(),
        deserialize_unit(),
        deserialize_unit_struct(name: &'static str),
        deserialize_newtype_struct(name: &'static str),
        deserialize_seq(),
        deserialize_tuple(len: usize),
        deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map(),
        deserialize_struct(name: &'static str, fields: &'static [&'static str]),
        deserialize_enum(name: &'static str, variants: &'static [&'static str]),
        deserialize_identifier(),
        deserialize_ignored_any(),
    );

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

/// Hands a visitor a value with nothing inside it.
macro_rules! leaf {
    ($($method:ident($type:ty)),* $(,)?) => {$(
        fn $method<E: de::Error>(self, value: $type) -> Result<V::Value, E> {
            self.inner.$method(value)
        }
    )*};
}

/// The one place a level opens: a visitor is handed something to walk.
///
/// Every container serde has arrives at one of the five methods below that
/// are not leaves — a list or a tuple or a struct's fields as a sequence, a
/// map, a variant, the inside of an option or of a newtype — so a count kept
/// here is a count of every level, whichever of them a type is made of.
impl<'de, V: Visitor<'de>> Visitor<'de> for Nested<V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(formatter)
    }

    leaf!(
        visit_bool(bool),
        visit_i8(i8),
        visit_i16(i16),
        visit_i32(i32),
        visit_i64(i64),
        visit_i128(i128),
        visit_u8(u8),
        visit_u16(u16),
        visit_u32(u32),
        visit_u64(u64),
        visit_u128(u128),
        visit_f32(f32),
        visit_f64(f64),
        visit_char(char),
        visit_str(&str),
        visit_borrowed_str(&'de str),
        visit_string(String),
        visit_bytes(&[u8]),
        visit_borrowed_bytes(&'de [u8]),
        visit_byte_buf(Vec<u8>),
    );

    fn visit_none<E: de::Error>(self) -> Result<V::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<V::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<V::Value, D::Error> {
        let deserializer = self.inside(deserializer)?;
        self.inner.visit_some(deserializer)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<V::Value, D::Error> {
        let deserializer = self.inside(deserializer)?;
        self.inner.visit_newtype_struct(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<V::Value, A::Error> {
        let seq = self.inside(seq)?;
        self.inner.visit_seq(seq)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        let map = self.inside(map)?;
        self.inner.visit_map(map)
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<V::Value, A::Error> {
        let data = self.inside(data)?;
        self.inner.visit_enum(data)
    }
}

/// Reads one part of a value through a deserializer that knows its depth.
impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Nested<S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        let deserializer = self.beside(deserializer);
        self.inner.deserialize(deserializer)
    }
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Nested<A> {
    type Error = A::Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        let seed = self.beside(seed);
        self.inner.next_element_seed(seed)
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Nested<A> {
    type Error = A::Error;

    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        let seed = self.beside(seed);
        self.inner.next_key_seed(seed)
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        let seed = self.beside(seed);
        self.inner.next_value_seed(seed)
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for Nested<A> {
    type Error = A::Error;
    type Variant = Nested<A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Nested<A::Variant>), A::Error> {
        let seed = self.beside(seed);
        let (value, variant) = self.inner.variant_seed(seed)?;
        Ok((
            value,
            Nested {
                inner: variant,
                depth: self.depth,
            },
        ))
    }
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for Nested<A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.inner.unit_variant()
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, A::Error> {
        let seed = self.beside(seed);
        self.inner.newtype_variant_seed(seed)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        let visitor = self.beside(visitor);
        self.inner.tuple_variant(len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        let visitor = self.beside(visitor);
        self.inner.struct_variant(fields, visitor)
    }
}
