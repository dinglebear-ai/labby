//! Stream a request parameter struct into an existing map, omitting `_meta`.
//! This avoids RMCP WithMeta's intermediate serde_json::Value allocation.

use serde::ser::{Impossible, SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};

#[path = "relay_task_pending_value.rs"]
mod value;
use value::ValueWire;

#[derive(Clone, Copy)]
pub(super) enum FieldMode {
    Last,
    Only(&'static str),
    Except(Option<&'static str>),
}

pub(super) struct ParamsFields<'a, M> {
    pub(super) map: &'a mut M,
    pub(super) mode: FieldMode,
    pub(super) last: Option<&'static str>,
}

fn unsupported<E: serde::ser::Error>() -> E {
    E::custom("pending task request parameters must serialize as a struct")
}

macro_rules! unsupported_scalar {
    ($($method:ident($value:ty)),* $(,)?) => {
        $(fn $method(self, _value: $value) -> Result<Self::Ok, Self::Error> {
            Err(unsupported())
        })*
    };
}

impl<M: SerializeMap> Serializer for ParamsFields<'_, M> {
    type Ok = Option<&'static str>;
    type Error = M::Error;
    type SerializeSeq = Impossible<Self::Ok, M::Error>;
    type SerializeTuple = Impossible<Self::Ok, M::Error>;
    type SerializeTupleStruct = Impossible<Self::Ok, M::Error>;
    type SerializeTupleVariant = Impossible<Self::Ok, M::Error>;
    type SerializeMap = Impossible<Self::Ok, M::Error>;
    type SerializeStruct = Self;
    type SerializeStructVariant = Impossible<Self::Ok, M::Error>;

    unsupported_scalar! {
        serialize_bool(bool), serialize_i8(i8), serialize_i16(i16),
        serialize_i32(i32), serialize_i64(i64), serialize_i128(i128),
        serialize_u8(u8), serialize_u16(u16), serialize_u32(u32),
        serialize_u64(u64), serialize_u128(u128), serialize_f32(f32),
        serialize_f64(f64), serialize_char(char), serialize_str(&str),
        serialize_bytes(&[u8]),
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(unsupported())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(unsupported())
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(unsupported())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(unsupported())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(unsupported())
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(unsupported())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(unsupported())
    }
}

impl<M: SerializeMap> SerializeStruct for ParamsFields<'_, M> {
    type Ok = Option<&'static str>;
    type Error = M::Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if key != "_meta" {
            self.last = Some(key);
            let emit = match self.mode {
                FieldMode::Last => false,
                FieldMode::Only(selected) => key == selected,
                FieldMode::Except(skipped) => Some(key) != skipped,
            };
            if emit {
                self.map.serialize_entry(key, &ValueWire(value))?;
            }
        }
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.last)
    }
}
