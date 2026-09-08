//! Local client configuration. The printer profile lives here and not in
//! the database because the printer belongs to this machine.

use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;
use taskologic_print::DeviceProfile;

use crate::ui::theme::ColorMode;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientConfig {
    pub socket_path: PathBuf,
    /// Higher wins when the same user has several clients connected.
    pub print_priority: i32,
    /// No printer section means this client cannot print.
    pub printer: Option<DeviceProfile>,
    /// Force ASCII box drawing. Unset means detect from the locale.
    pub ascii: Option<bool>,
    /// Force a colour depth. Unset means detect from TERM and NO_COLOR.
    pub color: Option<ColorMode>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/taskologic/taskologicd.sock"),
            print_priority: 0,
            printer: None,
            ascii: None,
            color: None,
        }
    }
}

impl ClientConfig {
    pub fn has_printer(&self) -> bool {
        self.printer
            .as_ref()
            .is_some_and(|p| !p.queue.trim().is_empty())
    }

    pub fn color_mode(&self) -> ColorMode {
        self.color.unwrap_or_else(ColorMode::detect)
    }

    pub fn ascii(&self) -> bool {
        self.ascii.unwrap_or_else(|| {
            let lang = ["LC_ALL", "LC_CTYPE", "LANG"]
                .iter()
                .filter_map(|k| std::env::var(k).ok())
                .find(|v| !v.is_empty())
                .unwrap_or_default()
                .to_ascii_lowercase();
            !(lang.contains("utf-8") || lang.contains("utf8"))
        })
    }
}

/// First file that exists wins: `$TASKOLOGIC_CONFIG`, the user's
/// `~/.config/taskologic/client.toml`, then `/etc/taskologic/client.toml`.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("TASKOLOGIC_CONFIG") {
        out.push(PathBuf::from(p));
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(dir) = xdg {
        out.push(dir.join("taskologic").join("client.toml"));
    }
    out.push(PathBuf::from("/etc/taskologic/client.toml"));
    out
}

pub fn load() -> anyhow::Result<(ClientConfig, Option<PathBuf>)> {
    for path in candidates() {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let cfg =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                return Ok((cfg, Some(path)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
    Ok((ClientConfig::default(), None))
}

/// Write the printer profile back to the config file. `loaded` is where the
/// config was read from; with none loaded the user's own path is created.
/// The rest of the file is kept as values, but comments do not survive a
/// rewrite, which the header line owns up to.
pub fn save_printer(
    loaded: Option<&std::path::Path>,
    printer: Option<&DeviceProfile>,
) -> anyhow::Result<PathBuf> {
    let path = match loaded {
        Some(p) => p.to_path_buf(),
        None => candidates()
            .into_iter()
            .find(|p| p != std::path::Path::new("/etc/taskologic/client.toml"))
            .ok_or_else(|| {
                anyhow::anyhow!("no writable config location, set TASKOLOGIC_CONFIG or HOME")
            })?,
    };
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => text
            .parse()
            .with_context(|| format!("parsing {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    match printer {
        Some(p) => {
            table.insert(
                "printer".into(),
                toml::Value::try_from(p).context("encoding the printer profile")?,
            );
        }
        None => {
            table.remove("printer");
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut text =
        String::from("# Rewritten by Taskologic settings. Comments are not preserved on save.\n");
    text.push_str(&toml::to_string_pretty(&table).context("encoding the client config")?);
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printer_section_is_optional() {
        let c: ClientConfig = toml::from_str("print_priority = 5\n").unwrap();
        assert!(!c.has_printer());
        let c: ClientConfig =
            toml::from_str("color = \"mono\"\n[printer]\nqueue = \"receipt\"\npaper = \"mm80\"\n")
                .unwrap();
        assert_eq!(c.color_mode(), ColorMode::Mono);
        assert!(c.has_printer());
        assert_eq!(c.printer.unwrap().paper.columns(), 48);
        assert!(toml::from_str::<ClientConfig>("socket = 1").is_err());
    }
}
