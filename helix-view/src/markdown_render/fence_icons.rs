//! Language icons on the opening fences of Markdown code blocks, looked up in
//! `[icons.fs]`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use helix_core::syntax::config::FileType;
use helix_core::syntax::Loader;
use helix_core::Language;
use helix_stdx::rope::RopeSliceExt;

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
    /// or no icon is found, see [`lookup`].
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

/// The first icon of `language`'s file-type extensions.
fn by_language(icons: &Icons, loader: &Loader, language: Language) -> Option<FenceIcon> {
    loader
        .language(language)
        .config()
        .file_types
        .iter()
        .find_map(|file_type| match file_type {
            FileType::Extension(extension) => by_extension(icons, extension),
            FileType::Glob(_) => None,
        })
}

/// The language with `name` as an exact file type: an extension, or a glob
/// without wildcards (a file name).
fn language_for_file_type(loader: &Loader, name: &str) -> Option<Language> {
    loader.languages().find_map(|(language, data)| {
        data.config()
            .file_types
            .iter()
            .any(|file_type| match file_type {
                FileType::Extension(extension) => extension == name,
                FileType::Glob(glob) => glob.glob() == name,
            })
            .then_some(language)
    })
}

/// The language whose `injection-regex` matches all of `name`. Unlike
/// [`Loader::language_for_match`], which highlighting uses, a match of part
/// of `name` (`tex` in `text`) does not count.
fn language_for_whole_match(loader: &Loader, name: &str) -> Option<Language> {
    let input = helix_core::RopeSlice::from(name);
    loader.languages().find_map(|(language, data)| {
        let regex = data.config().injection_regex.as_ref()?;
        let found = regex.find(input.regex_input())?;
        (found.start() == 0 && found.end() == name.len()).then_some(language)
    })
}

/// The icon of the fence `language`, matched as a whole: as a file
/// extension, a language name, a language's file type or by a language's
/// `injection-regex`, in that order.
fn lookup(icons: &Icons, language: &str, loader: &Loader) -> Option<FenceIcon> {
    icons.fs().file()?;
    by_extension(icons, language).or_else(|| {
        let language = loader
            .language_for_name(language)
            .or_else(|| language_for_file_type(loader, language))
            .or_else(|| language_for_whole_match(loader, language))?;
        by_language(icons, loader, language)
    })
}
