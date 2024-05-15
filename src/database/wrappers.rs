/*
use std::io::Write;
use diesel::deserialize::FromSql;
use diesel::pg::{Pg, PgValue};
use diesel::{deserialize, serialize};
use diesel::row::NamedRow;
use diesel::serialize::{IsNull, Output, ToSql};
use diesel::sql_types::BigInt;
use image_hasher::ImageHash;
use serenity::all::MessageId;

// Define a custom SQL type for ImageHashWrapper
#[derive(SqlType)]
#[diesel(postgres_type(name = "ImageHashWrapper"))]
pub struct ImageHashWrapperType;

// Define a custom SQL type for MessageIdWrapper
#[derive(SqlType)]
#[diesel(postgres_type(name = "MessageIdWrapper"))]
pub struct MessageIdWrapperType;

// Your existing structs
#[derive(Debug, PartialEq, FromSqlRow, AsExpression, Eq)]
#[diesel(sql_type = ImageHashWrapperType)]
pub struct ImageHashWrapper(pub(crate) ImageHash);

#[derive(Debug, PartialEq, FromSqlRow, AsExpression, Eq, Copy, Clone)]
#[diesel(sql_type = MessageIdWrapperType)]
pub struct MessageIdWrapper(pub(crate) MessageId);

impl ToSql<ImageHashWrapperType, Pg> for ImageHashWrapper {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
        let hash_base64 = self.0.to_base64();
        out.write_all(hash_base64.as_bytes())?;
        Ok(IsNull::No)
    }
}

impl FromSql<ImageHashWrapperType, Pg> for ImageHashWrapper {
    fn from_sql(value: PgValue<'_>) -> deserialize::Result<Self> {
        let hash_str = std::str::from_utf8(value.as_bytes()).unwrap();
        let image_hash = ImageHash::from_base64(hash_str).map_err(|e| format!("Invalid ImageHash: {:?}", e))?;
        Ok(ImageHashWrapper(image_hash))
    }
}

impl ToSql<MessageIdWrapperType, Pg> for MessageIdWrapper {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
        let value = self.get();
        let binding = value.to_string();
        let value_bytes = binding.as_bytes();
        out.write_all(value_bytes)?;
        Ok(IsNull::No)
    }
}

impl FromSql<MessageIdWrapperType, Pg> for MessageIdWrapper {
    fn from_sql(value: PgValue<'_>) -> deserialize::Result<Self> {
        let message_id_value: i64 = FromSql::<BigInt, Pg>::from_sql(value)?;
        let message_id = MessageId::from(message_id_value as u64);
        Ok(MessageIdWrapper(message_id))
    }
}

*/