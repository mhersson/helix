use crate::keymap::{self};
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::commands::custom::CustomTypableCommand;
use helix_view::{document::Mode, theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use std::sync::Arc;
use toml::de::Error as TomlError;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub editor: helix_view::editor::Config,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<HashMap<Mode, KeyTrie>>,
    pub editor: Option<toml::Value>,
    commands: Option<Commands>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            editor: helix_view::editor::Config::default(),
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
struct Commands {
    #[serde(flatten)]
    commands: HashMap<String, CustomTypableCommand>,
}

impl Commands {
    /// Adds the `key` of the command as the `name` and checks for the `hidden` status
    /// and adds it to the `CustomTypableCommand`.
    fn process(mut self) -> Self {
        for (key, value) in &mut self.commands {
            value.name = key.trim_start_matches(':').to_string();
            value.hidden = !key.starts_with(':');
        }
        self
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
}

impl Default for ConfigLoadError {
    fn default() -> Self {
        ConfigLoadError::Error(IOError::new(std::io::ErrorKind::NotFound, "place holder"))
    }
}

impl Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::BadConfig(err) => err.fmt(f),
            ConfigLoadError::Error(err) => err.fmt(f),
        }
    }
}

impl Config {
    pub fn load(
        global: Result<&String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<Config, ConfigLoadError> {
        let global_config: Result<ConfigRaw, ConfigLoadError> =
            global.and_then(|file| toml::from_str(file).map_err(ConfigLoadError::BadConfig));
        let local_config: Result<ConfigRaw, ConfigLoadError> =
            local.and_then(|file| toml::from_str(&file).map_err(ConfigLoadError::BadConfig));
        let res = match (global_config, local_config) {
            (Ok(mut global), Ok(local)) => {
                let mut keys = keymap::default();
                if let Some(global_keys) = global.keys {
                    merge_keys(&mut keys, global_keys)
                }
                if let Some(local_keys) = local.keys {
                    merge_keys(&mut keys, local_keys)
                }

                let mut editor = match (global.editor, local.editor) {
                    (None, None) => helix_view::editor::Config::default(),
                    (None, Some(val)) | (Some(val), None) => {
                        val.try_into().map_err(ConfigLoadError::BadConfig)?
                    }
                    (Some(global), Some(local)) => merge_toml_values(global, local, 3)
                        .try_into()
                        .map_err(ConfigLoadError::BadConfig)?,
                };

                // Merge locally defined commands, overwriting global space commands if encountered
                if let Some(lcommands) = local.commands {
                    if let Some(gcommands) = &mut global.commands {
                        for (name, details) in lcommands.commands {
                            gcommands.commands.insert(name, details);
                        }
                    } else {
                        global.commands = Some(lcommands);
                    }
                }

                // If any commands were defined anywhere, add to editor
                if let Some(commands) = global.commands.map(Commands::process) {
                    let mut holder = Vec::with_capacity(commands.commands.len());

                    for (_, command) in commands.commands {
                        holder.push(command);
                    }

                    editor.commands.commands = Arc::from(holder);
                }

                Config {
                    theme: local.theme.or(global.theme),
                    keys,
                    editor,
                }
            }
            // if any configs are invalid return that first
            (_, Err(ConfigLoadError::BadConfig(err)))
            | (Err(ConfigLoadError::BadConfig(err)), _) => {
                return Err(ConfigLoadError::BadConfig(err))
            }
            (Ok(config), Err(_)) | (Err(_), Ok(config)) => {
                let mut keys = keymap::default();
                if let Some(keymap) = config.keys {
                    merge_keys(&mut keys, keymap);
                }

                let mut editor = config.editor.map_or_else(
                    || Ok(helix_view::editor::Config::default()),
                    |val| val.try_into().map_err(ConfigLoadError::BadConfig),
                )?;

                // Add custom commands
                if let Some(commands) = config.commands.map(Commands::process) {
                    let mut holder = Vec::with_capacity(commands.commands.len());

                    for (_, command) in commands.commands {
                        holder.push(command);
                    }

                    editor.commands.commands = Arc::from(holder);
                }

                Config {
                    theme: config.theme,
                    keys,
                    editor,
                }
            }

            // these are just two io errors return the one for the global config
            (Err(err), Err(_)) => return Err(err),
        };

        Ok(res)
    }

    pub fn load_default() -> Result<Config, ConfigLoadError> {
        let global_config =
            fs::read_to_string(helix_loader::config_file()).map_err(ConfigLoadError::Error)?;
        let local_config = fs::read_to_string(helix_loader::workspace_config_file())
            .map_err(ConfigLoadError::Error);

        let phony_config = ConfigLoadError::Error(IOError::other("hacky placeholder"));
        let global_parsed = Config::load(Ok(&global_config), Err(phony_config))?;

        // We need to build a transient `WorkspaceTrust` just to ask whether the workspace is
        // trusted enough to load its `.helix/config.toml`. The persisted-trust file on disk is the
        // source of truth either way; this transient instance has an empty cache and is dropped
        // after the check.
        let trust = helix_loader::workspace_trust::WorkspaceTrust::new(
            (&global_parsed.editor.workspace_trust).into(),
        );
        if trust
            .query_current(helix_loader::workspace_trust::TrustQuery::LocalConfig)
            .is_trusted()
        {
            let mut merged = Config::load(Ok(&global_config), local_config)?;
            // editor.workspace-trust is global/user-scope only. Without this override, a
            // workspace's `.helix/config.toml` could set `level = "insecure"`; once the user trusted
            // *that* workspace, refresh_config would re-load with the override merged in and from
            // then on every subsequent workspace in the session would be implicitly trusted. Pin
            // the gate's own configuration to the global file.
            merged.editor.workspace_trust = global_parsed.editor.workspace_trust;
            Ok(merged)
        } else {
            Ok(global_parsed)
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    impl Config {
        fn load_test(config: &str) -> Result<Config, ConfigLoadError> {
            Config::load(Ok(&config.to_owned()), Err(ConfigLoadError::default()))
        }
    }

    #[test]
    fn parsing_keymaps_config_file() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let sample_keymaps = r#"
            [keys.insert]
            y = "move_line_down"
            S-C-a = "delete_selection"

            [keys.normal]
            A-F12 = "move_next_word_end"
        "#;

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Insert => keymap!({ "Insert mode"
                    "y" => move_line_down,
                    "S-C-a" => delete_selection,
                }),
                Mode::Normal => keymap!({ "Normal mode"
                    "A-F12" => move_next_word_end,
                }),
            },
        );

        assert_eq!(
            Config::load_test(sample_keymaps).unwrap(),
            Config {
                keys,
                ..Default::default()
            }
        );
    }

    #[test]
    fn parsing_menus() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let sample_keymaps = r#"
            [keys.normal]
            f = { f = "file_picker", c = "wclose" }
            b = { label = "buffer", b = "buffer_picker", n = "goto_next_buffer" }
        "#;

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Normal => keymap!({ "Normal mode"
                    "f" => { ""
                        "f" => file_picker,
                        "c" => wclose,
                    },
                    "b" => { "buffer"
                        "b" => buffer_picker,
                        "n" => goto_next_buffer,
                    },
                }),
            },
        );

        assert_eq!(
            Config::load_test(sample_keymaps).unwrap(),
            Config {
                keys,
                ..Default::default()
            }
        );
    }

    #[test]
    fn parsing_typable_commands() {
        use crate::keymap;
        use crate::keymap::MappableCommand;
        use helix_view::document::Mode;
        use helix_view::input::KeyEvent;
        use std::str::FromStr;

        let sample_keymaps = r#"
            [keys.normal]
            o = { label = "Edit Config", command = ":open ~/.config" }
            c = ":buffer-close" 
            h = ["vsplit", "normal_mode", "swap_view_left"]
            j = {command = ["hsplit", "normal_mode", "swap_view_down"], label = "split down"}
            n = { label = "Delete word", command = "@wd" }
        "#;

        let config = Config::load_test(sample_keymaps).unwrap();

        let tree = config.keys.get(&Mode::Normal).unwrap();

        if let keymap::KeyTrie::Node(node) = tree {
            let open_node = node.get(&KeyEvent::from_str("o").unwrap()).unwrap();

            if let keymap::KeyTrie::MappableCommand(MappableCommand::Typable { doc, .. }) =
                open_node
            {
                assert_eq!(doc, "Edit Config");
            } else {
                panic!("Edit Config did not parse to typable command");
            }

            let close_node = node.get(&KeyEvent::from_str("c").unwrap()).unwrap();
            if let keymap::KeyTrie::MappableCommand(MappableCommand::Typable { doc, .. }) =
                close_node
            {
                assert_eq!(doc, "Close the current buffer.");
            } else {
                panic!(":buffer-close command did not parse to typable command");
            }

            let split_left = node.get(&KeyEvent::from_str("h").unwrap()).unwrap();
            if let keymap::KeyTrie::Sequence(label, cmds) = split_left {
                assert_eq!(label, KeyTrie::DEFAULT_SEQUENCE_LABEL);
                assert_eq!(
                    *cmds,
                    vec![
                        MappableCommand::vsplit,
                        MappableCommand::normal_mode,
                        MappableCommand::swap_view_left
                    ]
                );
            }

            let split_down = node.get(&KeyEvent::from_str("j").unwrap()).unwrap();
            if let keymap::KeyTrie::Sequence(label, cmds) = split_down {
                assert_eq!(label, "split down");
                assert_eq!(
                    *cmds,
                    vec![
                        MappableCommand::hsplit,
                        MappableCommand::normal_mode,
                        MappableCommand::swap_view_down
                    ]
                );
            }

            let macro_keys = node.get(&KeyEvent::from_str("n").unwrap()).unwrap();
            if let keymap::KeyTrie::MappableCommand(MappableCommand::Macro { name, keys }) =
                macro_keys
            {
                assert_eq!(name, "Delete word");
                assert_eq!(
                    keys,
                    &vec![
                        KeyEvent::from_str("w").unwrap(),
                        KeyEvent::from_str("d").unwrap()
                    ]
                );
            }
        } else {
            panic!("Config did not parse to trie");
        }
    }

    #[test]
    fn keys_resolve_to_correct_defaults() {
        // From serde default
        let default_keys = Config::load_test("").unwrap().keys;
        assert_eq!(default_keys, keymap::default());

        // From the Default trait
        let default_keys = Config::default().keys;
        assert_eq!(default_keys, keymap::default());
    }

    #[test]
    fn should_deserialize_custom_commands() {
        let config = r#"
[commands]
":wq" = [":write", ":quit"]
":w" = ":write!"
":wcd!" = { commands = [':write! %arg{0}', ':cd %sh{ %arg{0} | path dirname }'], desc = "writes buffer to disk forcefully, then changes to its directory", accepts = "<path>", completer = ":write" }
":0" = { commands = [":goto 1"] }
":static" = "no_op"
":d" = "@100xd"
":foo" = { commands = ["no_op", ":noop"] }

[commands.":touch"]
commands = [":noop %sh{ touch %arg{0} }"]
desc = "creates file at path"
accepts = "<path>"
completer = ":write"
"#;

        if let Err(err) = Config::load_test(config) {
            panic!("{err:#?}")
        }
    }

    #[test]
    #[should_panic]
    fn should_fail_to_deserialize_custom_command_with_macros_in_sequence() {
        let config = r#"
[commands]
":fail" =  { commands = ["@100xd","@100xd"] }
"#;

        Config::load_test(config).unwrap();
    }

    #[test]
    fn custom_commands_coexist_with_keymap_labels() {
        use crate::keymap::MappableCommand;
        use helix_view::input::KeyEvent;
        use std::str::FromStr;

        let config = r#"
[keys.normal.space.f]
label = "File"
s = { label = "Save", command = ":write" }

[commands]
":wq" = [":write", ":quit"]
"#;

        let config = Config::load_test(config).unwrap();

        let space_f = config
            .keys
            .get(&Mode::Normal)
            .unwrap()
            .search(&[
                KeyEvent::from_str("space").unwrap(),
                KeyEvent::from_str("f").unwrap(),
            ])
            .and_then(KeyTrie::node)
            .unwrap();
        assert_eq!(space_f.infobox().title.as_ref(), "File");
        match space_f.get(&KeyEvent::from_str("s").unwrap()) {
            Some(KeyTrie::MappableCommand(MappableCommand::Typable { doc, .. })) => {
                assert_eq!(doc, "Save")
            }
            other => panic!("unexpected binding: {other:?}"),
        }

        let wq = config.editor.commands.get(":wq").unwrap();
        assert_eq!(wq.commands, ["write", "quit"]);
    }
}
