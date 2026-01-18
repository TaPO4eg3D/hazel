use anyhow::anyhow;
use gpui::*;
use gpui_component::IconNamed;
use client_macros::IconPath;
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

#[derive(IconPath)]
pub enum IconName {
    Lock,
    Server,
    Loader,
    Eye,
    Mic,
    MicOff,
    Headphones,
    HeadphoneOff,
    Cast,
    Hash,
    MessageCircleOff,
    MessageCircle,
    ChevronsDownUp,
    ChevronsUpDown,
    AudioLines,
    Settings,
    User,
    Users,
    EllipsisVertical,
    VolumeFull
}
