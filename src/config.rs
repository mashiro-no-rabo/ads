use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub procs: HashMap<String, ProcConfig>,
    /// Allocated ports, keyed by port variable name (e.g. "web" for `{{ port.web }}`)
    #[serde(skip)]
    pub ports: HashMap<String, u16>,
}

#[derive(Debug, Deserialize)]
pub struct ProcConfig {
    /// Command as array: ["program", "arg1", "arg2"]
    pub cmd: Option<Vec<String>>,
    /// Command as shell string: "program arg1 arg2"
    pub shell: Option<String>,
    /// Working directory (resolved to absolute path)
    pub cwd: Option<PathBuf>,
    /// Extra environment variables
    pub env: Option<HashMap<String, String>>,
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let config_dir = path
            .canonicalize()?
            .parent()
            .ok_or("config file has no parent directory")?
            .to_path_buf();

        let content = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&content)?;
        config.validate()?;
        config.resolve_paths(&config_dir)?;

        let port_names = config.collect_port_variables();
        config.ports = Self::allocate_ports(&port_names)?;
        config.render_templates()?;

        Ok(config)
    }

    /// Scan all template strings in the config and return the set of port variable names.
    /// e.g. `{{ port.web }}` yields `"web"`.
    fn collect_port_variables(&self) -> HashSet<String> {
        let mut env = minijinja::Environment::new();
        let mut port_names = HashSet::new();

        let mut templates: Vec<String> = Vec::new();
        for proc in self.procs.values() {
            if let Some(cmd) = &proc.cmd {
                templates.extend(cmd.iter().cloned());
            }
            if let Some(shell) = &proc.shell {
                templates.push(shell.clone());
            }
            if let Some(env_map) = &proc.env {
                templates.extend(env_map.values().cloned());
            }
        }

        for (i, tmpl_str) in templates.iter().enumerate() {
            let name = format!("_{i}");
            if env.add_template_owned(name.clone(), tmpl_str).is_ok() {
                if let Ok(tmpl) = env.get_template(&name) {
                    for var in tmpl.undeclared_variables(true) {
                        if let Some(name) = var.strip_prefix("port.") {
                            port_names.insert(name.to_string());
                        }
                    }
                }
            }
        }

        port_names
    }

    /// Allocate an ephemeral OS port for each port variable name.
    fn allocate_ports(
        names: &HashSet<String>,
    ) -> Result<HashMap<String, u16>, Box<dyn std::error::Error>> {
        let mut ports = HashMap::new();
        for name in names {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();
            // listener is dropped here, freeing the port
            ports.insert(name.clone(), port);
        }
        Ok(ports)
    }

    /// Render all template strings in the config using the allocated ports.
    fn render_templates(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.ports.is_empty() {
            return Ok(());
        }

        let ctx = minijinja::context! { port => &self.ports };

        for proc in self.procs.values_mut() {
            if let Some(cmd) = &mut proc.cmd {
                for arg in cmd.iter_mut() {
                    *arg = Self::render_one(arg, &ctx)?;
                }
            }
            if let Some(shell) = &mut proc.shell {
                *shell = Self::render_one(shell, &ctx)?;
            }
            if let Some(env_map) = &mut proc.env {
                for value in env_map.values_mut() {
                    *value = Self::render_one(value, &ctx)?;
                }
            }
        }

        Ok(())
    }

    fn render_one(
        template: &str,
        ctx: &minijinja::Value,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut env = minijinja::Environment::new();
        env.add_template_owned("t".to_string(), template)?;
        let tmpl = env.get_template("t")?;
        Ok(tmpl.render(ctx)?)
    }

    fn resolve_paths(&mut self, config_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        for (name, proc) in &mut self.procs {
            if let Some(cwd) = &proc.cwd {
                let resolved = if cwd.is_absolute() {
                    cwd.clone()
                } else {
                    config_dir.join(cwd)
                };
                if !resolved.is_dir() {
                    return Err(format!("proc '{name}': cwd '{}' is not a directory", resolved.display()).into());
                }
                proc.cwd = Some(resolved);
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        for (name, proc) in &self.procs {
            match (&proc.cmd, &proc.shell) {
                (Some(_), Some(_)) => {
                    return Err(format!("proc '{name}': specify either `cmd` or `shell`, not both"));
                }
                (None, None) => {
                    return Err(format!("proc '{name}': must specify either `cmd` or `shell`"));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
