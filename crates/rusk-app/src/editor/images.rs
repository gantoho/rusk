//! Local image loading and texture caching for image blocks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions, Vec2};

#[derive(Default)]
pub struct ImageCache {
    loaded: HashMap<PathBuf, LoadedImage>,
    failed: HashSet<PathBuf>,
}

struct LoadedImage {
    modified: Option<SystemTime>,
    texture: TextureHandle,
    size: Vec2,
}

pub struct CachedImage {
    pub texture_id: egui::TextureId,
    pub size: Vec2,
}

impl ImageCache {
    pub fn load(&mut self, ctx: &egui::Context, path: &Path) -> Option<CachedImage> {
        if self.failed.contains(path) {
            return None;
        }
        let modified = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        if let Some(entry) = self.loaded.get(path)
            && entry.modified == modified
        {
            return Some(CachedImage {
                texture_id: entry.texture.id(),
                size: entry.size,
            });
        }

        let dyn_image = match image::open(path) {
            Ok(image) => image,
            Err(_) => {
                self.failed.insert(path.to_path_buf());
                return None;
            }
        };
        // Keep GPU memory bounded while preserving enough detail for a page.
        let max_side = 1800u32;
        let (w, h) = (dyn_image.width(), dyn_image.height());
        let dyn_image = if w.max(h) > max_side {
            let scale = max_side as f32 / w.max(h) as f32;
            dyn_image.resize(
                (w as f32 * scale).round().max(1.0) as u32,
                (h as f32 * scale).round().max(1.0) as u32,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            dyn_image
        };
        let rgba = dyn_image.to_rgba8();
        let size = Vec2::new(rgba.width() as f32, rgba.height() as f32);
        let image = ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        );
        let name = format!("rusk-image:{}", path.display());
        let texture = ctx.load_texture(name, image, TextureOptions::LINEAR);
        let entry = LoadedImage {
            modified,
            texture,
            size,
        };
        let texture_id = entry.texture.id();
        self.loaded.insert(path.to_path_buf(), entry);
        Some(CachedImage { texture_id, size })
    }
}
