mod config;
mod logs;
mod ports;
mod state;
mod supervisor;
mod sys;
mod template;
mod watchdog;

use std::{ffi::OsStr, path::PathBuf, process::ExitCode, time::Duration};

use state::State;

type Res<T> = Result<T, String>;

const GRACE: Duration = Duration::from_secs(5);

const USAGE: &str = "\
ads - agent dev stack

usage: ads [-c ads.toml] <command>

commands:
  up [svc...]             start services in the foreground (Ctrl-C to stop)
  down                    stop the running daemon
  ps                      show service status
  ports [--json]          show assigned ports (ADS_PORT_<NAME>=<port> by default)
  logs [svc...] [-f] [-n N]
                          show the last N (100) log lines, -f to follow
  check                   validate the config and show rendered services
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ads: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Res<()> {
    let mut args = pico_args::Arguments::from_env();
    if args.contains(["-h", "--help"]) {
        print!("{USAGE}");
        return Ok(());
    }
    let explicit: Option<PathBuf> = args
        .opt_value_from_os_str(["-c", "--config"], |s: &OsStr| {
            Ok::<_, String>(PathBuf::from(s))
        })
        .map_err(|e| e.to_string())?;
    let sub = args.subcommand().map_err(|e| e.to_string())?;
    let state = || -> Res<State> {
        Ok(State::new(&config::root_of(&config::find(
            explicit.clone(),
        )?)))
    };
    match sub.as_deref() {
        Some("__watchdog") => {
            watchdog::run();
            Ok(())
        }
        Some("up") => {
            let only = free(args)?;
            supervisor::up(&config::find(explicit)?, &only)
        }
        Some("down") => {
            free_none(args)?;
            supervisor::down(&state()?)
        }
        Some("ps") => {
            free_none(args)?;
            supervisor::ps(&state()?)
        }
        Some("ports") => {
            let json = args.contains("--json");
            free_none(args)?;
            supervisor::ports_cmd(&state()?, if json { "json" } else { "env" })
        }
        Some("logs") => {
            let follow = args.contains(["-f", "--follow"]);
            let n: Option<usize> = args
                .opt_value_from_str(["-n", "--lines"])
                .map_err(|e| e.to_string())?;
            let services = free(args)?;
            logs::cmd(&state()?, &services, n.unwrap_or(100), follow)
        }
        Some("check") => {
            free_none(args)?;
            supervisor::check(&config::find(explicit)?)
        }
        Some(other) => Err(format!("unknown command `{other}`, see `ads --help`")),
        None => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn free(args: pico_args::Arguments) -> Res<Vec<String>> {
    args.finish()
        .into_iter()
        .map(|a| match a.into_string() {
            Ok(s) if s.starts_with('-') => Err(format!("unknown flag `{s}`")),
            Ok(s) => Ok(s),
            Err(a) => Err(format!("invalid argument {a:?}")),
        })
        .collect()
}

fn free_none(args: pico_args::Arguments) -> Res<()> {
    match free(args)?.first() {
        Some(a) => Err(format!("unexpected argument `{a}`")),
        None => Ok(()),
    }
}
