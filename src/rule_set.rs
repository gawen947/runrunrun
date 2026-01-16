use std::{
    cell::OnceCell, collections::HashMap, os::unix::process::CommandExt, path::Path,
    process::Command,
};

use anyhow::{Result, anyhow, ensure};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{RegexBuilder, RegexSet, RegexSetBuilder};

use crate::{
    types::{ActionCommand, AliasIdentifier, ProfileIdentifier},
    utils,
};

/// Iteratively build and resolve rules.
pub struct RuleSetBuilder {
    profile: ProfileIdentifier,
    case_insensitive: bool,

    alias: HashMap<AliasIdentifier, ActionCommand>,

    regex_rules: Vec<Rule>,
    glob_rules: Vec<Rule>,
}

/// Contains set of resolved rules that can be matched against an input.
pub struct RuleSet {
    regex_set: RegexSet,
    glob_set: GlobSet,

    builder: RuleSetBuilder,
}

/// Origin of the rule creation in the config.
#[derive(Debug, Clone)]
pub struct ConfigOrigin {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

/// Specify if the rule was explicitely stated in config or created from an import.
#[derive(Debug)]
pub enum RuleOrigin {
    Explicit,         // comes directly from the config
    Imported(String), // created from an imported .desktop file
}

/// Pattern that this rule should match (left part of the rule).
#[derive(Debug)]
pub enum Pattern {
    Regex(String),
    Glob(String),
}

/// Type of action associated to the rule (right part of the rule).
#[derive(Debug)]
pub enum Action {
    Alias(AliasIdentifier), // rule action references an alias
    Command(ActionCommand), // rule action directly reference a command to execute
}

/**
  A rule that map a matching pattern to an action.
  If this action is an alias they must be resolved into an actual command.
  Then the rule must be substituted with some input to prepare the actual command to be executed.
  The rule_origin and config_origin specify what created this rule (explicit in config
  or imported by ':import') and the place in the config that triggered this rule creation.
*/
#[derive(Debug)]
pub struct Rule {
    pub pattern: Pattern, // pattern that should be matched (left side in config)
    pub action: Action,   // action as specified in the config (right side in config)
    pub resolved: OnceCell<ActionCommand>, // action with eventual alias resolved
    pub execution: OnceCell<ActionCommand>, // action substituted and ready for execution
    pub case_insensitive: bool,

    pub rule_origin: RuleOrigin, // where that rule was declared (explicit in config or created from import)
    pub config_origin: ConfigOrigin, // which line in the config was at the origin of this rule
}

/// Resolve an Action (alias, command) into a action_command that can be executed.
trait RuleResolver {
    fn resolve<'a>(&'a self, action: &'a Action) -> Result<&'a str>;
}

impl RuleSetBuilder {
    pub fn new(profile: ProfileIdentifier, case_insensitive: bool) -> Self {
        // todo: accept &ProfileIdentifier instead
        Self {
            profile,
            case_insensitive,
            alias: HashMap::new(),
            regex_rules: vec![],
            glob_rules: vec![],
        }
    }

    /// Add an alias to the rule set. It can be recalled when you add a rule.
    pub fn alias(&mut self, identifier: AliasIdentifier, action_command: ActionCommand) {
        // todo: accept &AliasIdentifier, &Action
        self.alias.insert(identifier, action_command);
    }

    /// Add a rule that comes from the config file directly with an action.
    pub fn rule_with_command(
        &mut self,
        config_origin: ConfigOrigin,
        pattern: Pattern,
        action_command: ActionCommand,
    ) -> () {
        self.rule(
            pattern,
            Action::Command(action_command),
            self.case_insensitive,
            RuleOrigin::Explicit,
            config_origin,
        );
    }

    /// Add a rule that comes from the config file and references an alias.
    pub fn rule_with_alias(
        &mut self,
        config_origin: ConfigOrigin,
        pattern: Pattern,
        alias_identifier: AliasIdentifier,
    ) -> Result<()> {
        self.rule(
            pattern,
            Action::Alias(alias_identifier),
            self.case_insensitive,
            RuleOrigin::Explicit,
            config_origin,
        );

        Ok(())
    }

    #[cfg(feature = "import")]
    /// Add a rule that comes from an imported desktop file.
    pub fn rule_with_import(
        &mut self,
        config_origin: &ConfigOrigin,
        imported_path: &Path,
        ignore_missing_attrs: bool,
    ) -> Result<()> {
        use anyhow::Context;

        let desktop_entry = freedesktop_entry_parser::parse_entry(imported_path)?;
        let desktop_section = desktop_entry
            .section("Desktop Entry")
            .context("missing 'Desktop Entry' section")?;

        let get_attr = |name: &str| -> Result<Option<&str>> {
            match desktop_section.attr(name).get(0) {
                Some(val) => Ok(Some(val)),
                None if ignore_missing_attrs => Ok(None),
                None => anyhow::bail!("missing '{}' attribute", name),
            }
        };

        let Some(exec_cmd) = get_attr("Exec")?.map(|s| {
            // Handle most common desktop flags. We still don't handle %i, %c, %k.
            ["%U", "%u", "%F", "%f"]
                .iter()
                .fold(s.to_string(), |acc, format_specifier| {
                    acc.replace(format_specifier, "%s")
                })
        }) else {
            return Ok(());
        };
        let Some(mime_types) = get_attr("MimeType")?.map(|s| s.to_string()) else {
            return Ok(());
        };

        for mime_type in mime_types.split(";").filter(|s| !s.is_empty()) {
            if let Some(extensions) = mime_guess::get_mime_extensions_str(mime_type) {
                for extension in extensions {
                    let pattern = Pattern::Glob(format!("*.{}", extension));

                    self.rule(
                        pattern,
                        Action::Command(exec_cmd.to_string()),
                        self.case_insensitive,
                        RuleOrigin::Imported(imported_path.to_string_lossy().to_string()),
                        config_origin.clone(),
                    )
                }
            }
        }

        Ok(())
    }

    fn rule(
        &mut self,
        pattern: Pattern,
        action: Action,
        case_insensitive: bool,
        rule_origin: RuleOrigin,
        config_origin: ConfigOrigin,
    ) {
        let rule = Rule {
            pattern,
            action,
            resolved: OnceCell::new(),
            execution: OnceCell::new(),
            case_insensitive,
            rule_origin,
            config_origin,
        };

        match rule.pattern {
            Pattern::Regex(_) => self.regex_rules.push(rule),
            Pattern::Glob(_) => self.glob_rules.push(rule),
        }
    }

    fn resolve(&self, rules: &[Rule]) -> Result<()> {
        for rule in rules {
            rule.resolve(self)?;
        }
        Ok(())
    }

    pub fn build(mut self) -> Result<RuleSet> {
        // resolve each rule (map alias to action)
        self.resolve(&self.regex_rules)?;
        self.resolve(&self.glob_rules)?;

        // reverse the patterns to match the last one first
        self.regex_rules.reverse();
        self.glob_rules.reverse();

        let regex_patterns: Vec<&str> = self
            .regex_rules
            .iter()
            .map(|r| r.pattern_as_str())
            .collect();
        let regex_set = RegexSetBuilder::new(&regex_patterns)
            .case_insensitive(self.case_insensitive)
            .build()?;

        let mut glob_set_builder = GlobSetBuilder::new();
        for rule in &self.glob_rules {
            glob_set_builder.add(
                GlobBuilder::new(rule.pattern_as_str())
                    .case_insensitive(self.case_insensitive)
                    .build()?,
            );
        }
        let glob_set = glob_set_builder.build()?;

        Ok(RuleSet {
            regex_set,
            glob_set,
            builder: self,
        })
    }
}

impl RuleResolver for &RuleSetBuilder {
    fn resolve<'a>(&'a self, action: &'a Action) -> Result<&'a str> {
        match action {
            Action::Command(action_command) => Ok(action_command),
            Action::Alias(alias_identifier) => self
                .alias
                .get(alias_identifier)
                .ok_or_else(|| {
                    anyhow!(
                        "Alias '{}' does not exist in profile '{}'",
                        alias_identifier,
                        self.profile
                    )
                })
                .map(|s| s.as_str()),
        }
    }
}

impl RuleSet {
    fn match_glob(&self, input: &str) -> Option<&Rule> {
        let matches = self.glob_set.matches(input);

        if let Some(index) = matches.first() {
            Some(
                self.builder
                    .glob_rules
                    .get(*index)
                    .expect("Glob first match gave a non existing index"),
            )
        } else {
            None
        }
    }

    fn match_regex(&self, input: &str) -> Option<&Rule> {
        let matches: Vec<usize> = self.regex_set.matches(input).into_iter().collect();

        if let Some(index) = matches.first() {
            Some(
                self.builder
                    .regex_rules
                    .get(*index)
                    .expect("Regex first match gave a non existing index"),
            )
        } else {
            None
        }
    }

    /// Return the first glob or regex rule that matches the input.
    pub fn r#match(&self, input: &str) -> Option<&Rule> {
        if let r @ Some(_) = self.match_regex(input) {
            return r;
        }
        if let r @ Some(_) = self.match_glob(input) {
            return r;
        }
        None
    }
}

impl Rule {
    pub fn pattern_as_str(&self) -> &str {
        match &self.pattern {
            Pattern::Glob(pattern) | Pattern::Regex(pattern) => pattern,
        }
    }

    /// Map the action as orginally speicfied to an actual command to execute.
    fn resolve(&self, resolver: impl RuleResolver) -> Result<()> {
        let resolved_action = resolver.resolve(&self.action)?.to_string();
        self.resolved
            .set(resolved_action)
            .expect("rule should not be already resolved");
        Ok(())
    }

    pub fn is_executable(&self) -> bool {
        self.execution.get().is_some()
    }

    pub fn get_executed_action(&self) -> Result<&str> {
        Ok(self
            .execution
            .get()
            .ok_or_else(|| anyhow!("Rule was not prepared for execution."))?)
    }

    /// Substitute %s in the action with the input that we matched against
    fn substitute_file(action: String, input: &str) -> Result<String> {
        // automatically append "%s" if not present
        let action_with_tag = if action.contains("%s") {
            action
        } else {
            format!("{} %s", action)
        };

        // replace with the matched input
        let action_with_input = action_with_tag.replace("%s", &utils::quote(input)?);

        Ok(action_with_input)
    }

    /// Substitute in the action the captures of the Regex with %1, %2, %3, ...
    fn substitute_captures(mut action: String, captures: Vec<String>) -> Result<String> {
        for (i, capture) in captures.iter().enumerate() {
            let tag = format!("%{}", i + 1); // %1, %2, %3, ...
            action = action.replace(&tag, &utils::quote(capture)?)
        }

        Ok(action)
    }

    /// Substitute in the action the input that we matched against and the captures of the Regex.
    fn substitute(&self, captures: Vec<String>, input: &str) -> Result<()> {
        let resolved_action = self.resolved.get().expect("rule must be resolved");

        let executable_action = Self::substitute_captures(resolved_action.to_string(), captures)?;
        let executable_action = Self::substitute_file(executable_action, input)?;
        self.execution
            .set(executable_action)
            .expect("rule should not be ready for execution");
        Ok(())
    }

    /// Cature the matched regex group into a vector.
    fn captures(&self, input: &str) -> Result<Vec<String>> {
        // captures is a regex thing, skip if this is a glob pattern
        if let Pattern::Glob(_) = self.pattern {
            return Ok(vec![]);
        }

        // match capture groups of the regex
        let re = RegexBuilder::new(self.pattern_as_str())
            .case_insensitive(self.case_insensitive)
            .build()?;
        let captures = re
            .captures(input)
            .ok_or_else(|| anyhow!("The rule should already match in order to capture"))?;

        let captures_strings: Vec<String> = captures
            .iter()
            .skip(1) // first capture is the full match (we don't need that)
            .filter_map(|m| m.map(|m| m.as_str().to_string()))
            .collect();

        Ok(captures_strings)
    }

    /// Prepare the rule for execution with proper substitution against the matched file.
    pub fn prepare(&self, input: &str) -> Result<()> {
        let captures = self.captures(input)?;
        self.substitute(captures, input)?;
        Ok(())
    }

    /// Execute the rule action as a shell command (only returns if there was an error)
    pub fn exec(&self, fork: bool, sh: &Option<Vec<&str>>) -> Result<()> {
        let default_shell = vec!["sh", "-c"];
        let shell = sh.as_ref().unwrap_or(&default_shell);
        let command_to_execute = self
            .execution
            .get()
            .ok_or_else(|| anyhow!("Rule not prepared for execution"))?;

        ensure!(
            shell.len() > 0,
            "provided shell should have at least one argument"
        );

        let mut cmd = Command::new(shell[0]);
        cmd.args(&shell[1..]).arg(command_to_execute);

        if fork {
            Ok(cmd.spawn().map(|_| ())?)
        } else {
            Err(cmd.exec())?
        }
    }
}

/* todo: add unit test for RuleSetBuilder and RuleSet, test matching, substitution and eventually
   execution
*/
