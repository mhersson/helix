//! Language icons on the opening fences of Markdown code blocks, looked up in
//! `[icons.fs]`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use helix_core::syntax::Loader;

use crate::icons::{Icon, Icons};

/// The icon of a fence language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceIcon {
    /// The glyph followed by one space, the replacement of the opening
    /// fence's delimiter.
    pub replacement: String,
    /// The file extension the icon was found for, which also picks its
    /// theme style (`icons.file.<extension>`).
    pub extension: String,
}

/// Looks up and caches the [`FenceIcon`] of each fence language.
#[derive(Debug)]
pub struct FenceIcons {
    icons: Arc<Icons>,
    loader: Arc<Loader>,
    cache: HashMap<String, Option<FenceIcon>>,
}

impl FenceIcons {
    pub fn new(icons: Arc<Icons>, loader: Arc<Loader>) -> Self {
        Self {
            icons,
            loader,
            cache: HashMap::new(),
        }
    }

    /// Whether the cache was filled from `icons` and `loader`.
    pub fn is_for(&self, icons: &Arc<Icons>, loader: &Arc<Loader>) -> bool {
        Arc::ptr_eq(&self.icons, icons) && Arc::ptr_eq(&self.loader, loader)
    }

    /// The icon of the fence `language`, or `None` when `[icons.fs]` is off
    /// or no icon is found: first `language` as a file extension, then the
    /// file-type extensions, in order, of the language that code blocks of
    /// `language` are highlighted with.
    pub fn get(&mut self, language: &str) -> Option<FenceIcon> {
        if let Some(icon) = self.cache.get(language) {
            return icon.clone();
        }
        let icon = lookup(&self.icons, language, &self.loader);
        self.cache.insert(language.to_string(), icon.clone());
        icon
    }
}

fn by_extension(icons: &Icons, extension: &str) -> Option<FenceIcon> {
    // a separator or a dot would not survive as a path's extension
    if extension.is_empty() || extension.contains(['.', '/', '\\']) {
        return None;
    }
    let icon: Icon = icons
        .fs()
        .file()?
        .get_by_extension(Path::new(&format!("_.{extension}")))?;
    if icon.is_empty() {
        return None;
    }
    Some(FenceIcon {
        replacement: icon.with_padding(0, 1).glyph().to_string(),
        extension: extension.to_string(),
    })
}

fn lookup(icons: &Icons, language: &str, loader: &Loader) -> Option<FenceIcon> {
    icons.fs().file()?;
    by_extension(icons, language).or_else(|| {
        let language = loader.language_for_match(helix_core::RopeSlice::from(language))?;
        loader
            .language(language)
            .config()
            .file_types
            .iter()
            .find_map(|file_type| match file_type {
                helix_core::syntax::config::FileType::Extension(extension) => {
                    by_extension(icons, extension)
                }
                helix_core::syntax::config::FileType::Glob(_) => None,
            })
    })
}
