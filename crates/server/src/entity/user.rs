use rpc::{models::markers, tag_entity};

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

tag_entity!(Model, markers::User);

impl ActiveModelBehavior for ActiveModel {}
