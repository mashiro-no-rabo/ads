use std::{collections::BTreeMap, fs, path::Path, process::Command};

use crate::{
    Res, config, ports,
    state::State,
    template::{Ctx, Template},
};

pub fn cmd(config_path: &Path, name: Option<&str>, all: bool) -> Res<()> {
    let cfg = config::load(config_path)?;
    let entries = select(&cfg.open, name, all)?;
    let state = State::new(&cfg.root);
    let names: std::collections::BTreeSet<_> =
        entries.iter().flat_map(|(_, t)| t.ports()).collect();
    let mut assigned = BTreeMap::new();
    if !names.is_empty() {
        if state.daemon_pid().is_none() {
            return Err("not running; run `ads up` before opening templated ports".into());
        }
        let text =
            fs::read_to_string(state.file("ports.env")).map_err(|e| format!("ports.env: {e}"))?;
        let env: BTreeMap<_, _> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        for n in names {
            let key = ports::env_name(n);
            let port = env
                .get(key.as_str())
                .and_then(|p| p.parse::<u16>().ok())
                .filter(|&p| p != 0)
                .ok_or_else(|| format!("port `{n}` was not allocated; restart with `ads up`"))?;
            assigned.insert(n.to_string(), port);
        }
    }
    // Resolve everything first, so a template error cannot partially open --all.
    let urls = render(&cfg, &state, &assigned, entries)?;
    for url in urls {
        let status = Command::new("open")
            .arg("--")
            .arg(&url)
            .status()
            .map_err(|e| format!("open {url:?}: {e}"))?;
        if !status.success() {
            return Err(format!("open {url:?}: {status}"));
        }
    }
    Ok(())
}

pub fn render(
    cfg: &config::Config,
    state: &State,
    assigned: &BTreeMap<String, u16>,
    entries: Vec<(&str, &Template)>,
) -> Res<Vec<String>> {
    let lookup = |n: &str| std::env::var(n).ok();
    entries
        .into_iter()
        .map(|(name, t)| {
            let ctx = Ctx {
                ports: assigned,
                root: &cfg.root,
                state: &state.dir,
                service: name,
                env: &lookup,
            };
            url(&t.render(&ctx)?, name)
        })
        .collect()
}

fn select<'a>(
    entries: &'a [(String, Template)],
    name: Option<&str>,
    all: bool,
) -> Res<Vec<(&'a str, &'a Template)>> {
    if entries.is_empty() {
        return Err("no entries defined in [open]".into());
    }
    if let Some(name) = name {
        return entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(n, t)| vec![(n.as_str(), t)])
            .ok_or_else(|| format!("unknown open name `{name}`"));
    }
    if !all {
        let (name, template) = &entries[0];
        return Ok(vec![(name.as_str(), template)]);
    }
    Ok(entries.iter().map(|(n, t)| (n.as_str(), t)).collect())
}

fn url(rendered: &str, name: &str) -> Res<String> {
    if !rendered.is_empty() && rendered.bytes().all(|b| b.is_ascii_digit()) {
        let port = rendered
            .parse::<u16>()
            .ok()
            .filter(|&p| p != 0)
            .ok_or_else(|| format!("open.{name}: port must be between 1 and 65535"))?;
        return Ok(format!("http://localhost:{port}"));
    }
    if rendered.trim().is_empty() {
        return Err(format!("open.{name}: URL must not be empty"));
    }
    Ok(rendered.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_entries() {
        let entries = vec![
            ("web".into(), Template::parse("3000", "open.web").unwrap()),
            (
                "docs".into(),
                Template::parse("https://example.com", "open.docs").unwrap(),
            ),
        ];
        assert!(select(&[], None, true).unwrap_err().contains("no entries"));
        let default = select(&entries, None, false).unwrap();
        assert_eq!(default.len(), 1);
        assert_eq!(default[0].0, "web");
        assert!(select(&entries, Some("missing"), false).is_err());
        assert_eq!(select(&entries, Some("docs"), false).unwrap()[0].0, "docs");
        assert_eq!(select(&entries, None, true).unwrap().len(), 2);
        assert_eq!(select(&entries[..1], None, false).unwrap()[0].0, "web");
    }

    #[test]
    fn ports_become_local_urls() {
        assert_eq!(url("3000", "web").unwrap(), "http://localhost:3000");
        assert_eq!(
            url("https://example.com", "docs").unwrap(),
            "https://example.com"
        );
        for invalid in ["0", "65536", "", " "] {
            assert!(url(invalid, "web").is_err());
        }
    }
}
