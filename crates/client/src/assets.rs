use anyhow::anyhow;
use gpui::*;
use gpui_component::IconNamed;
use client_macros::IconPath;
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "./assets"]
#[include = "icons/**/*"]
#[include = "fonts/**/*"]
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

impl Assets {
    /// Populate the [`TextSystem`] of the given [`AppContext`] with all `.ttf` fonts in the `fonts` directory.
    pub fn load_fonts(cx: &App) -> anyhow::Result<()> {
        let font_paths = cx.asset_source().list("fonts")?;
        let mut embedded_fonts = Vec::new();

        for font_path in font_paths {
            if font_path.ends_with(".ttf") {
                let font_bytes = cx
                    .asset_source()
                    .load(&font_path)?
                    .expect("Assets should never return None");
                embedded_fonts.push(font_bytes);

                println!("Loaded font: {}", font_path);
            }
        }

        cx.text_system().add_fonts(embedded_fonts)
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
    PhoneOff,
    Headphones,
    HeadphoneOff,
    Cast,
    Hash,
    MessageCircleOff,
    MessageCircle,
    ChevronsDownUp,
    ChevronsUpDown,
    ChevronUp,
    ChevronDown,
    ChevronRight,
    AudioLines,
    Settings,
    User,
    Users,
    EllipsisVertical,
    VolumeFull
}
