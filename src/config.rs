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
    run: Vec<ServiceSpec>,
    pub services: Vec<ServiceSpec>,
    pub open: Vec<(String, Template)>,
}

pub const RUN: &str = "run";

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
    let mut run = Vec::new();
    let mut services = Vec::new();
    let mut open = Vec::new();
    for (key, value) in &table {
        match key.as_str() {
            "env" => env = parse_env(value, "env")?,
            "open" => {
                let t = value.as_table().ok_or("`open` must be a table")?;
                for (name, v) in t {
                    let path = format!("open.{name}");
                    if !template::valid_name(name) {
                        return Err(format!(
                            "invalid open name `{name}` (use letters, digits, `_`, `-`)"
                        ));
                    }
                    let src = match v {
                        Value::String(s) if !s.trim().is_empty() => s.clone(),
                        Value::Integer(p) if (1..=65535).contains(p) => p.to_string(),
                        _ => {
                            return Err(format!(
                                "`{path}` must be a non-empty URL string or a port integer (1-65535)"
                            ));
                        }
                    };
                    open.push((name.clone(), Template::parse(&src, &path)?));
                }
            }
            "run" => {
                let steps = value
                    .as_array()
                    .ok_or("`run` must be an array of tables, write it as `[[run]]`")?;
                for (i, v) in steps.iter().enumerate() {
                    run.push(parse_spec(RUN, &format!("run[{i}]"), v)?);
                }
            }
            "services" => {
                let t = value.as_table().ok_or("`services` must be a table")?;
                for (name, v) in t {
                    if !template::valid_name(name) {
                        return Err(format!(
                            "invalid service name `{name}` (use letters, digits, `_`, `-`)"
                        ));
                    }
                    if name == RUN {
                        return Err(format!(
                            "service name `{RUN}` is reserved for `[[run]]` steps"
                        ));
                    }
                    services.push(parse_spec(name, &format!("services.{name}"), v)?);
                }
            }
            _ => return Err(format!("unknown key `{key}`")),
        }
    }
    if services.is_empty() {
        return Err("no services defined".into());
    }
    // Keep services sorted as before; only [open] uses config-file order.
    services.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Config {
        root,
        env,
        run,
        services,
        open,
    })
}

fn parse_spec(name: &str, path: &str, v: &Value) -> Res<ServiceSpec> {
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
        .collect::<BTreeMap<_, _>>()
        .into_iter()
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
            .chain(
                self.run
                    .iter()
                    .chain(&self.services)
                    .flat_map(|s| s.templates()),
            )
            .chain(self.open.iter().map(|(_, t)| t))
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
        self.services
            .iter()
            .filter(|s| only.is_empty() || only.contains(&s.name))
            .map(|s| self.render_one(s, ports, state))
            .collect()
    }

    pub fn render_run(&self, ports: &BTreeMap<String, u16>, state: &Path) -> Res<Vec<Service>> {
        self.run
            .iter()
            .map(|s| self.render_one(s, ports, state))
            .collect()
    }

    fn render_one(
        &self,
        s: &ServiceSpec,
        ports: &BTreeMap<String, u16>,
        state: &Path,
    ) -> Res<Service> {
        let lookup = |n: &str| std::env::var(n).ok();
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
            CmdSpec::Argv(a) => Cmd::Argv(a.iter().map(|t| t.render(&ctx)).collect::<Res<_>>()?),
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
GLOBAL = "{{service}}:{{ports.cache}}"

[services.db]
cmd = ["postgres", "-p", "{{ports.db}}"]

[services.api]
cmd = "run --port {{ports.api}}"
cwd = "backend"
env = { DB = "127.0.0.1:{{ports.db}}", N = 3 }
"#;

    #[test]
    fn open_config_and_port_discovery() {
        let text = format!(
            "{SAMPLE}\n[open]\nweb = \"http://localhost:{{{{ports.web}}}}/app\"\nfixed = 3000\n"
        );
        let cfg = parse(&text, PathBuf::from("/r")).unwrap();
        assert!(cfg.port_names().contains("web"));
        assert_eq!(cfg.open.len(), 2);
        assert_eq!(cfg.open[0].0, "web");
        assert_eq!(cfg.open[1].0, "fixed");
        for value in ["0", "65536", "-1", "true", "[]", "\"\"", "\"{{unknown}}\""] {
            let text = format!("{SAMPLE}\n[open]\nweb = {value}\n");
            assert!(
                parse(&text, PathBuf::from("/r"))
                    .err()
                    .unwrap()
                    .contains("open.web")
            );
        }
    }

    #[test]
    fn parse_and_render() {
        let cfg = parse(SAMPLE, PathBuf::from("/r")).unwrap();
        assert_eq!(
            cfg.port_names().into_iter().collect::<Vec<_>>(),
            ["api", "cache", "db"]
        );
        let ports = BTreeMap::from([
            ("api".to_string(), 8000),
            ("db".to_string(), 8001),
            ("cache".to_string(), 8002),
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
    fn run_steps() {
        let cfg = parse(
            "[[run]]\ncmd = \"migrate --port {{ports.db}}\"\ncwd = \"backend\"\n\n[[run]]\ncmd = [\"seed\", \"{{service}}\"]\n\n[services.db]\ncmd = \"db {{ports.db}}\"\n",
            PathBuf::from("/r"),
        )
        .unwrap();
        assert_eq!(cfg.port_names().into_iter().collect::<Vec<_>>(), ["db"]);
        let ports = BTreeMap::from([("db".to_string(), 8000)]);
        let steps = cfg.render_run(&ports, Path::new("/r/.ads")).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0].cmd, Cmd::Shell(s) if s == "migrate --port 8000"));
        assert_eq!(steps[0].cwd, PathBuf::from("/r/backend"));
        assert!(matches!(&steps[1].cmd, Cmd::Argv(a) if a == &["seed", "run"]));
        assert_eq!(
            cfg.render(&ports, Path::new("/r/.ads"), &[]).unwrap().len(),
            1
        );
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
        assert_eq!(
            e("[run]\ncmd = \"x\"\n[services.a]\ncmd = \"x\""),
            "`run` must be an array of tables, write it as `[[run]]`"
        );
        assert_eq!(
            e("[[run]]\ncwd = \".\"\n[services.a]\ncmd = \"x\""),
            "`run[0].cmd` is required"
        );
        assert_eq!(
            e("[services.run]\ncmd = \"x\""),
            "service name `run` is reserved for `[[run]]` steps"
        );
    }
}
