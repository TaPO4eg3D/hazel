use std::marker::PhantomData;

use rpc::models::{common::Id, messages::User};
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub username: String,
    pub password: String,
    pub created_at: DateTime,
    pub banned: bool,
}

impl From<Model> for Id<User> {
    fn from(value: Model) -> Self {
        Id {
            value: value.id,
            _marker: PhantomData,
        }
    }
}

impl ActiveModelBehavior for ActiveModel {}
