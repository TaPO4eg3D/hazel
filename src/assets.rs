use anyhow::anyhow;
use gpui::*;
use gpui_component::IconNamed;
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "./assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;


impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

pub enum IconName {
    UserAvatar,
    PasswordLock,
    Server,
    Loader,
    Eye,
    Hash,
    MessageCircleOff,
    MessageCircle,
    ChevronsDownUp,
    ChevronsUpDown,
}

impl IconNamed for IconName {
    fn path(self) -> SharedString {
        match self {
            IconName::Hash => "icons/hash.svg",
            IconName::MessageCircle => "icons/message-circle.svg",
            IconName::MessageCircleOff => "icons/message-circle-off.svg",
            IconName::ChevronsDownUp => "icons/chevrons-down-up.svg",
            IconName::ChevronsUpDown => "icons/chevrons-up-down.svg",
            IconName::UserAvatar => "icons/user.svg",
            IconName::PasswordLock => "icons/lock.svg",
            IconName::Server => "icons/server.svg",
            IconName::Eye => "icons/eye.svg",
            IconName::Loader => "icons/loader.svg",
        }.into()
    }
}
