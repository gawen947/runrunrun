use std::{
    borrow::Cow, collections::HashMap, os::unix::process::CommandExt, path::Path, process::Command,
};

use anyhow::{Result, anyhow};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};

use crate::types::{Action, AliasIdentifier, ProfileIdentifier};

#[derive(Debug)]
pub struct RuleSetBuilder {
    profile: ProfileIdentifier,
    case_insensitive: bool,

    alias: HashMap<AliasIdentifier, Action>,

    regex_rules: Vec<Rule>,
    glob_rules: Vec<Rule>,
}

#[derive(Debug)]
pub struct RuleSet {
    regex_set: RegexSet,
    glob_set: GlobSet,

    builder: RuleSetBuilder,
}

#[derive(Clone, Debug)]
pub struct ConfigOrigin {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub enum RuleOrigin {
    Alias(AliasIdentifier),
    ImportedDesktop(String),
    Config,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Regex(String),
    Glob(String),
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub pattern: Pattern,
    pub action: Action,
    pub origin: RuleOrigin,
    pub config_origin: ConfigOrigin,
    pub case_insensitive: bool,
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
    pub fn alias(&mut self, identifier: AliasIdentifier, action: Action) {
        // todo: accept &AliasIdentifier, &Action
        self.alias.insert(identifier, action);
    }

    /// Add a rule that comes from the config file directly with an action.
    pub fn rule_with_action(
        &mut self,
        config_origin: ConfigOrigin,
        pattern: Pattern,
        action: Action,
    ) -> () {
        self.rule(Rule {
            pattern,
            action,
            config_origin,
            origin: RuleOrigin::Config,
            case_insensitive: self.case_insensitive,
        });
    }

    /// Add a rule that comes from the config file and references an alias.
    pub fn rule_with_alias(
        &mut self,
        config_origin: ConfigOrigin,
        pattern: Pattern,
        alias_identifier: AliasIdentifier,
    ) -> Result<()> {
        let action = self
            .alias
            .get(&alias_identifier)
            .ok_or_else(|| {
                anyhow!(
                    "Alias '{}' does not exist in profile '{}'",
                    alias_identifier,
                    self.profile
                )
            })?
            .to_string();

        self.rule(Rule {
            pattern,
            action: action.to_string(),
            config_origin,
            origin: RuleOrigin::Alias(alias_identifier),
            case_insensitive: self.case_insensitive,
        });

        Ok(())
    }

    /// Add a rule that comes from an imported desktop file.
    pub fn rule_with_import(
        &mut self,
        config_origin: ConfigOrigin,
        imported_path: &Path,
    ) -> Result<()> {
        todo!()
    }

    fn rule(&mut self, rule: Rule) {
        match rule.pattern {
            Pattern::Regex(_) => self.regex_rules.push(rule),
            Pattern::Glob(_) => self.glob_rules.push(rule),
        }
    }

    pub fn build(self) -> Result<RuleSet> {
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

impl RuleSet {
    fn match_glob(&self, input: &str) -> Option<Rule> {
        let matches = self.glob_set.matches(input);

        if let Some(index) = matches.first() {
            Some(
                self.builder
                    .glob_rules
                    .get(*index)
                    .expect("Glob first match gave a non existing index")
                    .clone(),
            )
        } else {
            None
        }
    }

    fn match_regex(&self, input: &str) -> Option<Rule> {
        let matches: Vec<usize> = self.regex_set.matches(input).into_iter().collect();

        if let Some(index) = matches.first() {
            Some(
                self.builder
                    .regex_rules
                    .get(*index)
                    .expect("Regex first match gave a non existing index")
                    .clone(),
            )
        } else {
            None
        }
    }

    /// Return the first glob or regex rule that matches the input.
    pub fn r#match(&self, input: &str) -> Option<Rule> {
        if let r @ Some(_) = self.match_glob(input) {
            return r;
        }
        if let r @ Some(_) = self.match_regex(input) {
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

    /// Substitute %s in the action with the input that we matched against
    fn substitute_file(self, input: &str) -> Self {
        // automatically append "%s" if not present
        let action = if self.action.contains("%s") {
            Cow::Borrowed(&self.action)
        } else {
            Cow::Owned(format!("{} %s", self.action))
        };

        // replace with the matched input
        let action = action.replace("%s", &format!("{:?}", input));

        Self { action, ..self }
    }

    /// Substitute in the action the captures of the Regex with %1, %2, %3, ...
    fn substitute_captures(self, captures: Vec<String>) -> Self {
        let mut result = self.action.to_string();

        for (i, capture) in captures.iter().enumerate() {
            let tag = format!("%{}", i + 1); // %1, %2, %3, ...
            result = result.replace(&tag, &format!("{:?}", capture))
        }

        Self {
            action: result,
            ..self
        }
    }

    /// Substitute in the action the input that we matched against and the captures of the Regex.
    fn substitute(self, captures: Vec<String>, input: &str) -> Self {
        self.substitute_captures(captures).substitute_file(input)
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
    pub fn prepare(self, input: &str) -> Result<Self> {
        let captures = self.captures(input)?;
        Ok(self.substitute(captures, input))
    }

    /// Execute the rule action as a shell command (only returns if there was an error)
    pub fn exec(&self, fork: bool) -> Result<()> {
        // todo: use type ! when not experimental anymore
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&self.action);

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
