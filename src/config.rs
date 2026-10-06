use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use toml::{Table, Value};

use crate::{
    Res, ports,
    template::{self, Ctx, Template},
};

pub const FILE: &str = "ads.toml";

pub struct Config {
    pub root: PathBuf,
    env: Vec<(String, Template)>,
    pub services: Vec<ServiceSpec>,
}

pub struct ServiceSpec {
    pub name: String,
    cmd: CmdSpec,
    cwd: Option<Template>,
    env: Vec<(String, Template)>,
}

enum CmdSpec {
    Shell(Template),
    Argv(Vec<Template>),
}

pub struct Service {
    pub name: String,
    pub cmd: Cmd,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

pub enum Cmd {
    Shell(String),
    Argv(Vec<String>),
}

pub fn find(explicit: Option<PathBuf>) -> Res<PathBuf> {
    if let Some(p) = explicit {
        return p
            .canonicalize()
            .map_err(|e| format!("{}: {e}", p.display()));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    cwd.ancestors()
        .map(|d| d.join(FILE))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("no {FILE} in {} or any parent", cwd.display()))
}

pub fn root_of(config: &Path) -> PathBuf {
    config.parent().unwrap_or(Path::new("/")).to_path_buf()
}

pub fn load(path: &Path) -> Res<Config> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text, root_of(path)).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(text: &str, root: PathBuf) -> Res<Config> {
    let table: Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut env = Vec::new();
    let mut services = Vec::new();
    for (key, value) in &table {
        match key.as_str() {
            "env" => env = parse_env(value, "env")?,
            "services" => {
                let t = value.as_table().ok_or("`services` must be a table")?;
                for (name, v) in t {
                    services.push(parse_service(name, v)?);
                }
            }
            _ => return Err(format!("unknown key `{key}`")),
        }
    }
    if services.is_empty() {
        return Err("no services defined".into());
    }
    Ok(Config {
        root,
        env,
        services,
    })
}

fn parse_service(name: &str, v: &Value) -> Res<ServiceSpec> {
    if !template::valid_name(name) {
        return Err(format!(
            "invalid service name `{name}` (use letters, digits, `_`, `-`)"
        ));
    }
    let path = format!("services.{name}");
    let t = v
        .as_table()
        .ok_or_else(|| format!("`{path}` must be a table"))?;
    let mut cmd = None;
    let mut cwd = None;
    let mut env = Vec::new();
    for (k, v) in t {
        let kp = format!("{path}.{k}");
        match k.as_str() {
            "cmd" => {
                cmd = Some(match v {
                    Value::String(s) => CmdSpec::Shell(Template::parse(s, &kp)?),
                    Value::Array(a) if !a.is_empty() => CmdSpec::Argv(
                        a.iter()
                            .enumerate()
                            .map(|(i, x)| {
                                let ip = format!("{kp}[{i}]");
                                let s = x
                                    .as_str()
                                    .ok_or_else(|| format!("`{ip}` must be a string"))?;
                                Template::parse(s, &ip)
                            })
                            .collect::<Res<_>>()?,
                    ),
                    _ => {
                        return Err(format!(
                            "`{kp}` must be a string or a non-empty array of strings"
                        ));
                    }
                })
            }
            "cwd" => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("`{kp}` must be a string"))?;
                cwd = Some(Template::parse(s, &kp)?);
            }
            "env" => env = parse_env(v, &kp)?,
            _ => return Err(format!("unknown key `{kp}`")),
        }
    }
    Ok(ServiceSpec {
        name: name.to_string(),
        cmd: cmd.ok_or_else(|| format!("`{path}.cmd` is required"))?,
        cwd,
        env,
    })
}

fn parse_env(v: &Value, path: &str) -> Res<Vec<(String, Template)>> {
    let t = v
        .as_table()
        .ok_or_else(|| format!("`{path}` must be a table"))?;
    t.iter()
        .map(|(k, v)| {
            let kp = format!("{path}.{k}");
            if k.is_empty() || k.contains(['=', '\0']) {
                return Err(format!("invalid environment variable name `{kp}`"));
            }
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Integer(i) => i.to_string(),
                Value::Float(f) => f.to_string(),
                Value::Boolean(b) => b.to_string(),
                _ => return Err(format!("`{kp}` must be a string, number or boolean")),
            };
            Ok((k.clone(), Template::parse(&s, &kp)?))
        })
        .collect()
}

impl ServiceSpec {
    fn templates(&self) -> impl Iterator<Item = &Template> {
        let cmd: Vec<&Template> = match &self.cmd {
            CmdSpec::Shell(t) => vec![t],
            CmdSpec::Argv(a) => a.iter().collect(),
        };
        cmd.into_iter()
            .chain(&self.cwd)
            .chain(self.env.iter().map(|(_, t)| t))
    }
}

impl Config {
    pub fn port_names(&self) -> BTreeSet<String> {
        self.env
            .iter()
            .map(|(_, t)| t)
            .chain(self.services.iter().flat_map(|s| s.templates()))
            .flat_map(|t| t.ports())
            .map(str::to_string)
            .collect()
    }

    pub fn render(
        &self,
        ports: &BTreeMap<String, u16>,
        state: &Path,
        only: &[String],
    ) -> Res<Vec<Service>> {
        let lookup = |n: &str| std::env::var(n).ok();
        self.services
            .iter()
            .filter(|s| only.is_empty() || only.contains(&s.name))
            .map(|s| {
                let ctx = Ctx {
                    ports,
                    root: &self.root,
                    state,
                    service: &s.name,
                    env: &lookup,
                };
                let mut env = vec![
                    ("ADS_SERVICE".to_string(), s.name.clone()),
                    ("ADS_ROOT".to_string(), self.root.to_string_lossy().into()),
                    ("ADS_STATE".to_string(), state.to_string_lossy().into()),
                ];
                env.extend(
                    ports
                        .iter()
                        .map(|(n, p)| (ports::env_name(n), p.to_string())),
                );
                for (k, t) in self.env.iter().chain(&s.env) {
                    env.push((k.clone(), t.render(&ctx)?));
                }
                let cmd = match &s.cmd {
                    CmdSpec::Shell(t) => Cmd::Shell(t.render(&ctx)?),
                    CmdSpec::Argv(a) => {
                        Cmd::Argv(a.iter().map(|t| t.render(&ctx)).collect::<Res<_>>()?)
                    }
                };
                let cwd = match &s.cwd {
                    Some(t) => self.root.join(t.render(&ctx)?),
                    None => self.root.clone(),
                };
                Ok(Service {
                    name: s.name.clone(),
                    cmd,
                    cwd,
                    env,
                })
            })
            .collect()
    }
}

impl Cmd {
    pub fn display(&self) -> String {
        match self {
            Cmd::Shell(s) => format!("sh -c {s:?}"),
            Cmd::Argv(a) => format!("{a:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[env]
GLOBAL = "{{service}}:{{ports.otel}}"

[services.db]
cmd = ["postgres", "-p", "{{ports.db}}"]

[services.api]
cmd = "run --port {{ports.api}}"
cwd = "backend"
env = { DB = "127.0.0.1:{{ports.db}}", N = 3 }
"#;

    #[test]
    fn parse_and_render() {
        let cfg = parse(SAMPLE, PathBuf::from("/r")).unwrap();
        assert_eq!(
            cfg.port_names().into_iter().collect::<Vec<_>>(),
            ["api", "db", "otel"]
        );
        let ports = BTreeMap::from([
            ("api".to_string(), 8000),
            ("db".to_string(), 8001),
            ("otel".to_string(), 8002),
        ]);
        let svcs = cfg.render(&ports, Path::new("/r/.ads"), &[]).unwrap();
        let api = &svcs[0];
        assert_eq!(api.name, "api");
        assert!(matches!(&api.cmd, Cmd::Shell(s) if s == "run --port 8000"));
        assert_eq!(api.cwd, PathBuf::from("/r/backend"));
        let env: BTreeMap<_, _> = api.env.iter().cloned().collect();
        assert_eq!(env["DB"], "127.0.0.1:8001");
        assert_eq!(env["N"], "3");
        assert_eq!(env["GLOBAL"], "api:8002");
        assert_eq!(env["ADS_PORT_DB"], "8001");
        assert_eq!(env["ADS_SERVICE"], "api");
        let db = &svcs[1];
        assert!(matches!(&db.cmd, Cmd::Argv(a) if a == &["postgres", "-p", "8001"]));
        assert_eq!(db.cwd, PathBuf::from("/r"));

        let only = cfg
            .render(&ports, Path::new("/r/.ads"), &["db".into()])
            .unwrap();
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn errors() {
        let e = |s: &str| parse(s, PathBuf::from("/r")).err().unwrap();
        assert_eq!(
            e("[services.a]\ncwd = \".\""),
            "`services.a.cmd` is required"
        );
        assert_eq!(
            e("[services.a]\ncmd = \"x\"\nfoo = 1"),
            "unknown key `services.a.foo`"
        );
        assert_eq!(e("[nope]"), "unknown key `nope`");
        assert_eq!(e(""), "no services defined");
        assert_eq!(
            e("[services.a]\ncmd = []"),
            "`services.a.cmd` must be a string or a non-empty array of strings"
        );
        assert_eq!(
            e("[services.a]\ncmd = \"{{bad}}\""),
            "services.a.cmd: unknown reference `{{bad}}`"
        );
        assert!(e("[services.\"a b\"]\ncmd = \"x\"").contains("invalid service name"));
    }
}
