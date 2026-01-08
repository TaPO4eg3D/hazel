use gpui::{App, AppContext, AsyncApp, Entity, Global, ReadGlobal};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait};

use crate::gpui_tokio::Tokio;

use entity::registry::{self, Entity as Registry, Model as RegistryModel};

pub mod entity;

pub struct DBConnectionManager {
    db: DatabaseConnection,
}

impl DBConnectionManager {
    pub async fn new() -> Self {
        let db = Database::connect("sqlite://db.sqlite?mode=rwc")
            .await
            .unwrap();

        db.get_schema_registry("hazel_client::db::entity::*")
            .sync(&db)
            .await
            .unwrap();

        Self { db }
    }

    pub fn get(cx: &mut AsyncApp) -> DatabaseConnection {
        cx.read_global(|this: &Self, _| {
            this.db.clone()
        }).unwrap()
    }

    pub async fn get_registry(db: &DatabaseConnection) -> RegistryModel {
        let item = Registry::find().one(db)
            .await
            .unwrap();

        match item {
            Some(item) => item,
            None => {
                let item = registry::ActiveModel {
                    ..Default::default()
                };
                
                item.insert(db)
                    .await
                    .unwrap()
            }
        }
    }
}

impl Global for DBConnectionManager {}

pub async fn init(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let manager = Tokio::spawn(cx, DBConnectionManager::new())?.await?;

    cx.update(|cx| cx.set_global(manager)).unwrap();

    Ok(())
}

