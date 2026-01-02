use std::{
    env,
    io::{self, BufRead},
    path::{Path, PathBuf},
    process::exit,
};

use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, error, info, warn};
use runrunrun::rrr::{Rrr, RrrBuilder};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Increase verbosity level
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Do not execute any matching rule
    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    /// Choose the main configuration file
    #[arg(short, long, env = "RRR_CONFIG")]
    config: Option<PathBuf>,

    /// Choose the profile
    #[arg(short, long, env = "RRR_PROFILE", default_value = "default")]
    profile: String,

    /// Just print the action instead of executing it
    #[arg(short, long)]
    query: bool,

    /// Match in case sensitive mode
    #[arg(
        short = 's',
        long = "case-sensitive",
        env = "RRR_CASE_SENSITIVE",
        default_value = "false"
    )]
    case_sensitive: bool,

    /// Read the input from stdin
    #[arg(long = "stdin")]
    stdin: bool,

    /// Run action in a child process (fork + exec), instead of replacing the current process
    #[arg(short = 'F', long = "fork")]
    fork: bool,

    /// Change the default shell used to execute actions to another command
    #[arg(long = "sh", env = "RRR_SHELL")]
    sh: Option<String>,

    /// Input arguments
    #[arg(required_unless_present = "stdin")]
    inputs: Vec<String>,
}

fn r#match(args: &Args, rrr: &Rrr, input: &str) -> Result<()> {
    if let Some(rule) = rrr.profile(&args.profile)?.r#match(input) {
        debug!("matched rule for '{}': {:?}", input, rule);
        let rule = rule
            .prepare(input)
            .context("preparing the rule for execution")?;

        if args.query {
            println!("{}", rule.action);
        } else {
            if !args.dry_run {
                info!(
                    "{} '{}'",
                    if args.fork { "fork-exec" } else { "exec" },
                    rule.action
                );
                rule.exec()
                    .with_context(|| format!("executing '{}'", rule.action))?;
            }
        }
    } else {
        warn!("no match for '{}'", input);
    }

    Ok(())
}

fn try_main() -> Result<()> {
    let args = Args::parse();

    // configure logger
    stderrlog::new()
        .module(module_path!())
        .verbosity(args.verbose as usize)
        .timestamp(stderrlog::Timestamp::Microsecond)
        .init()
        .unwrap();
    debug!("log operational");

    // load configuration
    let mut builder = RrrBuilder::new();

    if let Some(config_path) = &args.config {
        debug!("loading config '{}'", config_path.display());
        builder = builder.config(&config_path).with_context(|| {
            format!("cannot load configuration file '{}'", config_path.display())
        })?;
    } else {
        let mut main_config_path: PathBuf = match env::consts::OS {
            "freebsd" => "/usr/local/etc".into(),
            _ => "/etc".into(),
        };
        main_config_path.push("rrr.conf");

        let home_dir = env::var("HOME").context("cannot read HOME env")?;
        let home_config_path = Path::new(&home_dir).join(".config").join("rrr.conf");

        debug!("loading config '{}'", main_config_path.display());
        builder = builder.config(&main_config_path).with_context(|| {
            format!(
                "cannot load configuration file '{}'",
                main_config_path.display()
            )
        })?;

        if home_config_path.is_file() {
            debug!("loading config '{}'", home_config_path.display());
            builder = builder.config(&home_config_path).with_context(|| {
                format!(
                    "cannot load configuration file '{}'",
                    home_config_path.display()
                )
            })?;
        }
    }

    // match the inputs
    let rrr = builder.build()?;
    if args.stdin {
        debug!("process inputs from stdin");
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let input = line.context("reading from stdin")?;
            r#match(&args, &rrr, &input)?;
        }
    } else {
        debug!("process inputs from arguments");
        for input in &args.inputs {
            r#match(&args, &rrr, input)?;
        }
    }

    debug!("all inputs processed");

    Ok(())
}

fn main() {
    if let Err(e) = try_main() {
        error!("{:#}", e);
        exit(1);
    }
}

