use std::{collections::BTreeMap, fmt::Write, path::Path};

use crate::Res;

#[derive(Debug, PartialEq)]
enum Part {
    Lit(String),
    Port(String),
    Env(String, Option<String>),
    Root,
    State,
    Service,
}

#[derive(Debug)]
pub struct Template {
    path: String,
    parts: Vec<Part>,
}

pub struct Ctx<'a> {
    pub ports: &'a BTreeMap<String, u16>,
    pub root: &'a Path,
    pub state: &'a Path,
    pub service: &'a str,
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

impl Template {
    /// `path` is the config key the source came from, used in error messages.
    pub fn parse(src: &str, path: &str) -> Res<Self> {
        let err = |m: String| format!("{path}: {m}");
        let mut parts = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("{{") {
            if i > 0 {
                parts.push(Part::Lit(rest[..i].to_string()));
            }
            let after = &rest[i + 2..];
            let j = after
                .find("}}")
                .ok_or_else(|| err("unclosed `{{`".into()))?;
            parts.push(parse_ref(after[..j].trim()).map_err(err)?);
            rest = &after[j + 2..];
        }
        if !rest.is_empty() {
            parts.push(Part::Lit(rest.to_string()));
        }
        Ok(Self {
            path: path.to_string(),
            parts,
        })
    }

    pub fn ports(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(|p| match p {
            Part::Port(n) => Some(n.as_str()),
            _ => None,
        })
    }

    pub fn render(&self, ctx: &Ctx) -> Res<String> {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Lit(s) => out.push_str(s),
                Part::Port(n) => match ctx.ports.get(n) {
                    Some(p) => write!(out, "{p}").unwrap(),
                    None => return Err(format!("{}: port `{n}` was not allocated", self.path)),
                },
                Part::Env(n, default) => match ((ctx.env)(n), default) {
                    (Some(v), _) => out.push_str(&v),
                    (None, Some(d)) => out.push_str(d),
                    (None, None) => {
                        return Err(format!(
                            "{}: environment variable `{n}` is not set",
                            self.path
                        ));
                    }
                },
                Part::Root => out.push_str(&ctx.root.to_string_lossy()),
                Part::State => out.push_str(&ctx.state.to_string_lossy()),
                Part::Service => out.push_str(ctx.service),
            }
        }
        Ok(out)
    }
}

fn parse_ref(inner: &str) -> Result<Part, String> {
    match inner {
        "root" => return Ok(Part::Root),
        "state" => return Ok(Part::State),
        "service" => return Ok(Part::Service),
        _ => {}
    }
    if let Some(name) = inner.strip_prefix("ports.") {
        let name = name.trim();
        if !valid_name(name) {
            return Err(format!(
                "invalid port name `{name}` (use letters, digits, `_`, `-`)"
            ));
        }
        return Ok(Part::Port(name.to_string()));
    }
    if let Some(rest) = inner.strip_prefix("env.") {
        let (name, default) = match rest.split_once('?') {
            Some((n, d)) => (n.trim(), Some(d.trim().to_string())),
            None => (rest.trim(), None),
        };
        if name.is_empty() {
            return Err("empty environment variable name".into());
        }
        return Ok(Part::Env(name.to_string(), default));
    }
    Err(format!("unknown reference `{{{{{inner}}}}}`"))
}

pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(src: &str, env: &dyn Fn(&str) -> Option<String>) -> Res<String> {
        let ports = BTreeMap::from([("web".to_string(), 8000)]);
        let ctx = Ctx {
            ports: &ports,
            root: Path::new("/r"),
            state: Path::new("/r/.ads"),
            service: "api",
            env,
        };
        Template::parse(src, "k")?.render(&ctx)
    }

    #[test]
    fn renders_all_refs() {
        let env = |n: &str| (n == "HOME").then(|| "/home".to_string());
        assert_eq!(
            render(
                "{{root}} {{ state }} {{service}} :{{ports.web}} {{env.HOME}} {{env.NOPE ? x y}}",
                &env
            )
            .unwrap(),
            "/r /r/.ads api :8000 /home x y"
        );
    }

    #[test]
    fn plain_string() {
        assert_eq!(render("no refs", &|_| None).unwrap(), "no refs");
    }

    #[test]
    fn errors() {
        let e = |s: &str| Template::parse(s, "services.a.cmd").unwrap_err();
        assert_eq!(e("x {{ports.web"), "services.a.cmd: unclosed `{{`");
        assert_eq!(e("{{foo}}"), "services.a.cmd: unknown reference `{{foo}}`");
        assert!(e("{{ports.a b}}").contains("invalid port name"));
        assert!(e("{{env.}}").contains("empty environment"));
        assert_eq!(
            render("{{env.NOPE}}", &|_| None).unwrap_err(),
            "k: environment variable `NOPE` is not set"
        );
    }

    #[test]
    fn collects_ports() {
        let t = Template::parse("{{ports.a}}:{{ports.b}}:{{ports.a}}", "k").unwrap();
        assert_eq!(t.ports().collect::<Vec<_>>(), ["a", "b", "a"]);
    }
}
