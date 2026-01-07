use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "registry")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub session_key: Option<Vec<u8>>,
    pub connected_server: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
