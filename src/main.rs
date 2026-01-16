use std::{
    env,
    io::{self, BufRead},
    path::{Path, PathBuf},
    process::exit,
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use log::{debug, error, info, warn};
use runrunrun::{
    rrr::{Rrr, RrrBuilder},
    rule_set::{ExecutionType, Rule},
};

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

    /// On execution failure, try the previous matching rule until one succeeds
    #[arg(
        short = 'f',
        long = "fallback",
        env = "RRR_FALLBACK",
        default_value = "false"
    )]
    fallback: bool,

    /// Change the default shell used to execute actions to another command
    #[arg(long = "sh", env = "RRR_SHELL")]
    sh: Option<String>,

    /// Input arguments
    #[arg(required_unless_present = "stdin")]
    inputs: Vec<String>,
}

/*
  Represents the result of a rule execution, and if execution happened.
  We need a way to treat errors in the execution of rules separately
  and this struct makes code that treat execution errors clearer.
*/
struct ExecutionResult(Option<Result<()>>);

impl ExecutionResult {
    fn no_execution() -> Self {
        ExecutionResult(None)
    }

    fn with_execution(result: Result<()>) -> Self {
        ExecutionResult(Some(result))
    }

    fn execution_result(self) -> Result<()> {
        self.0.unwrap_or(Ok(()))
    }
}

fn process_rule(
    args: &Args,
    sh: &Option<Vec<&str>>,
    input: &str,
    rule: &Rule,
) -> Result<ExecutionResult> {
    debug!("matched rule for '{}': {:?}", input, rule);
    rule.prepare(input)
        .context("preparing the rule for execution")?;
    let executed_action = rule.get_executed_action()?;

    if args.query {
        println!("{}", executed_action);
    } else {
        if !args.dry_run {
            info!(
                "{} '{}'",
                if args.fork { "fork-exec" } else { "exec" },
                executed_action
            );

            let execution_type = if args.fallback {
                ExecutionType::WaitSuccessSignalOk
            } else if args.fork {
                ExecutionType::Fork
            } else {
                ExecutionType::Exec
            };

            let result = rule
                .exec(execution_type, sh)
                .with_context(|| format!("executing '{}'", executed_action));

            return Ok(ExecutionResult::with_execution(result));
        }
    }

    Ok(ExecutionResult::no_execution())
}

fn process_input(args: &Args, sh: &Option<Vec<&str>>, rrr: &Rrr, input: &str) -> Result<()> {
    if args.fallback {
        process_input_with_fallback(args, sh, rrr, input)
    } else {
        process_input_without_fallback(args, sh, rrr, input)
    }
}

fn process_input_without_fallback(
    args: &Args,
    sh: &Option<Vec<&str>>,
    rrr: &Rrr,
    input: &str,
) -> Result<()> {
    if let Some(rule) = rrr.profile(&args.profile)?.r#match(input) {
        process_rule(args, sh, input, rule)?.execution_result()?;
    } else {
        warn!("no match for '{}'", input);
    }

    Ok(())
}

fn process_input_with_fallback(
    args: &Args,
    sh: &Option<Vec<&str>>,
    rrr: &Rrr,
    input: &str,
) -> Result<()> {
    let matches = rrr.profile(&args.profile)?.matches(input);

    let mut match_found = false;
    for rule in matches {
        match_found = true;
        match process_rule(args, sh, input, rule)?.0 {
            Some(Ok(())) => return Ok(()), // match found and executed correctly
            Some(Err(e)) => {
                // match found but execution resulted in an error
                info!("execution failed (continuing with next match): {:?}", e);
            }
            None => {} // nothing executed (dry-run or query) => proceed with other matches
        }
    }

    if !match_found {
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
    let mut builder = RrrBuilder::new(!args.case_sensitive, Some(vec![args.profile.to_string()]));

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

        let mut config_loaded = false;
        if main_config_path.is_file() {
            debug!("loading config '{}'", main_config_path.display());
            builder = builder.config(&main_config_path).with_context(|| {
                format!(
                    "cannot load configuration file '{}'",
                    main_config_path.display()
                )
            })?;
            config_loaded = true;
        }

        if home_config_path.is_file() {
            debug!("loading config '{}'", home_config_path.display());
            builder = builder.config(&home_config_path).with_context(|| {
                format!(
                    "cannot load configuration file '{}'",
                    home_config_path.display()
                )
            })?;
            config_loaded = true;
        }

        ensure!(
            config_loaded,
            "none of the configuration files '{}' nor '{}' could be loaded",
            main_config_path.display(),
            home_config_path.display()
        );
    }

    // some preparation for the execution
    let rrr = builder.build()?;
    // live and let (the Vec<&str>) live
    let sh = args
        .sh
        .as_ref()
        .map(|s| shlex::split(&s).context("invalid SH substitute"))
        .transpose()?;
    let sh_str: Option<Vec<&str>> = sh.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());

    // match the inputs
    if args.stdin {
        debug!("process inputs from stdin");
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let input = line.context("reading from stdin")?;
            process_input(&args, &sh_str, &rrr, &input)?;
        }
    } else {
        debug!("process inputs from arguments");
        for input in &args.inputs {
            process_input(&args, &sh_str, &rrr, input)?;
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
